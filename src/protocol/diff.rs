//! The diff (delta) encodings a field value may arrive in, and the set this
//! build advertises.
//!
//! A `U` notification may carry a field value not as content but as a
//! **difference** against the field's previous value, tagged by a single letter
//! after a caret: `^T` for the TLCP-diff format of Appendix D and `^P` for an
//! RFC 6902 JSON Patch [`docs/spec/04-notifications.md` §2.2]. The set of tags
//! the server may choose from is the set the client declared in
//! `LS_supported_diffs` at session creation [`docs/spec/03-requests.md` §2.1].
//!
//! Both halves of that fact live here, which is what ADR-0004 requires: the
//! [`DIFF_DECODERS`] registry is the single source of truth, the request
//! encoder derives [`supported_diffs`] from it, and the value-list decoder
//! dispatches through [`decoder_for`]. Advertising a format that is not
//! compiled in, or compiling one in without advertising it, is not
//! representable.
//!
//! Everything here is a pure function of its arguments — no I/O, no async, no
//! item state — so every algorithm is testable directly against the
//! specification's own worked tables. That is also why it belongs to the
//! protocol layer rather than to the subscription state that consumes it: the
//! request encoder must be able to name the advertised set without depending on
//! a higher layer (`docs/adr/0004-supported-diffs-derived-from-decoders.md`).

use std::sync::OnceLock;

use crate::protocol::{ProtocolError, field_value_error};

// ---------------------------------------------------------------------------
// Registry (ADR-0004)
// ---------------------------------------------------------------------------

/// Whether a field value is the exact character sequence the server sent, or
/// one the client produced itself.
///
/// The distinction only matters as the **base of a later character diff**. A
/// `^T` diff is defined by character positions into the previous value
/// [Appendix D, pp.98-99], so it can only be applied to a value that is
/// character-for-character the one the server diffed against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LexicalForm {
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
    /// SPEC-AMBIGUITY (json-patch-reserialisation) on [`apply_json_patch`].
    // The `^P` decoder is the only producer, so without that feature the
    // variant is unreachable by construction — which is the point: the guard
    // it feeds still compiles and still reads the same in both builds.
    #[cfg_attr(not(feature = "json-patch"), allow(dead_code))]
    ClientDerived,
}

/// One compiled-in diff format: its wire tag, the function that applies it, and
/// how it relates to the exactness of the value it reads and writes.
pub(crate) struct DiffDecoder {
    /// The letter that follows the caret on the wire
    /// [`docs/spec/04-notifications.md` §2.2].
    pub(crate) tag: char,
    /// Applies a percent-decoded diff payload to the field's previous value.
    pub(crate) apply: fn(base: &str, payload: &str) -> Result<String, ProtocolError>,
    /// Whether the format addresses the base by character position, and
    /// therefore needs the server's own characters.
    pub(crate) needs_base_as_sent: bool,
    /// What the exactness of the produced value is.
    pub(crate) result_form: LexicalForm,
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
/// requires. Both the wire dispatch in [`decoder_for`] and the
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

/// The decoder for a wire tag, or `None` if this build did not advertise it.
///
/// The same registry [`supported_diffs`] is derived from, so a tag can never be
/// advertised without being decodable, and a tag the server invents is a typed
/// error at the call site rather than a silent passthrough (ADR-0004).
#[must_use]
pub(crate) fn decoder_for(tag: char) -> Option<&'static DiffDecoder> {
    DIFF_DECODERS.iter().find(|decoder| decoder.tag == tag)
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

    // -- Malformed payloads and base bounds ----------------------------------

    #[test]
    fn test_malformed_tlcp_diff_payloads_are_errors() {
        // Empty payload: Appendix D requires "one or more sections" [p.98].
        assert!(apply_tlcp_diff("foobar", "").is_err());
        // Truncated encoded integer: no terminating lowercase letter [p.98].
        assert!(apply_tlcp_diff("foobar", "AB").is_err());
        // Non-letter inside an encoded integer [p.98].
        assert!(apply_tlcp_diff("foobar", "3").is_err());
        // ADD declares more characters than remain in the payload [p.98].
        assert!(apply_tlcp_diff("foobar", "azz").is_err());
        // SPEC-AMBIGUITY (diff-runs-past-base): COPY past the end of the base.
        assert!(apply_tlcp_diff("foobar", "AdacAa").is_err());
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
                decoder_for(tag).is_some(),
                "advertised `{tag}` has no decoder"
            );
        }
    }

    #[test]
    fn test_a_tag_that_was_never_advertised_has_no_decoder() {
        // The dispatch and the advertisement are the same list, so a tag the
        // server invents cannot resolve (ADR-0004).
        assert!(decoder_for('Z').is_none());
        assert!(decoder_for('t').is_none());
        #[cfg(not(feature = "json-patch"))]
        assert!(decoder_for('P').is_none());
    }
}
