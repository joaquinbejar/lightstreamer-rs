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

/// Whether the client's copy of a field value is the exact character sequence
/// the server sent, or one the client produced itself.
///
/// The distinction only matters as the **base of a later character diff**. A
/// `^T` diff is defined by character positions into the previous value
/// [Appendix D, pp.98-99], so it can only be applied to a value that is
/// character-for-character the one the server diffed against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LexicalForm {
    /// The value is exactly the character sequence that arrived on the wire —
    /// actual content, the empty string set by `$`, or the result of a diff
    /// whose base was itself exact and whose output is defined character by
    /// character.
    AsSent,
    /// The value was reconstructed by the client and its spelling is not
    /// guaranteed to match the server's.
    ///
    /// Produced only by `^P`: applying an RFC 6902 patch requires parsing the
    /// JSON document and serialising it again, which does not preserve
    /// insignificant whitespace, member order or number spelling. See
    /// SPEC-AMBIGUITY (json-patch-reserialisation) on `apply_json_patch`.
    // The `^P` decoder is the only producer, so without that feature the
    // variant is unreachable by construction — which is the point: the guard
    // it feeds still compiles and still reads the same in both builds.
    #[cfg_attr(not(feature = "json-patch"), allow(dead_code))]
    ClientDerived,
}

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
    /// carries content, `#` or `$` for every field of the schema — and
    /// [`ItemState::apply`] rejects any update that says otherwise.
    Unset,
    /// The field is null — the `#` marker
    /// [`docs/spec/04-notifications.md` §2.2].
    Null,
    /// The field holds a string, possibly empty — the `$` marker sets it to the
    /// empty string [`docs/spec/04-notifications.md` §2.2].
    Value {
        /// The characters of the value.
        text: String,
        /// Whether `text` may serve as the base of a `^T` character diff.
        form: LexicalForm,
    },
}

impl FieldSlot {
    /// A value that is exactly what the server sent.
    #[must_use]
    fn as_sent(text: impl Into<String>) -> Self {
        Self::Value {
            text: text.into(),
            form: LexicalForm::AsSent,
        }
    }
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
            FieldSlot::Value { text, .. } => Some(Some(text.as_str())),
        }
    }

    /// Whether field `index` has a value an unchanged token could preserve.
    ///
    /// §2.3 attaches the same note to the empty token and to every `^`
    /// directive: "this case never happens on the first update of a
    /// subscription". A field that no update has set therefore cannot be the
    /// target of one, and accepting it would publish a field that reads as null
    /// while no server value was ever behind it.
    #[must_use]
    fn has_baseline(&self, index: usize) -> bool {
        matches!(
            self.fields.get(index),
            Some(FieldSlot::Null | FieldSlot::Value { .. })
        )
    }

    /// The error an unchanged token that touches a never-set field reports.
    #[cold]
    #[inline(never)]
    fn no_baseline_error(token: &str, index: usize) -> ProtocolError {
        field_value_error(format!(
            "{token} leaves field {index} unchanged, but no update has ever set it; §2.3 states \
             this never happens on the first update of a subscription"
        ))
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
            // unchanged and the pointer moved to the next field." The same
            // clause carries the note "this case never happens on the first
            // update of a subscription", so there must be something to leave
            // unchanged.
            if token.is_empty() {
                if !self.has_baseline(pointer) {
                    return Err(Self::no_baseline_error("an empty token", pointer));
                }
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
                staged.push((pointer, FieldSlot::as_sent(String::new())));
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
                    // §2.3 attaches "this case never happens on the first
                    // update of a subscription" to the whole `^` case, so every
                    // field the run covers must already have a value.
                    if let Some(index) = (pointer..end).find(|index| !self.has_baseline(*index)) {
                        return Err(Self::no_baseline_error(
                            &format!("the run `^{rest}`"),
                            index,
                        ));
                    }
                    pointer = end;
                    continue;
                }

                if marker.is_ascii_alphabetic() {
                    // §2.3, case letter: "take the substring following the
                    // letter and decode any percent-encoding", then apply it as
                    // a difference in the format named by the letter [§2.2].
                    let payload = percent_decode(rest_chars.as_str())?;

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
                        Some(FieldSlot::Value { text, form }) => {
                            Self::diffable_base(decoder, marker, pointer, text, *form)?
                        }
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

                    let text = (decoder.apply)(base, payload.as_ref())?;
                    staged.push((
                        pointer,
                        FieldSlot::Value {
                            text,
                            form: decoder.result_form,
                        },
                    ));
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
            staged.push((pointer, FieldSlot::as_sent(decoded.into_owned())));
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

    /// Checks that the current value of a field may serve as the base of the
    /// diff `decoder` is about to apply.
    ///
    /// SPEC-AMBIGUITY (mixed-diff-baseline): the spec allows a client to
    /// advertise several formats in `LS_supported_diffs`
    /// [`docs/spec/03-requests.md` §2.1] and lets the server pick a different
    /// one per update [`docs/spec/04-notifications.md` §2.2], but never says
    /// which representation of the previous value the next diff is computed
    /// against. For `^P` that does not matter — RFC 6902 addresses a *parsed*
    /// document, so a re-serialised base is equivalent — but a `^T` is defined
    /// over character positions [Appendix D, pp.98-99] and can only be applied
    /// to the exact characters the server diffed against. After a `^P` this
    /// client holds a re-serialisation of its own (see
    /// SPEC-AMBIGUITY (json-patch-reserialisation)), which may differ from the
    /// server's text in whitespace, member order or number spelling. Choice:
    /// reject the `^T`. Applying it would either fail on a copy overrun or,
    /// worse, succeed and hand the caller silently corrupted JSON.
    ///
    /// # Errors
    ///
    /// [`ProtocolError::FieldValue`] when the diff needs the server's own
    /// characters and this client no longer has them.
    fn diffable_base<'slot>(
        decoder: &DiffDecoder,
        marker: char,
        index: usize,
        text: &'slot str,
        form: LexicalForm,
    ) -> Result<&'slot str, ProtocolError> {
        match form {
            LexicalForm::AsSent => Ok(text),
            LexicalForm::ClientDerived if decoder.needs_base_as_sent => {
                Err(field_value_error(format!(
                    "`^{marker}` diff applies to field {index}, whose current value was rebuilt \
                     by this client from a `^P` patch; a character diff cannot be applied to a \
                     value whose spelling is not the server's"
                )))
            }
            LexicalForm::ClientDerived => Ok(text),
        }
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
    // This and the two below have no non-test caller: the manager forwards
    // `changed_fields` into an `ItemUpdate`, which exposes the same three
    // questions publicly. They are what this module's tests assert against.
    #[allow(dead_code)]
    #[must_use]
    #[inline]
    pub(crate) fn is_field_changed(&self, index: usize) -> bool {
        self.changed.contains(&index)
    }

    /// How many fields this update changed.
    #[allow(dead_code)]
    #[must_use]
    #[inline]
    pub(crate) fn changed_count(&self) -> usize {
        self.changed.len()
    }

    /// Whether this update changed nothing at all.
    #[allow(dead_code)]
    #[must_use]
    #[inline]
    pub(crate) fn is_empty(&self) -> bool {
        self.changed.is_empty()
    }
}

// ---------------------------------------------------------------------------
// Diff decoder registry (ADR-0004)
// ---------------------------------------------------------------------------

/// One compiled-in diff format: its wire tag, the function that applies it, and
/// how it relates to the exactness of the value it reads and writes.
struct DiffDecoder {
    /// The letter that follows the caret on the wire
    /// [`docs/spec/04-notifications.md` §2.2].
    tag: char,
    /// Applies a percent-decoded diff payload to the field's previous value.
    apply: fn(base: &str, payload: &str) -> Result<String, ProtocolError>,
    /// Whether the format addresses the base by character position, and
    /// therefore needs the server's own characters — see
    /// [`ItemState::diffable_base`].
    needs_base_as_sent: bool,
    /// What the exactness of the produced value is.
    result_form: LexicalForm,
}

/// TLCP-diff, the custom format of Appendix D
/// [`docs/spec/04-notifications.md` §2.7, spec pp.98-99].
///
/// Every instruction is a character count into the base [p.98], so the base
/// must be exact; the result is then assembled character by character out of an
/// exact base and the payload's own literals, and is exact in turn.
const TLCP_DIFF_DECODER: DiffDecoder = DiffDecoder {
    tag: 'T',
    apply: apply_tlcp_diff,
    needs_base_as_sent: true,
    result_form: LexicalForm::AsSent,
};

/// JSON Patch, RFC 6902 [`docs/spec/04-notifications.md` §2.2].
///
/// RFC 6902 addresses a parsed document rather than a character sequence, so
/// any equivalent spelling of the base will do; the value it produces is this
/// client's own serialisation and is therefore not exact.
#[cfg(feature = "json-patch")]
const JSON_PATCH_DECODER: DiffDecoder = DiffDecoder {
    tag: 'P',
    apply: apply_json_patch,
    needs_base_as_sent: false,
    result_form: LexicalForm::ClientDerived,
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
    // §2.2 states the precondition of the `T` format verbatim: it "can be
    // applied to all strings, only provided that their encoding in UTF-16 (the
    // format used internally in Java) doesn't contain surrogate pairs.
    // Obviously, if a TLCP-diff is received, this guarantees that both the
    // previous and new value are compliant."
    //
    // Under that guarantee one Unicode scalar value is one UTF-16 code unit, so
    // the appendix's "characters" — UTF-16 code units, the format being defined
    // for a Java implementation — coincide exactly with Rust `char`s. Byte
    // offsets do *not* coincide: a two-byte UTF-8 character would make every
    // count wrong. Both the base and the payload are therefore materialised as
    // `Vec<char>` and every position and count in this function is a `char`
    // index.
    //
    // A character outside the BMP breaks that coincidence: the server counts it
    // as two units and this decoder would count it as one, so every position
    // after it refers to a different place in the two implementations. The
    // guarantee is the server's to keep, so its violation is checked and
    // reported rather than absorbed — silently decoding under a broken
    // precondition produces a plausible value that is wrong where the caller
    // cannot see it.
    reject_astral(base, "the base value")?;
    reject_astral(payload, "the diff payload")?;

    let base_chars: Vec<char> = base.chars().collect();
    let instructions = parse_tlcp_diff(payload)?;
    apply_tlcp_instructions(&base_chars, &instructions)
}

/// Rejects text the `T` precondition of
/// [`docs/spec/04-notifications.md` §2.2] forbids.
///
/// A `char` whose UTF-16 encoding takes two code units is exactly a character
/// "whose encoding in UTF-16 contains a surrogate pair".
///
/// # Errors
///
/// [`ProtocolError::FieldValue`] if `text` contains such a character.
fn reject_astral(text: &str, role: &str) -> Result<(), ProtocolError> {
    match text.chars().find(|character| character.len_utf16() == 2) {
        None => Ok(()),
        Some(character) => Err(field_value_error(format!(
            "`^T` diff violates its own precondition: {role} contains `{character}` \
             (U+{:04X}), whose UTF-16 encoding is a surrogate pair, so character positions \
             cannot be counted the same way on both sides",
            u32::from(character)
        ))),
    }
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

    /// The text of field `index`, failing the test if there is no such field or
    /// if it is null.
    #[cfg(feature = "json-patch")]
    fn text_of(state: &ItemState, index: usize) -> String {
        match state.field(index) {
            Some(Some(text)) => text.to_owned(),
            other => panic!("field {index} should hold text, found {other:?}"),
        }
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
    fn test_apply_example_1_snapshot_sets_every_field_p51() -> Result<(), ProtocolError> {
        let mut state = ItemState::new(QUOTE_FIELDS);
        let outcome = state.apply(EX1)?;

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

        Ok(())
    }

    #[test]
    fn test_apply_example_2_four_unchanged_fields_p51() -> Result<(), ProtocolError> {
        let mut state = quote_state_after(&[EX1]);
        let outcome = state.apply(EX2)?;

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

        Ok(())
    }

    #[test]
    fn test_apply_example_3_dollar_resets_status_to_empty_p52() -> Result<(), ProtocolError> {
        let mut state = quote_state_after(&[EX1, EX2]);
        let outcome = state.apply(EX3)?;

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

        Ok(())
    }

    #[test]
    fn test_apply_example_4_caret_run_of_four_p52() -> Result<(), ProtocolError> {
        let mut state = quote_state_after(&[EX1, EX2, EX3]);
        let outcome = state.apply(EX4)?;

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

        Ok(())
    }

    #[test]
    fn test_apply_example_5_caret_run_reaching_end_of_schema_p52() -> Result<(), ProtocolError> {
        let mut state = quote_state_after(&[EX1, EX2, EX3, EX4]);
        let outcome = state.apply(EX5)?;

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

        Ok(())
    }

    #[test]
    fn test_apply_example_6_final_update_p52() -> Result<(), ProtocolError> {
        let mut state = quote_state_after(&[EX1, EX2, EX3, EX4, EX5]);
        let outcome = state.apply(EX6)?;

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

        Ok(())
    }

    // -- §2.6 worked examples (diffs) ---------------------------------------

    /// The §2.6 schema: `timestamp`, `text`
    /// [`docs/spec/04-notifications.md` §2.6, p.52].
    const DIFF_SNAPSHOT: &str =
        r#"20:00:33|{ "text": "aa%7Cbb", "attributes": [ { "font": "courier" }, "..." ] }"#;
    const DIFF_SNAPSHOT_TEXT: &str =
        r#"{ "text": "aa|bb", "attributes": [ { "font": "courier" }, "..." ] }"#;

    #[test]
    fn test_apply_diff_example_1_snapshot_percent_decodes_pipe_p52() -> Result<(), ProtocolError> {
        let mut state = ItemState::new(2);
        let outcome = state.apply(DIFF_SNAPSHOT)?;

        assert_eq!(state.field(0), Some(Some("20:00:33")));
        // `%7C` inside a value decodes to `|` and does not split the list
        // [`docs/spec/04-notifications.md` §2.2, §2.3 invariant 1, p.52].
        assert_eq!(state.field(1), Some(Some(DIFF_SNAPSHOT_TEXT)));
        assert_eq!(outcome.changed_fields(), &[0, 1]);

        Ok(())
    }

    #[test]
    fn test_apply_diff_example_3_empty_token_leaves_text_unchanged_p53() -> Result<(), ProtocolError>
    {
        let mut state = ItemState::new(2);
        state.apply(DIFF_SNAPSHOT)?;

        // "In the final update, the text field is unchanged; this is carried,
        // as usual, by an empty field" [p.53].
        let outcome = state.apply("20:04:16|")?;
        assert_eq!(state.field(0), Some(Some("20:04:16")));
        assert_eq!(state.field(1), Some(Some(DIFF_SNAPSHOT_TEXT)));
        assert_eq!(outcome.changed_fields(), &[0]);

        Ok(())
    }

    #[cfg(feature = "json-patch")]
    #[test]
    fn test_apply_diff_example_2_json_patch_p53() -> Result<(), Box<dyn std::error::Error>> {
        let mut state = ItemState::new(2);
        state.apply(DIFF_SNAPSHOT)?;

        let line = concat!(
            r#"20:00:54|^P[{"op":"replace","path":"/text","value":"aa%7Cbb%7Ccc"},"#,
            r#"{"op":"add","path":"/attributes/1","value":"bold"}]"#
        );
        let outcome = state.apply(line)?;

        assert_eq!(state.field(0), Some(Some("20:00:54")));
        assert_eq!(outcome.changed_fields(), &[0, 1]);

        // The spec's "actual value" row is
        // {"text":"aa|bb|cc","attributes":[{"font":"courier"},"bold","..."]}
        // [p.53]. See SPEC-AMBIGUITY (json-patch-reserialisation): the byte
        // form after re-serialisation is not defined by the spec, so the
        // assertion is on the JSON document, not on its spelling.
        let got: serde_json::Value = serde_json::from_str(&text_of(&state, 1))?;
        let want: serde_json::Value = serde_json::from_str(
            r#"{"text":"aa|bb|cc","attributes":[{"font":"courier"},"bold","..."]}"#,
        )?;
        assert_eq!(got, want);

        Ok(())
    }

    #[cfg(feature = "json-patch")]
    #[test]
    fn test_apply_json_patch_that_does_not_apply_is_an_error() -> Result<(), ProtocolError> {
        let mut state = ItemState::new(1);
        state.apply(r#"{"a":1}"#)?;
        // `remove` of a path that is not there cannot be applied.
        let result = state.apply(r#"^P[{"op":"remove","path":"/nope"}]"#);
        assert!(matches!(result, Err(ProtocolError::FieldValue { .. })));
        // The rejected update left the field alone.
        assert_eq!(state.field(0), Some(Some(r#"{"a":1}"#)));

        Ok(())
    }

    // -- Mixing `^P` and `^T` on the same field ------------------------------

    /// A JSON document whose spelling this client cannot reproduce: padded
    /// whitespace, a member order `serde_json` does not keep by default, an
    /// escaped character with a shorter equivalent, and a number spelling that
    /// does not survive a parse.
    #[cfg(feature = "json-patch")]
    const LEXICALLY_AWKWARD_JSON: &str = r#"{ "z" : 1.50 ,  "a" : "A" , "n" : [ 1, 2 ] }"#;

    #[cfg(feature = "json-patch")]
    #[test]
    fn test_a_tlcp_diff_after_a_json_patch_is_rejected() -> Result<(), ProtocolError> {
        let mut state = ItemState::new(1);
        state.apply(LEXICALLY_AWKWARD_JSON)?;

        // The `^P` succeeds, but what this client now holds is its own
        // serialisation, not the server's characters.
        state.apply(r#"^P[{"op":"replace","path":"/z","value":2}]"#)?;
        let after_patch = text_of(&state, 0);
        assert_ne!(
            after_patch, LEXICALLY_AWKWARD_JSON,
            "the reserialisation is what makes this case dangerous"
        );

        // SPEC-AMBIGUITY (mixed-diff-baseline): a `^T` is defined over
        // character positions [Appendix D, pp.98-99] and the server computed it
        // against its own text. Applying it here would corrupt the value
        // silently, so it is refused.
        assert_field_value_error(state.apply("^Tbdzap"));
        assert_eq!(state.field(0), Some(Some(after_patch.as_str())));

        Ok(())
    }

    #[cfg(feature = "json-patch")]
    #[test]
    fn test_a_json_patch_after_a_json_patch_is_accepted() -> Result<(), Box<dyn std::error::Error>>
    {
        let mut state = ItemState::new(1);
        state.apply(LEXICALLY_AWKWARD_JSON)?;
        state.apply(r#"^P[{"op":"replace","path":"/z","value":2}]"#)?;
        // RFC 6902 addresses the parsed document, so an equivalent spelling of
        // the base is as good as the server's [§2.2].
        state.apply(r#"^P[{"op":"replace","path":"/a","value":"B"}]"#)?;

        let got: serde_json::Value = serde_json::from_str(&text_of(&state, 0))?;
        let want: serde_json::Value = serde_json::from_str(r#"{"z":2,"a":"B","n":[1,2]}"#)?;
        assert_eq!(got, want);

        Ok(())
    }

    #[cfg(feature = "json-patch")]
    #[test]
    fn test_a_full_value_restores_the_baseline_a_tlcp_diff_needs() -> Result<(), ProtocolError> {
        let mut state = ItemState::new(1);
        state.apply(LEXICALLY_AWKWARD_JSON)?;
        state.apply(r#"^P[{"op":"replace","path":"/z","value":2}]"#)?;
        // Actual content is the server's own characters again, so the field is
        // once more a legal base for a character diff.
        state.apply("foobar")?;
        state.apply("^Tbdzapcd")?;
        assert_eq!(state.field(0), Some(Some("fzapbar")));

        Ok(())
    }

    #[cfg(feature = "json-patch")]
    #[test]
    fn test_a_tlcp_diff_chain_stays_a_legal_baseline() -> Result<(), ProtocolError> {
        let mut state = ItemState::new(1);
        state.apply("foobar")?;
        // A `^T` result is assembled from the server's own characters, so it is
        // exact and a further `^T` applies to it.
        state.apply("^Tbdzapcd")?;
        state.apply("^Tdczz")?;
        assert_eq!(state.field(0), Some(Some("fzazz")));

        Ok(())
    }

    #[cfg(feature = "json-patch")]
    #[test]
    fn test_apply_json_patch_onto_non_json_base_is_an_error() -> Result<(), ProtocolError> {
        let mut state = ItemState::new(1);
        state.apply("not json")?;
        let result = state.apply(r#"^P[{"op":"replace","path":"/a","value":1}]"#);
        assert!(matches!(result, Err(ProtocolError::FieldValue { .. })));

        Ok(())
    }

    // -- Appendix D: value -> instruction list [p.98] ------------------------

    #[test]
    fn test_parse_tlcp_diff_appendix_d_section_table_p98() -> Result<(), ProtocolError> {
        use DiffInstruction::{Add, Copy, Del};

        assert_eq!(parse_tlcp_diff("d")?, vec![Copy(3)]);
        assert_eq!(
            parse_tlcp_diff("bdzap")?,
            vec![Copy(1), Add("zap".to_owned())]
        );
        assert_eq!(
            parse_tlcp_diff("bdzapcd")?,
            vec![Copy(1), Add("zap".to_owned()), Del(2), Copy(3)]
        );
        assert_eq!(
            parse_tlcp_diff("adzapad")?,
            vec![Copy(0), Add("zap".to_owned()), Del(0), Copy(3)]
        );
        // The multi-letter row. See SPEC-AMBIGUITY
        // (encoded-integer-letter-weight): `Ad` is COPY(29) and `Aa` is
        // COPY(26) per the table, which the prose's own weighting rule cannot
        // produce.
        assert_eq!(
            parse_tlcp_diff("AdacAa")?,
            vec![Copy(29), Add(String::new()), Del(2), Copy(26)]
        );

        Ok(())
    }

    #[test]
    fn test_decode_encoded_integer_matches_appendix_d_examples_p98() -> Result<(), ProtocolError> {
        fn decode(text: &str) -> Result<usize, ProtocolError> {
            let chars: Vec<char> = text.chars().collect();
            let mut position = 0;
            let value = decode_encoded_integer(&chars, &mut position)?;
            assert_eq!(position, chars.len(), "consumed the whole integer");
            Ok(value)
        }

        assert_eq!(decode("a")?, 0);
        assert_eq!(decode("b")?, 1);
        assert_eq!(decode("c")?, 2);
        assert_eq!(decode("d")?, 3);
        assert_eq!(decode("z")?, 25);
        assert_eq!(decode("Aa")?, 26);
        assert_eq!(decode("Ad")?, 29);
        // Extrapolated from the same rule: (0+1)·26² + (0+1)·26 + 0.
        assert_eq!(decode("AAa")?, 702);

        Ok(())
    }

    // -- Appendix D: base + instructions -> result [p.99] --------------------

    #[test]
    fn test_apply_tlcp_instructions_appendix_d_application_table_p99() -> Result<(), ProtocolError>
    {
        use DiffInstruction::{Add, Copy, Del};

        fn run(base: &str, instructions: &[DiffInstruction]) -> Result<String, ProtocolError> {
            let chars: Vec<char> = base.chars().collect();
            apply_tlcp_instructions(&chars, instructions)
        }

        assert_eq!(run("foo", &[Copy(3)])?, "foo");
        // A diff that stops early truncates the base: no implicit trailing
        // copy [p.99].
        assert_eq!(run("foobar", &[Copy(3)])?, "foo");
        assert_eq!(run("foobar", &[Copy(1), Add("zap".to_owned())])?, "fzap");
        assert_eq!(
            run("foobar", &[Copy(1), Add("zap".to_owned()), Del(2), Copy(3)])?,
            "fzapbar"
        );
        assert_eq!(
            run("foobar", &[Copy(0), Add("zap".to_owned()), Del(0), Copy(3)])?,
            "zapfoo"
        );
        assert_eq!(
            run("foobar", &[Copy(2), Add(String::new()), Del(2), Copy(2)])?,
            "foar"
        );

        Ok(())
    }

    #[test]
    fn test_apply_tlcp_diff_end_to_end_over_both_appendix_d_tables_p98_p99()
    -> Result<(), ProtocolError> {
        // Every value from the section table [p.98] run against the base of the
        // application table [p.99]; the two tables share the same instruction
        // lists, so the pairings are exactly the spec's own.
        assert_eq!(apply_tlcp_diff("foo", "d")?, "foo");
        assert_eq!(apply_tlcp_diff("foobar", "d")?, "foo");
        assert_eq!(apply_tlcp_diff("foobar", "bdzap")?, "fzap");
        assert_eq!(apply_tlcp_diff("foobar", "bdzapcd")?, "fzapbar");
        assert_eq!(apply_tlcp_diff("foobar", "adzapad")?, "zapfoo");
        // The last application row, `COPY(2) ADD(0,) DEL(2) COPY(2)`, encodes
        // as `cacc` — the spec gives the instruction list but not the value
        // [p.99].
        assert_eq!(apply_tlcp_diff("foobar", "cacc")?, "foar");

        Ok(())
    }

    #[test]
    fn test_apply_tlcp_diff_through_item_state() -> Result<(), ProtocolError> {
        let mut state = ItemState::new(2);
        state.apply("t0|foobar")?;

        let outcome = state.apply("t1|^Tbdzapcd")?;
        assert_eq!(state.field(1), Some(Some("fzapbar")));
        assert_eq!(outcome.changed_fields(), &[0, 1]);

        Ok(())
    }

    #[test]
    fn test_apply_tlcp_diff_counts_characters_not_bytes() -> Result<(), ProtocolError> {
        // Base of five characters, ten UTF-8 bytes. The diff is
        // COPY(2) ADD(1,"ñ") DEL(1) COPY(2) = `c` `b`+`ñ` `b` `c`.
        // Appendix D counts characters [p.98]; a byte-indexed decoder would cut
        // this base in the middle of a code point.
        let base = "áéíóú";
        assert_eq!(base.len(), 10);
        assert_eq!(base.chars().count(), 5);
        assert_eq!(apply_tlcp_diff(base, "cbñbc")?, "áéñóú");

        Ok(())
    }

    #[test]
    fn test_apply_tlcp_diff_onto_multi_byte_value_through_item_state() -> Result<(), ProtocolError>
    {
        let mut state = ItemState::new(1);
        state.apply("日本語テキスト")?;
        // COPY(3) ADD(2,"です") = `d` + `c` + `です`; keeps the first three
        // characters of a seven-character, twenty-one-byte base.
        let outcome = state.apply("^Tdcです")?;
        assert_eq!(state.field(0), Some(Some("日本語です")));
        assert_eq!(outcome.changed_fields(), &[0]);

        Ok(())
    }

    // -- Null vs empty, and the markers as content ---------------------------

    #[test]
    fn test_null_and_empty_are_distinct() -> Result<(), ProtocolError> {
        let mut state = ItemState::new(2);
        state.apply("#|$")?;
        // `#` is null [§2.2]; `$` is the empty string [§2.2]. Collapsing them
        // would lose information the protocol carries deliberately.
        assert_eq!(state.field(0), Some(None));
        assert_eq!(state.field(1), Some(Some("")));
        assert_ne!(state.field(0), state.field(1));

        Ok(())
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
    fn test_null_can_be_replaced_by_a_value_and_back() -> Result<(), ProtocolError> {
        let mut state = ItemState::new(1);
        state.apply("#")?;
        assert_eq!(state.field(0), Some(None));
        state.apply("v")?;
        assert_eq!(state.field(0), Some(Some("v")));
        state.apply("#")?;
        assert_eq!(state.field(0), Some(None));

        Ok(())
    }

    #[test]
    fn test_escaped_markers_are_content_not_markers() -> Result<(), ProtocolError> {
        let mut state = ItemState::new(3);
        // "#, $ and ^ characters are percent-encoded if occurring at the
        // beginning of an actual content (and only in this case)" [§2.2, p.51].
        state.apply("%23|%24|%5E")?;
        assert_eq!(state.field(0), Some(Some("#")));
        assert_eq!(state.field(1), Some(Some("$")));
        assert_eq!(state.field(2), Some(Some("^")));

        Ok(())
    }

    #[test]
    fn test_hash_and_dollar_match_only_as_a_whole_token() -> Result<(), ProtocolError> {
        let mut state = ItemState::new(2);
        // §2.3 invariant 2: the marker test is for a token that *is* a single
        // hash / dollar sign, so a longer token is content.
        state.apply("%23abc|%24x")?;
        assert_eq!(state.field(0), Some(Some("#abc")));
        assert_eq!(state.field(1), Some(Some("$x")));

        Ok(())
    }

    #[test]
    fn test_pipe_inside_a_value_is_percent_encoded() -> Result<(), ProtocolError> {
        let mut state = ItemState::new(2);
        // §2.3 invariant 1: splitting comes first, and `%7C` is not a
        // separator [p.52].
        let outcome = state.apply("aa%7Cbb|cc")?;
        assert_eq!(state.field(0), Some(Some("aa|bb")));
        assert_eq!(state.field(1), Some(Some("cc")));
        assert_eq!(outcome.changed_count(), 2);

        Ok(())
    }

    // -- `^N` runs -----------------------------------------------------------

    #[test]
    fn test_caret_run_is_inclusive_of_the_pointed_field() -> Result<(), ProtocolError> {
        let mut state = ItemState::new(4);
        state.apply("a|b|c|d")?;
        // §2.3 invariant 6: `^3` covers the pointed field and the following
        // two, so only the fourth field is left for the next token.
        let outcome = state.apply("^3|z")?;
        assert_eq!(state.field(0), Some(Some("a")));
        assert_eq!(state.field(1), Some(Some("b")));
        assert_eq!(state.field(2), Some(Some("c")));
        assert_eq!(state.field(3), Some(Some("z")));
        assert_eq!(outcome.changed_fields(), &[3]);

        Ok(())
    }

    #[test]
    fn test_caret_run_at_the_end_of_the_schema() -> Result<(), ProtocolError> {
        let mut state = ItemState::new(4);
        state.apply("a|b|c|d")?;
        let outcome = state.apply("z|^3")?;
        assert_eq!(state.field(0), Some(Some("z")));
        assert_eq!(state.field(3), Some(Some("d")));
        assert_eq!(outcome.changed_fields(), &[0]);

        Ok(())
    }

    #[test]
    fn test_caret_run_covering_the_whole_schema_changes_nothing() -> Result<(), ProtocolError> {
        let mut state = ItemState::new(3);
        state.apply("a|b|c")?;
        let outcome = state.apply("^3")?;
        assert!(outcome.is_empty());
        assert_eq!(outcome.changed_fields(), &[] as &[usize]);

        Ok(())
    }

    #[test]
    fn test_caret_run_of_zero_advances_nothing() -> Result<(), ProtocolError> {
        let mut state = ItemState::new(1);
        state.apply("a")?;
        // SPEC-AMBIGUITY (unchanged-run-number-format): the spec allows a count
        // of 0 without saying so explicitly; it is accepted as the no-op it
        // literally describes.
        let outcome = state.apply("^0|b")?;
        assert_eq!(state.field(0), Some(Some("b")));
        assert_eq!(outcome.changed_fields(), &[0]);

        Ok(())
    }

    // -- Unchanged tokens need a baseline (§2.3 "never on the first update") --

    #[test]
    fn test_empty_token_on_the_first_update_is_rejected() {
        let mut state = ItemState::new(2);
        // §2.3, empty-token case: "Note: this case never happens on the first
        // update of a subscription." Accepting it would publish field 0 as null
        // although no server value was ever behind it.
        assert_field_value_error(state.apply("|a"));
        assert_eq!(state.field(0), Some(None));
        assert_eq!(state.field(1), Some(None));
    }

    #[test]
    fn test_caret_run_on_the_first_update_is_rejected() {
        let mut state = ItemState::new(3);
        // §2.3 attaches the same note to the whole `^` case, so a run cannot
        // skip fields that have never been set either.
        assert_field_value_error(state.apply("^2|c"));
        // Not even when the run is what completes the line.
        assert_field_value_error(state.apply("a|^2"));
    }

    #[test]
    fn test_unchanged_tokens_are_accepted_over_every_kind_of_baseline() -> Result<(), ProtocolError>
    {
        let mut state = ItemState::new(3);
        // A first update carries a token for every field: content, `#` and `$`
        // are all baselines [§2.3].
        state.apply("a|#|$")?;

        let outcome = state.apply("|^2")?;
        assert!(outcome.is_empty());
        assert_eq!(state.field(0), Some(Some("a")));
        assert_eq!(state.field(1), Some(None));
        assert_eq!(state.field(2), Some(Some("")));

        Ok(())
    }

    #[test]
    fn test_a_run_of_zero_needs_no_baseline() -> Result<(), ProtocolError> {
        let mut state = ItemState::new(1);
        // `^0` covers no field, so there is nothing to have a baseline for.
        state.apply("^0|a")?;
        assert_eq!(state.field(0), Some(Some("a")));

        Ok(())
    }

    #[test]
    fn test_caret_run_with_leading_zeros_is_accepted() -> Result<(), ProtocolError> {
        let mut state = ItemState::new(3);
        state.apply("a|b|c")?;
        assert!(state.apply("^003").is_ok());

        Ok(())
    }

    // -- Malformed input -----------------------------------------------------

    fn assert_field_value_error(result: Result<UpdateOutcome, ProtocolError>) {
        assert!(
            matches!(result, Err(ProtocolError::FieldValue { .. })),
            "expected ProtocolError::FieldValue, got {result:?}"
        );
    }

    #[test]
    fn test_unknown_diff_tag_is_an_error() -> Result<(), ProtocolError> {
        let mut state = ItemState::new(1);
        state.apply("base")?;
        // ADR-0004: an algorithm the client did not advertise is a typed error,
        // never a silent passthrough.
        assert_field_value_error(state.apply("^Zwhatever"));
        assert_eq!(state.field(0), Some(Some("base")));

        Ok(())
    }

    #[cfg(not(feature = "json-patch"))]
    #[test]
    fn test_json_patch_tag_is_unknown_when_the_feature_is_off() -> Result<(), ProtocolError> {
        let mut state = ItemState::new(1);
        state.apply(r#"{"a":1}"#)?;
        // The decoder is not compiled in, so `P` is not advertised and must not
        // be accepted (ADR-0004).
        assert_field_value_error(state.apply(r#"^P[{"op":"remove","path":"/a"}]"#));

        Ok(())
    }

    #[test]
    fn test_bare_caret_is_an_error() -> Result<(), ProtocolError> {
        let mut state = ItemState::new(1);
        state.apply("a")?;
        // SPEC-AMBIGUITY (caret-without-marker).
        assert_field_value_error(state.apply("^"));

        Ok(())
    }

    #[test]
    fn test_caret_followed_by_neither_letter_nor_digit_is_an_error() -> Result<(), ProtocolError> {
        let mut state = ItemState::new(1);
        state.apply("a")?;
        // SPEC-AMBIGUITY (caret-marker-not-letter-or-digit).
        assert_field_value_error(state.apply("^!x"));
        assert_field_value_error(state.apply("^-1"));
        assert_field_value_error(state.apply("^^"));

        Ok(())
    }

    #[test]
    fn test_caret_run_with_trailing_garbage_is_an_error() -> Result<(), ProtocolError> {
        let mut state = ItemState::new(3);
        state.apply("a|b|c")?;
        // SPEC-AMBIGUITY (unchanged-run-number-format): `^2x` is not read as 2.
        assert_field_value_error(state.apply("^2x|z"));

        Ok(())
    }

    #[test]
    fn test_caret_run_out_of_range_is_an_error() -> Result<(), ProtocolError> {
        let mut state = ItemState::new(3);
        state.apply("a|b|c")?;
        assert_field_value_error(state.apply("^99999999999999999999999999"));

        Ok(())
    }

    #[test]
    fn test_caret_run_overrunning_the_schema_is_an_error() -> Result<(), ProtocolError> {
        let mut state = ItemState::new(3);
        state.apply("a|b|c")?;
        // SPEC-AMBIGUITY (run-overruns-schema).
        assert_field_value_error(state.apply("z|^5"));
        assert_eq!(state.field(0), Some(Some("a")));

        Ok(())
    }

    #[test]
    fn test_line_shorter_than_the_schema_is_an_error() -> Result<(), ProtocolError> {
        let mut state = ItemState::new(3);
        state.apply("a|b|c")?;
        // SPEC-AMBIGUITY (line-shorter-than-schema).
        assert_field_value_error(state.apply("z|y"));
        assert_eq!(state.field(0), Some(Some("a")));

        Ok(())
    }

    #[test]
    fn test_line_longer_than_the_schema_is_an_error() {
        let mut state = ItemState::new(2);
        assert_field_value_error(state.apply("a|b|c"));
    }

    #[test]
    fn test_diff_onto_a_null_field_is_an_error() -> Result<(), ProtocolError> {
        let mut state = ItemState::new(1);
        state.apply("#")?;
        // §2.3: "this case never happens if the value of the pointed field is
        // currently null". SPEC-AMBIGUITY (diff-onto-non-value).
        assert_field_value_error(state.apply("^Td"));
        assert_eq!(state.field(0), Some(None));

        Ok(())
    }

    #[test]
    fn test_diff_onto_a_never_set_field_is_an_error() {
        let mut state = ItemState::new(1);
        // SPEC-AMBIGUITY (diff-onto-non-value).
        assert_field_value_error(state.apply("^Td"));
    }

    #[test]
    fn test_diff_onto_an_empty_field_is_accepted() -> Result<(), ProtocolError> {
        let mut state = ItemState::new(1);
        state.apply("$")?;
        // SPEC-AMBIGUITY (diff-onto-non-value): the empty string is a base of
        // zero characters, so COPY(0) ADD(3,"new") applies cleanly.
        let outcome = state.apply("^Tadnew")?;
        assert_eq!(state.field(0), Some(Some("new")));
        assert_eq!(outcome.changed_fields(), &[0]);

        Ok(())
    }

    #[test]
    fn test_malformed_tlcp_diff_payloads_are_errors() -> Result<(), ProtocolError> {
        let mut state = ItemState::new(1);
        state.apply("foobar")?;

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

        Ok(())
    }

    #[test]
    fn test_del_past_the_end_of_the_base_is_tolerated() -> Result<(), ProtocolError> {
        // SPEC-AMBIGUITY (del-past-base): COPY(3) ADD(0,) DEL(9) = `d` `a` `j`.
        // The sequence ends after the DEL, so the overshoot never matters.
        assert_eq!(apply_tlcp_diff("foo", "daj")?, "foo");

        Ok(())
    }

    #[test]
    fn test_a_copy_after_an_overshooting_del_is_rejected() {
        // The tolerance above is bounded: `daja` is
        // COPY(3) ADD(0,) DEL(9) COPY(0), and even a zero-length copy from a
        // cursor past the end of the base is refused, so a DEL overshoot that
        // could affect the result never escapes.
        assert!(apply_tlcp_diff("foo", "daja").is_err());
        // One character further out is refused for the same reason.
        assert!(apply_tlcp_diff("foo", "dajb").is_err());
    }

    #[test]
    fn test_copy_boundaries_against_the_base() -> Result<(), ProtocolError> {
        // Exactly to the end is the appendix's own first example [p.99].
        assert_eq!(apply_tlcp_diff("foo", "d")?, "foo");
        // One past the end is not: COPY(4) over a three-character base.
        assert!(apply_tlcp_diff("foo", "e").is_err());
        // An empty base is a base of zero characters: COPY(0) applies, COPY(1)
        // does not.
        assert_eq!(apply_tlcp_diff("", "a")?, "");
        assert!(apply_tlcp_diff("", "b").is_err());

        Ok(())
    }

    #[test]
    fn test_tlcp_diff_rejects_a_base_outside_the_bmp() {
        // §2.2: the `T` format "can be applied to all strings, only provided
        // that their encoding in UTF-16 ... doesn't contain surrogate pairs",
        // and receiving one "guarantees that both the previous and new value
        // are compliant". `😀` is U+1F600, a surrogate pair in UTF-16, so the
        // server counts it as two units and this decoder would count one.
        assert!(apply_tlcp_diff("😀bc", "d").is_err());
        assert!(apply_tlcp_diff("ab😀", "b").is_err());
        assert!(apply_tlcp_diff("a😀c", "bbz").is_err());
    }

    #[test]
    fn test_tlcp_diff_rejects_a_payload_outside_the_bmp() {
        // The same guarantee covers the new value, so an ADD cannot introduce
        // one either [§2.2].
        assert!(apply_tlcp_diff("foo", "db😀").is_err());
    }

    #[test]
    fn test_tlcp_diff_rejects_an_astral_base_through_item_state() -> Result<(), ProtocolError> {
        let mut state = ItemState::new(1);
        state.apply("a😀c")?;
        assert_field_value_error(state.apply("^Tbbz"));
        // The rejected update left the value alone.
        assert_eq!(state.field(0), Some(Some("a😀c")));

        Ok(())
    }

    #[test]
    fn test_a_rejected_update_leaves_every_field_untouched() -> Result<(), ProtocolError> {
        let mut state = ItemState::new(3);
        state.apply("a|b|c")?;
        // The first two tokens are well formed; the third is not. Nothing is
        // committed.
        assert_field_value_error(state.apply("x|y|^"));
        assert_eq!(state.field(0), Some(Some("a")));
        assert_eq!(state.field(1), Some(Some("b")));
        assert_eq!(state.field(2), Some(Some("c")));

        Ok(())
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
