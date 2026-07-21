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
//! - The **diff algorithms themselves**. `^T` and `^P` are pure functions of a
//!   base string and a payload, defined by the specification and advertised in
//!   `LS_supported_diffs`, so they live in [`crate::protocol::diff`] with the
//!   rest of the wire format. This module only decides *which* field a diff
//!   applies to and whether that field may serve as its base
//!   (`docs/adr/0004-supported-diffs-derived-from-decoders.md`).

use crate::protocol::diff::{DiffDecoder, LexicalForm, decoder_for, supported_diffs};
use crate::protocol::escaping::percent_decode;
use crate::protocol::{ProtocolError, field_value_error};

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
                    let decoder = decoder_for(marker).ok_or_else(|| {
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
}
