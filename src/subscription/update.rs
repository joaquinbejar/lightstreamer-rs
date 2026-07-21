//! Decoding of the pipe-separated value list carried by a real-time update.
//!
//! The third argument of a `U` notification is a second-level syntax of its
//! own: a `|`-separated list of tokens that is *not* percent-decoded during the
//! first parsing pass [`docs/spec/04-notifications.md` §1]. Each token is
//! either actual content, one of the markers `#` (null), `$` (empty) or the
//! empty token (unchanged), or a `^`-prefixed directive — a run of unchanged
//! fields (`^N`) or a diff to apply to the field's previous value (`^T`, `^P`)
//! [`docs/spec/04-notifications.md` §2.2].
//!
//! [`ItemState`] holds the field values of one subscribed item and applies
//! those tokens with the normative algorithm of
//! [`docs/spec/04-notifications.md` §2.3].
//!
//! Deliberately **not** modelled here:
//!
//! - The snapshot-vs-real-time decision table of §2.4. It is a function of
//!   `LS_snapshot`, `LS_mode`, whether `EOS` has arrived and whether this is
//!   the first notification — none of which is item *field* state. It belongs
//!   to the layer that owns the subscription configuration.
//! - The COMMAND-mode row state machine of §3.3. The key and command field
//!   *positions* arrive on `SUBCMD` and the values at those positions are
//!   ordinary fields decoded exactly like any other; mapping them onto rows is
//!   the subscription manager's job.

use std::sync::OnceLock;

use crate::protocol::ProtocolError;
use crate::protocol::escaping::percent_decode;

/// Builds the error every failure in this module reports.
///
/// A malformed value list is never a panic and never a silently dropped field
/// [`docs/spec/04-notifications.md` §2.3].
#[cold]
#[inline(never)]
fn field_value_error(reason: impl Into<String>) -> ProtocolError {
    ProtocolError::FieldValue {
        reason: reason.into(),
    }
}

// ---------------------------------------------------------------------------
// Field state
// ---------------------------------------------------------------------------

/// The value of one field of one item.
///
/// TLCP distinguishes three states that must not be collapsed: a field that has
/// never carried a value, a field explicitly set to **null** by the `#` marker,
/// and a field holding a string — which may legitimately be the **empty**
/// string, set by the `$` marker [`docs/spec/04-notifications.md` §2.2].
#[derive(Debug, Clone, PartialEq, Eq)]
enum FieldSlot {
    /// No update has ever set this field.
    ///
    /// Not reachable after the first update of a subscription: the empty token
    /// and every `^` token "never happens on the first update"
    /// [`docs/spec/04-notifications.md` §2.3], so the first update necessarily
    /// carries content, `#` or `$` for every field of the schema.
    Unset,
    /// The field is null — the `#` marker
    /// [`docs/spec/04-notifications.md` §2.2].
    Null,
    /// The field holds a string, possibly empty — the `$` marker sets it to the
    /// empty string [`docs/spec/04-notifications.md` §2.2].
    Value(String),
}

/// The client-side state of one subscribed item: the current value of every
/// field of the subscription schema.
///
/// The schema width is fixed at construction, because it is announced once by
/// `SUBOK`/`SUBCMD` before any update arrives
/// [`docs/spec/04-notifications.md` §3.1, §3.2].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ItemState {
    fields: Vec<FieldSlot>,
}

impl ItemState {
    /// Creates the state of an item whose schema has `field_count` fields.
    ///
    /// Every field starts out never-set; the first update of a subscription
    /// carries a value for all of them
    /// [`docs/spec/04-notifications.md` §2.3].
    #[must_use]
    pub(crate) fn new(field_count: usize) -> Self {
        Self {
            fields: vec![FieldSlot::Unset; field_count],
        }
    }

    /// The number of fields in the schema this state was built for.
    #[must_use]
    #[inline]
    pub(crate) fn field_count(&self) -> usize {
        self.fields.len()
    }

    /// The current value of field `index` (0-based).
    ///
    /// The two levels of [`Option`] are distinct questions:
    ///
    /// - `None` — there is no such field in the schema.
    /// - `Some(None)` — the field is **null**, either because an update set it
    ///   with the `#` marker or because no update has set it yet
    ///   [`docs/spec/04-notifications.md` §2.2].
    /// - `Some(Some(s))` — the field holds the string `s`, which may be the
    ///   empty string. **Empty is not null**: `$` and `#` are different markers
    ///   with different meanings [`docs/spec/04-notifications.md` §2.2], and
    ///   the distinction survives all the way to the caller.
    #[must_use]
    pub(crate) fn field(&self, index: usize) -> Option<Option<&str>> {
        match self.fields.get(index)? {
            FieldSlot::Unset | FieldSlot::Null => Some(None),
            FieldSlot::Value(value) => Some(Some(value.as_str())),
        }
    }

    /// Applies the raw, still-undecoded third argument of a `U` notification.
    ///
    /// This is the normative algorithm of
    /// [`docs/spec/04-notifications.md` §2.3], in the order the spec states it:
    /// split on `|` first, test `#`/`$` as whole tokens, test `^` as a prefix,
    /// and percent-decode last — only actual content and diff payloads.
    ///
    /// # Errors
    ///
    /// Returns [`ProtocolError::FieldValue`] for any malformed value list: a
    /// token count that does not match the schema, a `^` directive that is
    /// neither a count nor a known diff tag, a diff that does not apply, or a
    /// bad percent-escape. On error **no field is modified** — the update is
    /// staged and committed only once the whole list has been decoded, so a
    /// rejected update cannot leave the item half-applied.
    pub(crate) fn apply(&mut self, raw_values: &str) -> Result<UpdateOutcome, ProtocolError> {
        let field_count = self.fields.len();
        let mut pointer: usize = 0;
        let mut staged: Vec<(usize, FieldSlot)> = Vec::new();
        let mut changed: Vec<usize> = Vec::new();

        // §2.3: "Look for the next pipe from left to right". A `|` belonging to
        // a value arrives percent-encoded as `%7C` [§2.2], so a plain split on
        // the literal character never cuts a value in half.
        for token in raw_values.split('|') {
            // §2.3: the algorithm stops when "there are no more fields in the
            // schema", so a token arriving with the pointer already past the
            // end has no field to apply to.
            if pointer >= field_count {
                return Err(field_value_error(format!(
                    "value list carries more tokens than the {field_count} fields of the schema"
                )));
            }

            // §2.3: "If its value is empty, the pointed field should be left
            // unchanged and the pointer moved to the next field."
            if token.is_empty() {
                // Cannot overflow: `pointer < field_count <= usize::MAX`.
                pointer += 1;
                continue;
            }

            // §2.3: the `#` and `$` tests are for a token that *corresponds to*
            // a single hash / dollar sign — a whole-token match, not a prefix.
            // `#abc` is therefore actual content [§2.3 invariant 2].
            if token == "#" {
                staged.push((pointer, FieldSlot::Null));
                changed.push(pointer);
                pointer += 1;
                continue;
            }
            if token == "$" {
                staged.push((pointer, FieldSlot::Value(String::new())));
                changed.push(pointer);
                pointer += 1;
                continue;
            }

            // §2.3: "if its value begins with a caret" — this one *is* a prefix
            // test [§2.3 invariant 3].
            if let Some(rest) = token.strip_prefix('^') {
                let mut rest_chars = rest.chars();
                let Some(marker) = rest_chars.next() else {
                    // SPEC-AMBIGUITY (caret-without-marker): §2.3 says to
                    // "check if the following character is a letter or a digit"
                    // but does not say what a bare `^` means. Choice: reject.
                    // Guessing "unchanged" or "literal caret" would silently
                    // desynchronise the field pointer for the rest of the line.
                    return Err(field_value_error(
                        "value list token `^` has no directive after the caret",
                    ));
                };

                if marker.is_ascii_digit() {
                    // §2.3, case digit: "take the substring following the caret
                    // and convert it to an integer number; for the
                    // corresponding count, leave the fields unchanged and move
                    // the pointer forward". §2.3 invariant 6: the count is
                    // inclusive of the field currently pointed at, so `^3`
                    // covers the pointed field and the following two.
                    let count = parse_unchanged_run(rest)?;
                    let end = pointer.checked_add(count).ok_or_else(|| {
                        field_value_error(format!(
                            "unchanged-field run `^{rest}` overflows the field pointer"
                        ))
                    })?;
                    // SPEC-AMBIGUITY (run-overruns-schema): §2.3 does not say
                    // what to do when a `^N` count reaches past the last field
                    // of the schema. Choice: reject. Clamping would accept a
                    // line the server could not have meant, and the count is
                    // exactly the kind of value a schema-width mismatch
                    // corrupts, so it is worth surfacing.
                    if end > field_count {
                        return Err(field_value_error(format!(
                            "unchanged-field run `^{rest}` reaches field {end} of a \
                             {field_count}-field schema"
                        )));
                    }
                    pointer = end;
                    continue;
                }

                if marker.is_ascii_alphabetic() {
                    // §2.3, case letter: "take the substring following the
                    // letter and decode any percent-encoding", then apply it as
                    // a difference in the format named by the letter [§2.2].
                    let payload = percent_decode(rest_chars.as_str())?;

                    // §2.3: "this case never happens if the value of the
                    // pointed field is currently null".
                    // SPEC-AMBIGUITY (diff-onto-non-value): the spec rules out
                    // a diff onto a *null* field but says nothing about a field
                    // that has never been set, nor about one currently holding
                    // the empty string. Choice: never-set is rejected together
                    // with null — neither has a base character sequence to diff
                    // against — while the empty string is accepted as a base of
                    // zero characters, which is a perfectly well-defined input
                    // to both diff formats.
                    let base = match self.fields.get(pointer) {
                        Some(FieldSlot::Value(value)) => value.as_str(),
                        Some(FieldSlot::Null) => {
                            return Err(field_value_error(format!(
                                "`^{marker}` diff applies to field {pointer}, which is null"
                            )));
                        }
                        Some(FieldSlot::Unset) | None => {
                            return Err(field_value_error(format!(
                                "`^{marker}` diff applies to field {pointer}, which has never \
                                 been set"
                            )));
                        }
                    };

                    // ADR-0004: dispatch through the same registry the
                    // `LS_supported_diffs` advertisement is derived from, so a
                    // tag can never be advertised without being decodable. An
                    // unadvertised tag is a typed error, never a passthrough.
                    let decoder = DIFF_DECODERS
                        .iter()
                        .find(|decoder| decoder.tag == marker)
                        .ok_or_else(|| {
                            field_value_error(format!(
                                "unsupported diff format `{marker}`; this client advertised `{}`",
                                supported_diffs()
                            ))
                        })?;

                    let value = (decoder.apply)(base, payload.as_ref())?;
                    staged.push((pointer, FieldSlot::Value(value)));
                    changed.push(pointer);
                    pointer += 1;
                    continue;
                }

                // SPEC-AMBIGUITY (caret-marker-not-letter-or-digit): §2.3
                // enumerates only the digit and letter cases. Choice: reject.
                // A caret at the start of actual content arrives
                // percent-encoded [§2.2], so a token that begins with a literal
                // caret and continues with anything else cannot be content
                // either — it is malformed, and treating it as content would
                // hand the caller a value that is wrong undetectably.
                return Err(field_value_error(format!(
                    "value list token `{token}` has `{marker}` after the caret, which is neither \
                     an ASCII digit nor a diff format tag"
                )));
            }

            // §2.3: "Otherwise, the value is an actual content: decode any
            // percent-encoding and set the pointed field to the decoded value".
            // §2.3 invariant 4: decoding happens here, last, and only here —
            // which is why the `#`/`$`/`^` tests above ran against the raw
            // token and cannot be fooled by a `%23` that decodes to `#`.
            let decoded = percent_decode(token)?;
            staged.push((pointer, FieldSlot::Value(decoded.into_owned())));
            changed.push(pointer);
            pointer += 1;
        }

        // SPEC-AMBIGUITY (line-shorter-than-schema): §2.3 terminates when the
        // schema is exhausted and every worked example covers the full schema;
        // the spec never says whether a short line means "the rest is
        // unchanged" or is an error. Choice: reject. The protocol already has a
        // compact way to say "all remaining fields are unchanged" — the `^N`
        // run, used for exactly that in the §2.5 example `20:06:10|3.05|0.32|^7`
        // — so a server with nothing left to say still sends a token for every
        // field. A short line therefore signals a schema-width disagreement,
        // which must be visible rather than silently absorbed.
        if pointer < field_count {
            return Err(field_value_error(format!(
                "value list ended after {pointer} of the {field_count} fields of the schema"
            )));
        }

        for (index, slot) in staged {
            if let Some(target) = self.fields.get_mut(index) {
                *target = slot;
            }
        }

        Ok(UpdateOutcome { changed })
    }
}

/// Parses the count of a `^N` unchanged-field run.
///
/// # Errors
///
/// Returns [`ProtocolError::FieldValue`] if the substring after the caret is
/// not a plain run of ASCII digits, or if it does not fit a [`usize`].
fn parse_unchanged_run(digits: &str) -> Result<usize, ProtocolError> {
    // SPEC-AMBIGUITY (unchanged-run-number-format): §2.3 says only "convert it
    // to an integer number" and constrains neither sign, radix, leading zeros
    // nor magnitude. Choice: accept exactly a non-empty run of ASCII digits —
    // leading zeros included, since they denote the same number — and reject
    // everything else. In particular `^+3` and `^3x` are rejected rather than
    // being read as `3`, because a permissive parse of a corrupt token silently
    // desynchronises the field pointer.
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(field_value_error(format!(
            "unchanged-field count `{digits}` is not a run of ASCII digits"
        )));
    }
    digits
        .parse::<usize>()
        .map_err(|_| field_value_error(format!("unchanged-field count `{digits}` is out of range")))
}

// ---------------------------------------------------------------------------
// Update outcome
// ---------------------------------------------------------------------------

/// Which fields a single `U` notification actually changed.
///
/// "Changed" means the update **carried an explicit token** for the field —
/// content, `#` or `$` — matching the *Changed* / *Unchanged* rows of the
/// specification's own worked examples
/// [`docs/spec/04-notifications.md` §2.5]. It is not a value comparison: the
/// §2.5 example `20:04:16|3.02|-0.65|||3.01|3.02|||$` sets `status` to the
/// empty string when it is already the empty string, and the spec's table marks
/// that field *Changed*.
///
/// A field left out by an empty token or skipped by a `^N` run is not reported,
/// since both mean "unchanged" [`docs/spec/04-notifications.md` §2.2].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct UpdateOutcome {
    /// 0-based field indices, in ascending order — the field pointer only ever
    /// moves forward [`docs/spec/04-notifications.md` §2.3].
    changed: Vec<usize>,
}

impl UpdateOutcome {
    /// The 0-based indices of the fields this update changed, ascending.
    #[must_use]
    #[inline]
    pub(crate) fn changed_fields(&self) -> &[usize] {
        &self.changed
    }

    /// Whether this update carried a value for field `index` (0-based).
    #[must_use]
    #[inline]
    pub(crate) fn is_field_changed(&self, index: usize) -> bool {
        self.changed.contains(&index)
    }

    /// How many fields this update changed.
    #[must_use]
    #[inline]
    pub(crate) fn changed_count(&self) -> usize {
        self.changed.len()
    }

    /// Whether this update changed nothing at all.
    #[must_use]
    #[inline]
    pub(crate) fn is_empty(&self) -> bool {
        self.changed.is_empty()
    }
}

// ---------------------------------------------------------------------------
// Diff decoder registry (ADR-0004)
// ---------------------------------------------------------------------------

/// One compiled-in diff format: its wire tag and the function that applies it.
struct DiffDecoder {
    /// The letter that follows the caret on the wire
    /// [`docs/spec/04-notifications.md` §2.2].
    tag: char,
    /// Applies a percent-decoded diff payload to the field's previous value.
    apply: fn(base: &str, payload: &str) -> Result<String, ProtocolError>,
}

/// TLCP-diff, the custom format of Appendix D
/// [`docs/spec/04-notifications.md` §2.7, spec pp.98-99].
const TLCP_DIFF_DECODER: DiffDecoder = DiffDecoder {
    tag: 'T',
    apply: apply_tlcp_diff,
};

/// JSON Patch, RFC 6902 [`docs/spec/04-notifications.md` §2.2].
#[cfg(feature = "json-patch")]
const JSON_PATCH_DECODER: DiffDecoder = DiffDecoder {
    tag: 'P',
    apply: apply_json_patch,
};

/// The decoders compiled into this build — the single source of truth ADR-0004
/// requires. Both the wire dispatch in [`ItemState::apply`] and the
/// [`supported_diffs`] advertisement read from this one list, so the set the
/// client promises the server and the set it can actually decode cannot drift.
#[cfg(feature = "json-patch")]
const DIFF_DECODERS: &[DiffDecoder] = &[TLCP_DIFF_DECODER, JSON_PATCH_DECODER];

/// The decoders compiled into this build — see the `json-patch` variant above.
#[cfg(not(feature = "json-patch"))]
const DIFF_DECODERS: &[DiffDecoder] = &[TLCP_DIFF_DECODER];

/// Cache for the derived advertisement; the derivation is pure, so computing it
/// once is safe.
static SUPPORTED_DIFFS: OnceLock<String> = OnceLock::new();

/// The value of the `LS_supported_diffs` session-creation parameter for this
/// build.
///
/// The parameter is "a (possibly empty) comma-separated list of format tags, as
/// specified in *Real-Time Update*" [`docs/spec/03-requests.md` §2.1], and it
/// restricts the set of diff algorithms the server may choose from. Per
/// ADR-0004 this value is **derived from [`DIFF_DECODERS`]**, never written by
/// hand: it is `"T"` by default and `"T,P"` when the `json-patch` feature
/// compiles the RFC 6902 decoder in. Advertising a format that is not compiled
/// in would entitle the server to send values this client cannot decode.
#[must_use]
pub(crate) fn supported_diffs() -> &'static str {
    SUPPORTED_DIFFS
        .get_or_init(|| {
            let mut value = String::new();
            for decoder in DIFF_DECODERS {
                if !value.is_empty() {
                    value.push(',');
                }
                value.push(decoder.tag);
            }
            value
        })
        .as_str()
}

// ---------------------------------------------------------------------------
// `^T` — TLCP-diff (Appendix D, spec pp.98-99)
// ---------------------------------------------------------------------------

/// One TLCP-diff instruction [Appendix D, p.98].
///
/// `ADD(N, STR)` is modelled by `STR` alone: `N` is the count of characters
/// that follow the encoded integer, so it is exactly `STR`'s character count
/// and storing it separately could only introduce a disagreement.
#[derive(Debug, Clone, PartialEq, Eq)]
enum DiffInstruction {
    /// `COPY(N)`: append `N` characters of the base starting at `basePos`, then
    /// advance `basePos` by `N` [Appendix D, p.98].
    Copy(usize),
    /// `ADD(N, STR)`: append `STR` to the result. Does **not** advance
    /// `basePos` [Appendix D, p.98].
    Add(String),
    /// `DEL(N)`: advance `basePos` by `N` [Appendix D, p.98].
    Del(usize),
}

/// Applies a TLCP-diff payload to a base value.
///
/// # Errors
///
/// Returns [`ProtocolError::FieldValue`] if the payload is not a well-formed
/// TLCP-diff or if its instructions do not fit the base value.
fn apply_tlcp_diff(base: &str, payload: &str) -> Result<String, ProtocolError> {
    // Appendix D, p.98: a TLCP-diff and the base it applies to are guaranteed
    // to contain no character whose UTF-16 representation needs a surrogate
    // pair. Under that guarantee one Unicode scalar value is one UTF-16 code
    // unit, so the appendix's "characters" — which are UTF-16 code units,
    // the format being defined for a Java implementation — coincide exactly
    // with Rust `char`s. Byte offsets do *not* coincide: a two-byte UTF-8
    // character would make every count wrong. Both the base and the payload are
    // therefore materialised as `Vec<char>` and every position and count in
    // this function is a `char` index. Should a server violate the guarantee
    // and send an astral character, this decoder counts it as one unit where a
    // UTF-16 implementation counts two; that is a server-side protocol
    // violation, and counting scalar values is the only self-consistent reading
    // available to a UTF-8 language.
    let base_chars: Vec<char> = base.chars().collect();
    let instructions = parse_tlcp_diff(payload)?;
    apply_tlcp_instructions(&base_chars, &instructions)
}

/// Parses a TLCP-diff payload into its instruction list [Appendix D, p.98].
///
/// The section type cycle is fixed and positional — `COPY, ADD, DEL, COPY, …`,
/// starting at `COPY` — and the sequence may end after any section
/// [Appendix D, p.98]. Sequential parsing is possible because an encoded
/// integer ends at its first lowercase letter, so an `ADD` section knows how
/// many characters to consume as soon as its integer is read.
///
/// # Errors
///
/// Returns [`ProtocolError::FieldValue`] for an empty payload, a truncated or
/// malformed encoded integer, or an `ADD` whose declared length exceeds what is
/// left of the payload.
fn parse_tlcp_diff(payload: &str) -> Result<Vec<DiffInstruction>, ProtocolError> {
    let chars: Vec<char> = payload.chars().collect();

    // Appendix D, p.98: a TLCP-diff is "the concatenation of one or more
    // sections", so an empty payload has no valid reading.
    if chars.is_empty() {
        return Err(field_value_error("TLCP-diff payload is empty"));
    }

    let mut instructions = Vec::new();
    let mut position: usize = 0;
    // 0 = COPY, 1 = ADD, 2 = DEL — the fixed cycle of Appendix D, p.98.
    let mut section: u8 = 0;

    while position < chars.len() {
        let count = decode_encoded_integer(&chars, &mut position)?;
        let instruction = match section {
            0 => DiffInstruction::Copy(count),
            1 => {
                let end = position.checked_add(count).ok_or_else(|| {
                    field_value_error("TLCP-diff ADD length overflows the payload cursor")
                })?;
                let text = chars.get(position..end).ok_or_else(|| {
                    field_value_error(format!(
                        "TLCP-diff ADD at position {position} declares {count} characters but the \
                         payload is only {} characters long",
                        chars.len()
                    ))
                })?;
                position = end;
                DiffInstruction::Add(text.iter().collect())
            }
            _ => DiffInstruction::Del(count),
        };
        instructions.push(instruction);
        section = if section >= 2 { 0 } else { section + 1 };
    }

    Ok(instructions)
}

/// Decodes one encoded integer starting at `position`, advancing it past the
/// terminating lowercase letter [Appendix D, p.98].
///
/// # Errors
///
/// Returns [`ProtocolError::FieldValue`] if the payload ends mid-integer, if a
/// non-ASCII-letter is encountered, or if the value overflows a [`usize`].
fn decode_encoded_integer(chars: &[char], position: &mut usize) -> Result<usize, ProtocolError> {
    // Appendix D, p.98: "a sequence of one or more letters, such that the last
    // one is a small letter whereas any other preceding ones are capital
    // letters. This letter sequence forms a radix-26 representation of the
    // number, in which the letters weight progressively."
    //
    // SPEC-AMBIGUITY (encoded-integer-letter-weight): the appendix's prose adds
    // "each letter weights as its ASCII value minus the ASCII value of 'a'",
    // which contradicts the appendix's own example table. Under the prose,
    // `Ad` = 0·26 + 3 = 3 and `Aa` = 0·26 + 0 = 0; the table states `Ad` is
    // COPY(29) and `Aa` is COPY(26) [Appendix D, p.98]. The only rule that
    // satisfies both table entries is that each non-final (capital) letter
    // contributes `(value + 1)` at its radix weight while the final (lowercase)
    // letter contributes `value`: `Aa` = (0+1)·26 + 0 = 26 and
    // `Ad` = (0+1)·26 + 3 = 29. Choice: implement what fits the examples, since
    // the examples are the only executable statement of the format and a
    // decoder built on the prose would mis-decode the appendix's own table.
    // Single-letter integers — every other example in the appendix — are
    // identical under both readings, so this affects only multi-letter
    // integers, which the specification attests with exactly one data point.
    let mut accumulator: usize = 0;

    loop {
        let letter = *chars.get(*position).ok_or_else(|| {
            field_value_error(
                "TLCP-diff encoded integer is truncated: no terminating lowercase letter",
            )
        })?;
        // Cannot overflow: `*position < chars.len() <= usize::MAX`.
        *position += 1;

        if letter.is_ascii_uppercase() {
            // Non-final letter: contributes (value + 1) at this radix weight.
            let value = alphabet_index(letter, 'A')?;
            accumulator = accumulator
                .checked_mul(26)
                .and_then(|scaled| scaled.checked_add(value.checked_add(1)?))
                .ok_or_else(|| field_value_error("TLCP-diff encoded integer is out of range"))?;
            continue;
        }

        if letter.is_ascii_lowercase() {
            // Final letter: contributes its own value and terminates the
            // integer [Appendix D, p.98].
            let value = alphabet_index(letter, 'a')?;
            return accumulator
                .checked_mul(26)
                .and_then(|scaled| scaled.checked_add(value))
                .ok_or_else(|| field_value_error("TLCP-diff encoded integer is out of range"));
        }

        return Err(field_value_error(format!(
            "TLCP-diff encoded integer contains `{letter}`, which is not an ASCII letter"
        )));
    }
}

/// The 0-based position of `letter` in the alphabet starting at `first`.
///
/// # Errors
///
/// Returns [`ProtocolError::FieldValue`] if `letter` sorts before `first`,
/// which the callers' `is_ascii_uppercase` / `is_ascii_lowercase` guards
/// already exclude.
fn alphabet_index(letter: char, first: char) -> Result<usize, ProtocolError> {
    u32::from(letter)
        .checked_sub(u32::from(first))
        .and_then(|offset| usize::try_from(offset).ok())
        .ok_or_else(|| {
            field_value_error(format!(
                "TLCP-diff encoded integer contains `{letter}`, which is not an ASCII letter"
            ))
        })
}

/// Runs an instruction list against a base value [Appendix D, pp.98-99].
///
/// # Errors
///
/// Returns [`ProtocolError::FieldValue`] if a `COPY` reaches past the end of
/// the base value.
fn apply_tlcp_instructions(
    base: &[char],
    instructions: &[DiffInstruction],
) -> Result<String, ProtocolError> {
    // Appendix D, pp.98-99: "set result as an empty character sequence; set
    // basePos as 0; for each instruction …; return result".
    let mut result = String::new();
    let mut base_pos: usize = 0;

    for instruction in instructions {
        match instruction {
            DiffInstruction::Copy(count) => {
                // "append to result N characters taken from base starting at
                // position basePos; increment basePos by N" [p.99].
                let end = base_pos
                    .checked_add(*count)
                    .ok_or_else(|| field_value_error("TLCP-diff COPY overflows the base cursor"))?;
                // SPEC-AMBIGUITY (diff-runs-past-base): the appendix does not
                // define behaviour for a COPY that reaches past the end of the
                // base value [Appendix D, pp.98-99]. Choice: reject. Copying
                // fewer characters than asked would produce a value that is
                // wrong in a way the caller cannot detect, whereas the base is
                // known exactly and the mismatch can only mean the diff was
                // built against a different base than the one held here.
                let slice = base.get(base_pos..end).ok_or_else(|| {
                    field_value_error(format!(
                        "TLCP-diff COPY({count}) at position {base_pos} runs past the end of the \
                         {}-character base value",
                        base.len()
                    ))
                })?;
                result.extend(slice.iter());
                base_pos = end;
            }
            DiffInstruction::Add(text) => {
                // "append STR to result" — and, notably, `basePos` does not
                // move [Appendix D, p.99].
                result.push_str(text);
            }
            DiffInstruction::Del(count) => {
                // "increment basePos by N" [Appendix D, p.99].
                // SPEC-AMBIGUITY (del-past-base): the appendix places no bound
                // on the resulting `basePos`. Choice: follow the algorithm
                // literally and let `basePos` move past the end, guarding only
                // against integer overflow. A trailing DEL that overshoots is
                // harmless — the sequence may end after any section [p.98] and
                // nothing reads `basePos` again — while a DEL that overshoots
                // *and* matters is caught by the COPY that follows it.
                base_pos = base_pos
                    .checked_add(*count)
                    .ok_or_else(|| field_value_error("TLCP-diff DEL overflows the base cursor"))?;
            }
        }
    }

    // Appendix D, p.99: the result is exactly what the instructions produce.
    // The second application example — base `foobar` with only COPY(3),
    // yielding `foo` — shows there is no implicit trailing copy of the base.
    Ok(result)
}

// ---------------------------------------------------------------------------
// `^P` — JSON Patch, RFC 6902 (optional)
// ---------------------------------------------------------------------------

/// Applies an RFC 6902 JSON Patch to a base value
/// [`docs/spec/04-notifications.md` §2.2].
///
/// The specification guarantees that a `^P` diff is only ever sent when both
/// the previous and the new value are valid JSON representations
/// [`docs/spec/04-notifications.md` §2.2].
///
/// # Errors
///
/// Returns [`ProtocolError::FieldValue`] if the base is not valid JSON, if the
/// payload is not a valid patch document, or if the patch fails to apply.
#[cfg(feature = "json-patch")]
fn apply_json_patch(base: &str, payload: &str) -> Result<String, ProtocolError> {
    // SPEC-AMBIGUITY (json-patch-reserialisation): the spec prints the value
    // after a patch with the whitespace of the original snapshot removed
    // [`docs/spec/04-notifications.md` §2.6] but never states that the client
    // must re-serialise, nor in what canonical form. Choice: parse, patch and
    // re-serialise compactly. Applying a patch requires a parsed document
    // anyway, and compact serialisation is what the spec's own worked example
    // shows. The consequence — insignificant JSON whitespace, and the ordering
    // of object members, are not preserved across a patch — is invisible to any
    // caller that reads the value as JSON and unavoidable for one that compares
    // it byte for byte.
    let mut document: serde_json::Value = serde_json::from_str(base).map_err(|error| {
        field_value_error(format!(
            "`^P` diff applies to a field whose value is not valid JSON: {error}"
        ))
    })?;

    let patch: json_patch::Patch = serde_json::from_str(payload).map_err(|error| {
        field_value_error(format!("`^P` diff is not a valid RFC 6902 patch: {error}"))
    })?;

    // SPEC-AMBIGUITY (json-patch-application-failure): the spec does not define
    // client behaviour when a received patch fails to apply
    // [`docs/spec/04-notifications.md` §2.6]. Choice: a typed error. The
    // alternative — keeping the previous value, or the partially patched one —
    // leaves the client silently out of step with the server's view of the
    // field, which is exactly the failure this crate exists to make visible.
    json_patch::patch(&mut document, &patch)
        .map_err(|error| field_value_error(format!("`^P` diff failed to apply: {error}")))?;

    Ok(document.to_string())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    /// The §2.5 schema: `timestamp`, `price`, `change`, `minimum`, `maximum`,
    /// `bid`, `ask`, `open`, `close`, `status`
    /// [`docs/spec/04-notifications.md` §2.5, p.51].
    const QUOTE_FIELDS: usize = 10;

    /// The six §2.5 value lists, in the order the specification presents them
    /// as a stream [`docs/spec/04-notifications.md` §2.5, pp.51-52].
    const EX1: &str = "20:00:33|3.04|0.0|2.41|3.67|3.03|3.04|#|#|$";
    const EX2: &str = "20:00:54|3.07|0.98|||3.06|3.07|||Suspended";
    const EX3: &str = "20:04:16|3.02|-0.65|||3.01|3.02|||$";
    const EX4: &str = "20:04:40|^4|3.02|3.03|||";
    const EX5: &str = "20:06:10|3.05|0.32|^7";
    const EX6: &str = "20:06:49|3.08|1.31|||3.08|3.09|||";

    /// Replays a sequence of value lists onto a fresh state.
    fn quote_state_after(lines: &[&str]) -> ItemState {
        let mut state = ItemState::new(QUOTE_FIELDS);
        for line in lines {
            assert!(state.apply(line).is_ok(), "failed to apply `{line}`");
        }
        state
    }

    /// Asserts the whole schema at once. `None` means null.
    fn assert_quote(state: &ItemState, expected: [Option<&str>; QUOTE_FIELDS]) {
        for (index, want) in expected.iter().enumerate() {
            assert_eq!(state.field(index), Some(*want), "field {index}");
        }
        assert_eq!(state.field(QUOTE_FIELDS), None, "past end of schema");
    }

    // -- §2.5 worked examples ------------------------------------------------

    #[test]
    fn test_apply_example_1_snapshot_sets_every_field_p51() {
        let mut state = ItemState::new(QUOTE_FIELDS);
        let outcome = state.apply(EX1).unwrap();

        assert_quote(
            &state,
            [
                Some("20:00:33"),
                Some("3.04"),
                Some("0.0"),
                Some("2.41"),
                Some("3.67"),
                Some("3.03"),
                Some("3.04"),
                None,
                None,
                Some(""),
            ],
        );
        // "The initial update (the snapshot) carries information for all the
        // fields" [p.51]: every field is marked Changed in the spec's table.
        assert_eq!(outcome.changed_fields(), &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9]);
    }

    #[test]
    fn test_apply_example_2_four_unchanged_fields_p51() {
        let mut state = quote_state_after(&[EX1]);
        let outcome = state.apply(EX2).unwrap();

        assert_quote(
            &state,
            [
                Some("20:00:54"),
                Some("3.07"),
                Some("0.98"),
                Some("2.41"),
                Some("3.67"),
                Some("3.06"),
                Some("3.07"),
                None,
                None,
                Some("Suspended"),
            ],
        );
        // "This update contains 4 unchanged fields: minimum, maximum, open and
        // close" [p.52] — indices 3, 4, 7 and 8.
        assert_eq!(outcome.changed_fields(), &[0, 1, 2, 5, 6, 9]);
    }

    #[test]
    fn test_apply_example_3_dollar_resets_status_to_empty_p52() {
        let mut state = quote_state_after(&[EX1, EX2]);
        let outcome = state.apply(EX3).unwrap();

        assert_quote(
            &state,
            [
                Some("20:04:16"),
                Some("3.02"),
                Some("-0.65"),
                Some("2.41"),
                Some("3.67"),
                Some("3.01"),
                Some("3.02"),
                None,
                None,
                Some(""),
            ],
        );
        // "Moreover, it sets he status field to its initial empty value" [p.52]
        // — the spec's table marks `status` Changed even though it goes from
        // empty to empty, which is why `changed` is "carried a token", not
        // "value differs".
        assert_eq!(outcome.changed_fields(), &[0, 1, 2, 5, 6, 9]);
        assert!(outcome.is_field_changed(9));
    }

    #[test]
    fn test_apply_example_4_caret_run_of_four_p52() {
        let mut state = quote_state_after(&[EX1, EX2, EX3]);
        let outcome = state.apply(EX4).unwrap();

        assert_quote(
            &state,
            [
                Some("20:04:40"),
                Some("3.02"),
                Some("-0.65"),
                Some("2.41"),
                Some("3.67"),
                Some("3.02"),
                Some("3.03"),
                None,
                None,
                Some(""),
            ],
        );
        // "the special syntax for multiple contiguous unchanged fields: price,
        // change, minimum and maximum" [p.52] — `^4` covers indices 1..=4,
        // inclusive of the pointed field, and `bid`/`ask` follow it.
        assert_eq!(outcome.changed_fields(), &[0, 5, 6]);
    }

    #[test]
    fn test_apply_example_5_caret_run_reaching_end_of_schema_p52() {
        let mut state = quote_state_after(&[EX1, EX2, EX3, EX4]);
        let outcome = state.apply(EX5).unwrap();

        // The structural claim of the example is that `^7` covers the last
        // seven fields [p.52].
        //
        // SPEC-AMBIGUITY (example-5-inconsistent): the printed table gives
        // `price` = 3.03 and `change` = -0.32 while the wire line carries 3.05
        // and 0.32 [p.52]. This fixture asserts the wire line, which is the
        // artefact a decoder is defined over; the table's values cannot be
        // produced by any reading of the line.
        assert_quote(
            &state,
            [
                Some("20:06:10"),
                Some("3.05"),
                Some("0.32"),
                Some("2.41"),
                Some("3.67"),
                Some("3.02"),
                Some("3.03"),
                None,
                None,
                Some(""),
            ],
        );
        assert_eq!(outcome.changed_fields(), &[0, 1, 2]);
    }

    #[test]
    fn test_apply_example_6_final_update_p52() {
        let mut state = quote_state_after(&[EX1, EX2, EX3, EX4, EX5]);
        let outcome = state.apply(EX6).unwrap();

        assert_quote(
            &state,
            [
                Some("20:06:49"),
                Some("3.08"),
                Some("1.31"),
                Some("2.41"),
                Some("3.67"),
                Some("3.08"),
                Some("3.09"),
                None,
                None,
                Some(""),
            ],
        );
        // "once more 4 unchanged fields: minimum, maximum, open and close"
        // [p.52]; `status` is unchanged too, carried by the trailing empty
        // token.
        assert_eq!(outcome.changed_fields(), &[0, 1, 2, 5, 6]);
    }

    // -- §2.6 worked examples (diffs) ---------------------------------------

    /// The §2.6 schema: `timestamp`, `text`
    /// [`docs/spec/04-notifications.md` §2.6, p.52].
    const DIFF_SNAPSHOT: &str =
        r#"20:00:33|{ "text": "aa%7Cbb", "attributes": [ { "font": "courier" }, "..." ] }"#;
    const DIFF_SNAPSHOT_TEXT: &str =
        r#"{ "text": "aa|bb", "attributes": [ { "font": "courier" }, "..." ] }"#;

    #[test]
    fn test_apply_diff_example_1_snapshot_percent_decodes_pipe_p52() {
        let mut state = ItemState::new(2);
        let outcome = state.apply(DIFF_SNAPSHOT).unwrap();

        assert_eq!(state.field(0), Some(Some("20:00:33")));
        // `%7C` inside a value decodes to `|` and does not split the list
        // [`docs/spec/04-notifications.md` §2.2, §2.3 invariant 1, p.52].
        assert_eq!(state.field(1), Some(Some(DIFF_SNAPSHOT_TEXT)));
        assert_eq!(outcome.changed_fields(), &[0, 1]);
    }

    #[test]
    fn test_apply_diff_example_3_empty_token_leaves_text_unchanged_p53() {
        let mut state = ItemState::new(2);
        state.apply(DIFF_SNAPSHOT).unwrap();

        // "In the final update, the text field is unchanged; this is carried,
        // as usual, by an empty field" [p.53].
        let outcome = state.apply("20:04:16|").unwrap();
        assert_eq!(state.field(0), Some(Some("20:04:16")));
        assert_eq!(state.field(1), Some(Some(DIFF_SNAPSHOT_TEXT)));
        assert_eq!(outcome.changed_fields(), &[0]);
    }

    #[cfg(feature = "json-patch")]
    #[test]
    fn test_apply_diff_example_2_json_patch_p53() {
        let mut state = ItemState::new(2);
        state.apply(DIFF_SNAPSHOT).unwrap();

        let line = concat!(
            r#"20:00:54|^P[{"op":"replace","path":"/text","value":"aa%7Cbb%7Ccc"},"#,
            r#"{"op":"add","path":"/attributes/1","value":"bold"}]"#
        );
        let outcome = state.apply(line).unwrap();

        assert_eq!(state.field(0), Some(Some("20:00:54")));
        assert_eq!(outcome.changed_fields(), &[0, 1]);

        // The spec's "actual value" row is
        // {"text":"aa|bb|cc","attributes":[{"font":"courier"},"bold","..."]}
        // [p.53]. See SPEC-AMBIGUITY (json-patch-reserialisation): the byte
        // form after re-serialisation is not defined by the spec, so the
        // assertion is on the JSON document, not on its spelling.
        let got: serde_json::Value =
            serde_json::from_str(state.field(1).unwrap().unwrap()).unwrap();
        let want: serde_json::Value = serde_json::from_str(
            r#"{"text":"aa|bb|cc","attributes":[{"font":"courier"},"bold","..."]}"#,
        )
        .unwrap();
        assert_eq!(got, want);
    }

    #[cfg(feature = "json-patch")]
    #[test]
    fn test_apply_json_patch_that_does_not_apply_is_an_error() {
        let mut state = ItemState::new(1);
        state.apply(r#"{"a":1}"#).unwrap();
        // `remove` of a path that is not there cannot be applied.
        let result = state.apply(r#"^P[{"op":"remove","path":"/nope"}]"#);
        assert!(matches!(result, Err(ProtocolError::FieldValue { .. })));
        // The rejected update left the field alone.
        assert_eq!(state.field(0), Some(Some(r#"{"a":1}"#)));
    }

    #[cfg(feature = "json-patch")]
    #[test]
    fn test_apply_json_patch_onto_non_json_base_is_an_error() {
        let mut state = ItemState::new(1);
        state.apply("not json").unwrap();
        let result = state.apply(r#"^P[{"op":"replace","path":"/a","value":1}]"#);
        assert!(matches!(result, Err(ProtocolError::FieldValue { .. })));
    }

    // -- Appendix D: value -> instruction list [p.98] ------------------------

    #[test]
    fn test_parse_tlcp_diff_appendix_d_section_table_p98() {
        use DiffInstruction::{Add, Copy, Del};

        assert_eq!(parse_tlcp_diff("d").unwrap(), vec![Copy(3)]);
        assert_eq!(
            parse_tlcp_diff("bdzap").unwrap(),
            vec![Copy(1), Add("zap".to_owned())]
        );
        assert_eq!(
            parse_tlcp_diff("bdzapcd").unwrap(),
            vec![Copy(1), Add("zap".to_owned()), Del(2), Copy(3)]
        );
        assert_eq!(
            parse_tlcp_diff("adzapad").unwrap(),
            vec![Copy(0), Add("zap".to_owned()), Del(0), Copy(3)]
        );
        // The multi-letter row. See SPEC-AMBIGUITY
        // (encoded-integer-letter-weight): `Ad` is COPY(29) and `Aa` is
        // COPY(26) per the table, which the prose's own weighting rule cannot
        // produce.
        assert_eq!(
            parse_tlcp_diff("AdacAa").unwrap(),
            vec![Copy(29), Add(String::new()), Del(2), Copy(26)]
        );
    }

    #[test]
    fn test_decode_encoded_integer_matches_appendix_d_examples_p98() {
        fn decode(text: &str) -> usize {
            let chars: Vec<char> = text.chars().collect();
            let mut position = 0;
            let value = decode_encoded_integer(&chars, &mut position).unwrap();
            assert_eq!(position, chars.len(), "consumed the whole integer");
            value
        }

        assert_eq!(decode("a"), 0);
        assert_eq!(decode("b"), 1);
        assert_eq!(decode("c"), 2);
        assert_eq!(decode("d"), 3);
        assert_eq!(decode("z"), 25);
        assert_eq!(decode("Aa"), 26);
        assert_eq!(decode("Ad"), 29);
        // Extrapolated from the same rule: (0+1)·26² + (0+1)·26 + 0.
        assert_eq!(decode("AAa"), 702);
    }

    // -- Appendix D: base + instructions -> result [p.99] --------------------

    #[test]
    fn test_apply_tlcp_instructions_appendix_d_application_table_p99() {
        use DiffInstruction::{Add, Copy, Del};

        fn run(base: &str, instructions: &[DiffInstruction]) -> String {
            let chars: Vec<char> = base.chars().collect();
            apply_tlcp_instructions(&chars, instructions).unwrap()
        }

        assert_eq!(run("foo", &[Copy(3)]), "foo");
        // A diff that stops early truncates the base: no implicit trailing
        // copy [p.99].
        assert_eq!(run("foobar", &[Copy(3)]), "foo");
        assert_eq!(run("foobar", &[Copy(1), Add("zap".to_owned())]), "fzap");
        assert_eq!(
            run("foobar", &[Copy(1), Add("zap".to_owned()), Del(2), Copy(3)]),
            "fzapbar"
        );
        assert_eq!(
            run("foobar", &[Copy(0), Add("zap".to_owned()), Del(0), Copy(3)]),
            "zapfoo"
        );
        assert_eq!(
            run("foobar", &[Copy(2), Add(String::new()), Del(2), Copy(2)]),
            "foar"
        );
    }

    #[test]
    fn test_apply_tlcp_diff_end_to_end_over_both_appendix_d_tables_p98_p99() {
        // Every value from the section table [p.98] run against the base of the
        // application table [p.99]; the two tables share the same instruction
        // lists, so the pairings are exactly the spec's own.
        assert_eq!(apply_tlcp_diff("foo", "d").unwrap(), "foo");
        assert_eq!(apply_tlcp_diff("foobar", "d").unwrap(), "foo");
        assert_eq!(apply_tlcp_diff("foobar", "bdzap").unwrap(), "fzap");
        assert_eq!(apply_tlcp_diff("foobar", "bdzapcd").unwrap(), "fzapbar");
        assert_eq!(apply_tlcp_diff("foobar", "adzapad").unwrap(), "zapfoo");
        // The last application row, `COPY(2) ADD(0,) DEL(2) COPY(2)`, encodes
        // as `cacc` — the spec gives the instruction list but not the value
        // [p.99].
        assert_eq!(apply_tlcp_diff("foobar", "cacc").unwrap(), "foar");
    }

    #[test]
    fn test_apply_tlcp_diff_through_item_state() {
        let mut state = ItemState::new(2);
        state.apply("t0|foobar").unwrap();

        let outcome = state.apply("t1|^Tbdzapcd").unwrap();
        assert_eq!(state.field(1), Some(Some("fzapbar")));
        assert_eq!(outcome.changed_fields(), &[0, 1]);
    }

    #[test]
    fn test_apply_tlcp_diff_counts_characters_not_bytes() {
        // Base of five characters, ten UTF-8 bytes. The diff is
        // COPY(2) ADD(1,"ñ") DEL(1) COPY(2) = `c` `b`+`ñ` `b` `c`.
        // Appendix D counts characters [p.98]; a byte-indexed decoder would cut
        // this base in the middle of a code point.
        let base = "áéíóú";
        assert_eq!(base.len(), 10);
        assert_eq!(base.chars().count(), 5);
        assert_eq!(apply_tlcp_diff(base, "cbñbc").unwrap(), "áéñóú");
    }

    #[test]
    fn test_apply_tlcp_diff_onto_multi_byte_value_through_item_state() {
        let mut state = ItemState::new(1);
        state.apply("日本語テキスト").unwrap();
        // COPY(3) ADD(2,"です") = `d` + `c` + `です`; keeps the first three
        // characters of a seven-character, twenty-one-byte base.
        let outcome = state.apply("^Tdcです").unwrap();
        assert_eq!(state.field(0), Some(Some("日本語です")));
        assert_eq!(outcome.changed_fields(), &[0]);
    }

    // -- Null vs empty, and the markers as content ---------------------------

    #[test]
    fn test_null_and_empty_are_distinct() {
        let mut state = ItemState::new(2);
        state.apply("#|$").unwrap();
        // `#` is null [§2.2]; `$` is the empty string [§2.2]. Collapsing them
        // would lose information the protocol carries deliberately.
        assert_eq!(state.field(0), Some(None));
        assert_eq!(state.field(1), Some(Some("")));
        assert_ne!(state.field(0), state.field(1));
    }

    #[test]
    fn test_field_index_past_schema_is_none() {
        let state = ItemState::new(2);
        assert_eq!(state.field(2), None);
        // A never-set field reads as null; the first update of a subscription
        // sets every field, so this is only observable before it [§2.3].
        assert_eq!(state.field(0), Some(None));
    }

    #[test]
    fn test_null_can_be_replaced_by_a_value_and_back() {
        let mut state = ItemState::new(1);
        state.apply("#").unwrap();
        assert_eq!(state.field(0), Some(None));
        state.apply("v").unwrap();
        assert_eq!(state.field(0), Some(Some("v")));
        state.apply("#").unwrap();
        assert_eq!(state.field(0), Some(None));
    }

    #[test]
    fn test_escaped_markers_are_content_not_markers() {
        let mut state = ItemState::new(3);
        // "#, $ and ^ characters are percent-encoded if occurring at the
        // beginning of an actual content (and only in this case)" [§2.2, p.51].
        state.apply("%23|%24|%5E").unwrap();
        assert_eq!(state.field(0), Some(Some("#")));
        assert_eq!(state.field(1), Some(Some("$")));
        assert_eq!(state.field(2), Some(Some("^")));
    }

    #[test]
    fn test_hash_and_dollar_match_only_as_a_whole_token() {
        let mut state = ItemState::new(2);
        // §2.3 invariant 2: the marker test is for a token that *is* a single
        // hash / dollar sign, so a longer token is content.
        state.apply("%23abc|%24x").unwrap();
        assert_eq!(state.field(0), Some(Some("#abc")));
        assert_eq!(state.field(1), Some(Some("$x")));
    }

    #[test]
    fn test_pipe_inside_a_value_is_percent_encoded() {
        let mut state = ItemState::new(2);
        // §2.3 invariant 1: splitting comes first, and `%7C` is not a
        // separator [p.52].
        let outcome = state.apply("aa%7Cbb|cc").unwrap();
        assert_eq!(state.field(0), Some(Some("aa|bb")));
        assert_eq!(state.field(1), Some(Some("cc")));
        assert_eq!(outcome.changed_count(), 2);
    }

    // -- `^N` runs -----------------------------------------------------------

    #[test]
    fn test_caret_run_is_inclusive_of_the_pointed_field() {
        let mut state = ItemState::new(4);
        state.apply("a|b|c|d").unwrap();
        // §2.3 invariant 6: `^3` covers the pointed field and the following
        // two, so only the fourth field is left for the next token.
        let outcome = state.apply("^3|z").unwrap();
        assert_eq!(state.field(0), Some(Some("a")));
        assert_eq!(state.field(1), Some(Some("b")));
        assert_eq!(state.field(2), Some(Some("c")));
        assert_eq!(state.field(3), Some(Some("z")));
        assert_eq!(outcome.changed_fields(), &[3]);
    }

    #[test]
    fn test_caret_run_at_the_end_of_the_schema() {
        let mut state = ItemState::new(4);
        state.apply("a|b|c|d").unwrap();
        let outcome = state.apply("z|^3").unwrap();
        assert_eq!(state.field(0), Some(Some("z")));
        assert_eq!(state.field(3), Some(Some("d")));
        assert_eq!(outcome.changed_fields(), &[0]);
    }

    #[test]
    fn test_caret_run_covering_the_whole_schema_changes_nothing() {
        let mut state = ItemState::new(3);
        state.apply("a|b|c").unwrap();
        let outcome = state.apply("^3").unwrap();
        assert!(outcome.is_empty());
        assert_eq!(outcome.changed_fields(), &[] as &[usize]);
    }

    #[test]
    fn test_caret_run_of_zero_advances_nothing() {
        let mut state = ItemState::new(1);
        state.apply("a").unwrap();
        // SPEC-AMBIGUITY (unchanged-run-number-format): the spec allows a count
        // of 0 without saying so explicitly; it is accepted as the no-op it
        // literally describes.
        let outcome = state.apply("^0|b").unwrap();
        assert_eq!(state.field(0), Some(Some("b")));
        assert_eq!(outcome.changed_fields(), &[0]);
    }

    #[test]
    fn test_caret_run_with_leading_zeros_is_accepted() {
        let mut state = ItemState::new(3);
        state.apply("a|b|c").unwrap();
        assert!(state.apply("^003").is_ok());
    }

    // -- Malformed input -----------------------------------------------------

    fn assert_field_value_error(result: Result<UpdateOutcome, ProtocolError>) {
        assert!(
            matches!(result, Err(ProtocolError::FieldValue { .. })),
            "expected ProtocolError::FieldValue, got {result:?}"
        );
    }

    #[test]
    fn test_unknown_diff_tag_is_an_error() {
        let mut state = ItemState::new(1);
        state.apply("base").unwrap();
        // ADR-0004: an algorithm the client did not advertise is a typed error,
        // never a silent passthrough.
        assert_field_value_error(state.apply("^Zwhatever"));
        assert_eq!(state.field(0), Some(Some("base")));
    }

    #[cfg(not(feature = "json-patch"))]
    #[test]
    fn test_json_patch_tag_is_unknown_when_the_feature_is_off() {
        let mut state = ItemState::new(1);
        state.apply(r#"{"a":1}"#).unwrap();
        // The decoder is not compiled in, so `P` is not advertised and must not
        // be accepted (ADR-0004).
        assert_field_value_error(state.apply(r#"^P[{"op":"remove","path":"/a"}]"#));
    }

    #[test]
    fn test_bare_caret_is_an_error() {
        let mut state = ItemState::new(1);
        state.apply("a").unwrap();
        // SPEC-AMBIGUITY (caret-without-marker).
        assert_field_value_error(state.apply("^"));
    }

    #[test]
    fn test_caret_followed_by_neither_letter_nor_digit_is_an_error() {
        let mut state = ItemState::new(1);
        state.apply("a").unwrap();
        // SPEC-AMBIGUITY (caret-marker-not-letter-or-digit).
        assert_field_value_error(state.apply("^!x"));
        assert_field_value_error(state.apply("^-1"));
        assert_field_value_error(state.apply("^^"));
    }

    #[test]
    fn test_caret_run_with_trailing_garbage_is_an_error() {
        let mut state = ItemState::new(3);
        state.apply("a|b|c").unwrap();
        // SPEC-AMBIGUITY (unchanged-run-number-format): `^2x` is not read as 2.
        assert_field_value_error(state.apply("^2x|z"));
    }

    #[test]
    fn test_caret_run_out_of_range_is_an_error() {
        let mut state = ItemState::new(3);
        state.apply("a|b|c").unwrap();
        assert_field_value_error(state.apply("^99999999999999999999999999"));
    }

    #[test]
    fn test_caret_run_overrunning_the_schema_is_an_error() {
        let mut state = ItemState::new(3);
        state.apply("a|b|c").unwrap();
        // SPEC-AMBIGUITY (run-overruns-schema).
        assert_field_value_error(state.apply("z|^5"));
        assert_eq!(state.field(0), Some(Some("a")));
    }

    #[test]
    fn test_line_shorter_than_the_schema_is_an_error() {
        let mut state = ItemState::new(3);
        state.apply("a|b|c").unwrap();
        // SPEC-AMBIGUITY (line-shorter-than-schema).
        assert_field_value_error(state.apply("z|y"));
        assert_eq!(state.field(0), Some(Some("a")));
    }

    #[test]
    fn test_line_longer_than_the_schema_is_an_error() {
        let mut state = ItemState::new(2);
        assert_field_value_error(state.apply("a|b|c"));
    }

    #[test]
    fn test_diff_onto_a_null_field_is_an_error() {
        let mut state = ItemState::new(1);
        state.apply("#").unwrap();
        // §2.3: "this case never happens if the value of the pointed field is
        // currently null". SPEC-AMBIGUITY (diff-onto-non-value).
        assert_field_value_error(state.apply("^Td"));
        assert_eq!(state.field(0), Some(None));
    }

    #[test]
    fn test_diff_onto_a_never_set_field_is_an_error() {
        let mut state = ItemState::new(1);
        // SPEC-AMBIGUITY (diff-onto-non-value).
        assert_field_value_error(state.apply("^Td"));
    }

    #[test]
    fn test_diff_onto_an_empty_field_is_accepted() {
        let mut state = ItemState::new(1);
        state.apply("$").unwrap();
        // SPEC-AMBIGUITY (diff-onto-non-value): the empty string is a base of
        // zero characters, so COPY(0) ADD(3,"new") applies cleanly.
        let outcome = state.apply("^Tadnew").unwrap();
        assert_eq!(state.field(0), Some(Some("new")));
        assert_eq!(outcome.changed_fields(), &[0]);
    }

    #[test]
    fn test_malformed_tlcp_diff_payloads_are_errors() {
        let mut state = ItemState::new(1);
        state.apply("foobar").unwrap();

        // Empty payload: Appendix D requires "one or more sections" [p.98].
        assert_field_value_error(state.apply("^T"));
        // Truncated encoded integer: no terminating lowercase letter [p.98].
        assert_field_value_error(state.apply("^TAB"));
        // Non-letter inside an encoded integer [p.98].
        assert_field_value_error(state.apply("^T3"));
        // ADD declares more characters than remain in the payload [p.98].
        assert_field_value_error(state.apply("^Tazz"));
        // SPEC-AMBIGUITY (diff-runs-past-base): COPY past the end of the base.
        assert_field_value_error(state.apply("^TAdacAa"));

        assert_eq!(state.field(0), Some(Some("foobar")));
    }

    #[test]
    fn test_del_past_the_end_of_the_base_is_tolerated() {
        // SPEC-AMBIGUITY (del-past-base): COPY(3) ADD(0,) DEL(9) = `d` `a` `j`.
        // The sequence ends after the DEL, so the overshoot never matters.
        assert_eq!(apply_tlcp_diff("foo", "daj").unwrap(), "foo");
    }

    #[test]
    fn test_a_rejected_update_leaves_every_field_untouched() {
        let mut state = ItemState::new(3);
        state.apply("a|b|c").unwrap();
        // The first two tokens are well formed; the third is not. Nothing is
        // committed.
        assert_field_value_error(state.apply("x|y|^"));
        assert_eq!(state.field(0), Some(Some("a")));
        assert_eq!(state.field(1), Some(Some("b")));
        assert_eq!(state.field(2), Some(Some("c")));
    }

    // -- `LS_supported_diffs` (ADR-0004) -------------------------------------

    #[test]
    fn test_supported_diffs_is_derived_from_the_compiled_decoders() {
        // "A (possibly empty) comma-separated list of format tags, as specified
        // in Real-Time Update" [`docs/spec/03-requests.md` §2.1].
        let advertised = supported_diffs();
        let tags: Vec<&str> = advertised.split(',').collect();
        assert_eq!(tags.len(), DIFF_DECODERS.len());
        for (tag, decoder) in tags.iter().zip(DIFF_DECODERS) {
            assert_eq!(*tag, decoder.tag.to_string());
        }
    }

    #[cfg(feature = "json-patch")]
    #[test]
    fn test_supported_diffs_wire_form_with_json_patch() {
        assert_eq!(supported_diffs(), "T,P");
    }

    #[cfg(not(feature = "json-patch"))]
    #[test]
    fn test_supported_diffs_wire_form_without_json_patch() {
        assert_eq!(supported_diffs(), "T");
    }

    #[test]
    fn test_every_advertised_tag_has_a_decoder() {
        // ADR-0004 made this unrepresentable by construction; the test keeps
        // the derivation honest if the registry ever grows a second source.
        for tag in supported_diffs().chars().filter(|c| *c != ',') {
            assert!(
                DIFF_DECODERS.iter().any(|decoder| decoder.tag == tag),
                "advertised `{tag}` has no decoder"
            );
        }
    }
}
