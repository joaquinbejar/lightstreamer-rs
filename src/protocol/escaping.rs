//! Percent-encoding and percent-decoding, the only escaping mechanism TLCP
//! defines.
//!
//! Percent-encoding is used in both directions and the two directions have
//! different rules, so this module exposes one function per direction:
//!
//! | Item | Direction | Used by |
//! |---|---|---|
//! | [`percent_decode`] | server → client | `response.rs`, `subscription/update.rs` |
//! | [`encode_form_value`] | client → server | `request.rs` |
//!
//! There is deliberately **nothing else** here. The second-level value-list
//! syntax of a real-time update (the `#` / `$` / `^` markers and the pipe
//! splitting) is *not* implemented in this module: those markers are decided
//! before any decoding happens, and the only escaping primitive that algorithm
//! needs is [`percent_decode`], which it applies to actual content and to the
//! payload of a `^`+letter diff
//! [`docs/spec/04-notifications.md` §2.3, invariant 4].
//!
//! # Sources
//!
//! - `docs/spec/01-foundations.md` §6.1.3 / §6.2.4 — what a client must escape
//!   in a request parameter value.
//! - `docs/spec/01-foundations.md` §7.2 / §7.4 — percent-encoding is the only
//!   escaping mechanism in responses and notifications, and the character set
//!   is UTF-8 for both encoded and unencoded content.
//! - `docs/spec/04-notifications.md` §2.2 — the same percent-encoding applies
//!   inside the pipe-separated field-value list.

use std::borrow::Cow;

use crate::protocol::ProtocolError;

/// A `%` was found with fewer than two bytes after it.
const REASON_TRUNCATED: &str = "`%` is not followed by two characters";

/// A `%` was found followed by something other than two hex digits.
const REASON_NOT_HEX: &str = "`%` is not followed by two hexadecimal digits";

/// The decoded byte sequence is not valid UTF-8.
const REASON_NOT_UTF8: &str = "decoded bytes are not valid utf-8";

/// Percent-decodes a response or notification argument into UTF-8 text.
///
/// Percent-encoding is the only escaping mechanism TLCP defines for the
/// downstream direction — no backslash or other escape syntax exists anywhere
/// in the response syntax [`docs/spec/01-foundations.md` §7.2]. The character
/// set is UTF-8 for both encoded and unencoded content
/// [`docs/spec/01-foundations.md` §7.1], which is why the decoded bytes are
/// validated as UTF-8 rather than mapped one-escape-to-one-character: a
/// non-ASCII character arrives as a *run* of consecutive escapes (`é` is
/// `%C3%A9`, `€` is `%E2%82%AC`) and is only meaningful once the whole run has
/// been assembled at the byte level.
///
/// The same function serves the second-level field-value syntax, where
/// percent-decoding is applied last, to actual content and to the payload of a
/// `^`+letter diff [`docs/spec/04-notifications.md` §2.3].
///
/// Returns [`Cow::Borrowed`] whenever the input contains no `%` at all — the
/// overwhelmingly common case on a live stream — so the hot path allocates
/// nothing.
///
/// # Errors
///
/// Returns [`ProtocolError::Escaping`] if a `%` is not followed by two
/// hexadecimal digits, or if the decoded bytes are not valid UTF-8. In both
/// cases `position` is a byte offset **into `input`**: the offset of the `%`
/// that starts the malformed escape, or the offset of the first byte of the
/// invalid UTF-8 sequence.
///
/// # Examples
///
/// ```text
/// "20:00:33"      -> Cow::Borrowed("20:00:33")
/// "aa%7Cbb"       -> Cow::Owned("aa|bb")
/// "%C3%A9"        -> Cow::Owned("é")
/// "%"             -> Err(Escaping { position: 0, .. })
/// ```
pub(crate) fn percent_decode(input: &str) -> Result<Cow<'_, str>, ProtocolError> {
    let bytes = input.as_bytes();

    // No escape at all: hand the caller its own buffer back untouched.
    let Some(first) = input.find('%') else {
        return Ok(Cow::Borrowed(input));
    };

    let mut decoded: Vec<u8> = Vec::with_capacity(bytes.len());
    // `first` came from `find`, so `..first` is in bounds by construction.
    if let Some(prefix) = bytes.get(..first) {
        decoded.extend_from_slice(prefix);
    }

    let mut index = first;
    while let Some(&byte) = bytes.get(index) {
        if byte != b'%' {
            decoded.push(byte);
            index += 1;
            continue;
        }

        let high = bytes.get(index + 1).copied();
        let low = bytes.get(index + 2).copied();
        let (Some(high), Some(low)) = (high, low) else {
            return Err(ProtocolError::Escaping {
                position: index,
                reason: REASON_TRUNCATED,
            });
        };
        let (Some(high), Some(low)) = (hex_value(high), hex_value(low)) else {
            return Err(ProtocolError::Escaping {
                position: index,
                reason: REASON_NOT_HEX,
            });
        };

        // `high` is at most 0x0F, so the shift cannot lose a bit.
        decoded.push((high << 4) | low);
        index += 3;
    }

    match String::from_utf8(decoded) {
        Ok(text) => Ok(Cow::Owned(text)),
        Err(error) => Err(ProtocolError::Escaping {
            // `valid_up_to` is an offset into the *decoded* buffer; the caller
            // is diagnosing a wire line, so translate it back to an offset
            // into `input`.
            position: input_offset_of_decoded_byte(input, error.utf8_error().valid_up_to()),
            reason: REASON_NOT_UTF8,
        }),
    }
}

/// Percent-encodes the value of a request parameter.
///
/// A request is a query-string-style `<param>=<value>&...` list
/// [`docs/spec/01-foundations.md` §6.1.1, §6.2.2], and reserved characters in
/// the **values** must be percent-encoded [§6.1.3 for HTTP, §6.2.4 for WS —
/// the two lists are identical].
///
/// Returns [`Cow::Borrowed`] when nothing in the input needs escaping.
///
/// # Examples
///
/// ```text
/// "S1aa6c792585db57aT1726545" -> Cow::Borrowed(..)   // nothing to escape
/// "NASDAQ-100"                -> Cow::Borrowed(..)
/// "AAPL MSFT GOOGL"           -> Cow::Owned("AAPL%20MSFT%20GOOGL")
/// "a&b=c"                     -> Cow::Owned("a%26b%3Dc")
/// ```
#[must_use]
pub(crate) fn encode_form_value(input: &str) -> Cow<'_, str> {
    if input.bytes().all(is_unreserved) {
        return Cow::Borrowed(input);
    }

    // Worst case is 3 bytes out per byte in; guessing the exact growth is not
    // worth a second pass, so reserve for the input plus a little slack.
    let mut encoded = String::with_capacity(input.len() + input.len() / 2 + 8);
    for byte in input.bytes() {
        if is_unreserved(byte) {
            encoded.push(char::from(byte));
        } else {
            encoded.push('%');
            encoded.push(hex_digit(byte >> 4));
            encoded.push(hex_digit(byte & 0x0F));
        }
    }
    Cow::Owned(encoded)
}

// SPEC-AMBIGUITY: which characters an encoder must escape is not settled by
// the spec. `docs/spec/01-foundations.md` §6.1.3 says the reserved set is
// "limited to" CR, LF, `&`, `=`, `%` and `+`, while the flag raised in that
// same section records that the chapter 2 preamble instead demands the
// (strictly larger) RFC reserved set, and the two are never reconciled — see
// the "⚠️ Spec unclear: the reserved set is stated as *limited to* ..." flag
// in §6.1.3.
//
// The defensive choice is to escape everything that is not *unreserved* per
// RFC 3986 §2.3 (`ALPHA` / `DIGIT` / `-` / `.` / `_` / `~`), which is a
// superset of both candidate sets, so the encoded value is accepted whichever
// statement is normative. §6.1.3 explicitly blesses over-encoding: "any
// further percent-encoding is supported, provided that it is based on the
// UTF-8 character set", and even names the cost we are accepting ("although
// this will introduce inefficiencies"). Since `input` is a `&str`, encoding
// per UTF-8 byte is automatic.
//
// SPEC-AMBIGUITY: `+` is listed as reserved "used for percent-encoding / URL
// Encoding", but the spec never states whether the Server decodes a literal
// `+` as a space, nor whether a client may encode a space as `+` — see the
// "⚠️ Spec unclear: `+` is listed among the reserved characters ..." flag in
// §6.1.3. This encoder never emits `+`: a space becomes `%20`, and a literal
// `+` in the value becomes `%2B`. That is unambiguous under either reading.
// Symmetrically, `percent_decode` leaves a literal `+` alone rather than
// turning it into a space: §7.2 defines percent-encoding as the *only*
// escaping mechanism in the downstream direction, so a `+` on the wire is a
// plus sign.
//
// SPEC-AMBIGUITY: the case of the hex digits is never stated, neither for the
// encoder nor for the decoder (§6.1.3 and §7.2 both say only
// "percent-encoded"). We emit uppercase, as recommended by RFC 3986 §2.1, and
// `hex_value` accepts both cases on the way in.
//
// Note that this function encodes a parameter *value* only. §6.1.3 flags that
// the spec does not say whether parameter *names* may be percent-encoded; all
// names in TLCP are `LS_`-prefixed ASCII identifiers, so `request.rs` writes
// them literally and never routes them through here.

/// Is this byte *unreserved* per RFC 3986 §2.3, i.e. safe to emit literally?
#[inline]
const fn is_unreserved(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~')
}

/// Maps a hex digit to its value, accepting either case; `None` if not a hex
/// digit.
#[inline]
const fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// Maps a nibble to its uppercase hex digit.
///
/// Every call site passes `byte >> 4` or `byte & 0x0F`, so the argument is
/// always below 16 and the arithmetic below cannot leave the ASCII range; the
/// final arm exists only to keep the function total.
#[inline]
fn hex_digit(nibble: u8) -> char {
    match nibble {
        0..=9 => char::from(b'0' + nibble),
        10..=15 => char::from(b'A' + nibble - 10),
        _ => '0',
    }
}

/// Translates an offset into the decoded byte buffer back into an offset into
/// the original input.
///
/// Only reached on the UTF-8 failure path, and only to make the error point at
/// something a reader can find in the captured wire line. It re-walks the
/// input, which is sound because a UTF-8 failure is only reported after the
/// escape scan has already succeeded: every `%` in `input` is therefore
/// followed by exactly two hex digits and consumes three input bytes to
/// produce one decoded byte.
#[cold]
fn input_offset_of_decoded_byte(input: &str, decoded_offset: usize) -> usize {
    let bytes = input.as_bytes();
    let mut produced = 0usize;
    let mut index = 0usize;
    while let Some(&byte) = bytes.get(index) {
        if produced == decoded_offset {
            return index;
        }
        index += if byte == b'%' { 3 } else { 1 };
        produced += 1;
    }
    bytes.len()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    // ---- percent_decode: the borrow path -----------------------------------

    #[test]
    fn test_percent_decode_no_escape_borrows() {
        // A value with no `%` must not allocate.
        let decoded = percent_decode("20:00:33").unwrap();
        assert!(matches!(decoded, Cow::Borrowed(_)));
        assert_eq!(decoded, "20:00:33");
    }

    #[test]
    fn test_percent_decode_empty_input_borrows() {
        // "An argument may be empty" [docs/spec/01-foundations.md §7.1].
        let decoded = percent_decode("").unwrap();
        assert!(matches!(decoded, Cow::Borrowed(_)));
        assert_eq!(decoded, "");
    }

    #[test]
    fn test_percent_decode_non_ascii_without_escape_borrows() {
        // Unencoded content is UTF-8 too [docs/spec/01-foundations.md §7.1],
        // so raw multi-byte characters pass through untouched.
        let decoded = percent_decode("Suspended — é€🚀").unwrap();
        assert!(matches!(decoded, Cow::Borrowed(_)));
        assert_eq!(decoded, "Suspended — é€🚀");
    }

    #[test]
    fn test_percent_decode_with_escape_allocates() {
        let decoded = percent_decode("aa%7Cbb").unwrap();
        assert!(matches!(decoded, Cow::Owned(_)));
    }

    // ---- percent_decode: the spec's own examples ---------------------------

    #[test]
    fn test_percent_decode_spec_pipe_example_decodes_separator() {
        // `docs/spec/04-notifications.md` §2.6, diff example 1 [p.52]:
        // the field value `aa%7Cbb` decodes to `aa|bb`.
        assert_eq!(percent_decode("aa%7Cbb").unwrap(), "aa|bb");
    }

    #[test]
    fn test_percent_decode_spec_snapshot_json_example_decodes_separator() {
        // `docs/spec/04-notifications.md` §2.6, diff example 1 [p.52]: the
        // whole `text` field of the snapshot, verbatim from the printed page.
        let wire = r#"{ "text": "aa%7Cbb", "attributes": [ { "font": "courier" }, "..." ] }"#;
        let expected = r#"{ "text": "aa|bb", "attributes": [ { "font": "courier" }, "..." ] }"#;
        assert_eq!(percent_decode(wire).unwrap(), expected);
    }

    #[test]
    fn test_percent_decode_spec_json_patch_payload_decodes_separators() {
        // `docs/spec/04-notifications.md` §2.6, diff example 2 [p.53]: the
        // payload following `^P`, which §2.3 says to percent-decode.
        let wire = r#"[{"op":"replace","path":"/text","value":"aa%7Cbb%7Ccc"},{"op":"add","path":"/attributes/1","value":"bold"}]"#;
        let expected = r#"[{"op":"replace","path":"/text","value":"aa|bb|cc"},{"op":"add","path":"/attributes/1","value":"bold"}]"#;
        assert_eq!(percent_decode(wire).unwrap(), expected);
    }

    #[test]
    fn test_percent_decode_marker_characters_decode_to_themselves() {
        // `#` (0x23), `$` (0x24) and `^` (0x5E) are percent-encoded when they
        // start actual content [docs/spec/04-notifications.md §2.2, p.51].
        assert_eq!(percent_decode("%23").unwrap(), "#");
        assert_eq!(percent_decode("%24").unwrap(), "$");
        assert_eq!(percent_decode("%5E").unwrap(), "^");
    }

    #[test]
    fn test_percent_decode_line_terminator_decodes() {
        // CR-LF is a meta character inside the value list
        // [docs/spec/04-notifications.md §2.2].
        assert_eq!(percent_decode("a%0D%0Ab").unwrap(), "a\r\nb");
    }

    // ---- percent_decode: hex digit handling --------------------------------

    #[test]
    fn test_percent_decode_lowercase_hex_decodes() {
        assert_eq!(percent_decode("aa%7cbb").unwrap(), "aa|bb");
    }

    #[test]
    fn test_percent_decode_uppercase_hex_decodes() {
        assert_eq!(percent_decode("aa%7Cbb").unwrap(), "aa|bb");
    }

    #[test]
    fn test_percent_decode_mixed_case_hex_decodes() {
        // Case may differ between the two digits of one escape, and between
        // the escapes of one multi-byte character.
        assert_eq!(percent_decode("%2c%2C").unwrap(), ",,");
        assert_eq!(percent_decode("%c3%A9").unwrap(), "é");
        assert_eq!(percent_decode("%eF%bF%bD").unwrap(), "\u{FFFD}");
    }

    #[test]
    fn test_percent_decode_escaped_percent_is_not_decoded_twice() {
        // `%25` is a percent sign; the result must not be re-scanned.
        assert_eq!(percent_decode("%25").unwrap(), "%");
        assert_eq!(percent_decode("%2525").unwrap(), "%25");
        assert_eq!(percent_decode("100%25").unwrap(), "100%");
    }

    #[test]
    fn test_percent_decode_nul_byte_decodes() {
        // 0x00 is valid UTF-8 and carries no protocol meaning.
        assert_eq!(percent_decode("a%00b").unwrap(), "a\0b");
    }

    #[test]
    fn test_percent_decode_plus_is_left_literal() {
        // SPEC-AMBIGUITY (see the `+` note above): percent-encoding is the
        // only escaping mechanism in the downstream direction
        // [docs/spec/01-foundations.md §7.2], so `+` is a plus sign, never a
        // space.
        let decoded = percent_decode("a+b").unwrap();
        assert!(matches!(decoded, Cow::Borrowed(_)));
        assert_eq!(decoded, "a+b");
        assert_eq!(percent_decode("a%2Bb").unwrap(), "a+b");
    }

    // ---- percent_decode: multi-byte UTF-8 assembly -------------------------

    #[test]
    fn test_percent_decode_two_byte_sequence_assembles() {
        assert_eq!(percent_decode("%C3%A9").unwrap(), "é");
    }

    #[test]
    fn test_percent_decode_three_byte_sequence_assembles() {
        assert_eq!(percent_decode("%E2%82%AC").unwrap(), "€");
    }

    #[test]
    fn test_percent_decode_four_byte_sequence_assembles() {
        assert_eq!(percent_decode("%F0%9F%9A%80").unwrap(), "🚀");
    }

    #[test]
    fn test_percent_decode_multibyte_between_literals_assembles() {
        // The bytes of one character arrive as consecutive escapes and must be
        // assembled before the UTF-8 check
        // [docs/spec/01-foundations.md §7.1: UTF-8 for encoded content too].
        assert_eq!(
            percent_decode("caf%C3%A9 %E2%82%AC5 %F0%9F%9A%80!").unwrap(),
            "café €5 🚀!"
        );
    }

    #[test]
    fn test_percent_decode_escaped_and_raw_multibyte_mix_assembles() {
        assert_eq!(percent_decode("é%C3%A9é").unwrap(), "ééé");
    }

    // ---- percent_decode: malformed escapes ---------------------------------

    #[test]
    fn test_percent_decode_trailing_percent_errors() {
        assert_eq!(
            percent_decode("%"),
            Err(ProtocolError::Escaping {
                position: 0,
                reason: REASON_TRUNCATED,
            })
        );
        assert_eq!(
            percent_decode("abc%"),
            Err(ProtocolError::Escaping {
                position: 3,
                reason: REASON_TRUNCATED,
            })
        );
    }

    #[test]
    fn test_percent_decode_single_hex_digit_at_end_errors() {
        assert_eq!(
            percent_decode("%7"),
            Err(ProtocolError::Escaping {
                position: 0,
                reason: REASON_TRUNCATED,
            })
        );
        assert_eq!(
            percent_decode("aa%7"),
            Err(ProtocolError::Escaping {
                position: 2,
                reason: REASON_TRUNCATED,
            })
        );
    }

    #[test]
    fn test_percent_decode_non_hex_digits_error() {
        assert_eq!(
            percent_decode("%ZZ"),
            Err(ProtocolError::Escaping {
                position: 0,
                reason: REASON_NOT_HEX,
            })
        );
        // Second digit bad only.
        assert_eq!(
            percent_decode("aa%7Gbb"),
            Err(ProtocolError::Escaping {
                position: 2,
                reason: REASON_NOT_HEX,
            })
        );
        // A `%` followed by another `%`.
        assert_eq!(
            percent_decode("%%41"),
            Err(ProtocolError::Escaping {
                position: 0,
                reason: REASON_NOT_HEX,
            })
        );
    }

    #[test]
    fn test_percent_decode_reports_position_of_second_bad_escape() {
        // The position is the offset of the offending `%`, not of the first.
        assert_eq!(
            percent_decode("%41%ZZ"),
            Err(ProtocolError::Escaping {
                position: 3,
                reason: REASON_NOT_HEX,
            })
        );
    }

    // ---- percent_decode: invalid UTF-8 -------------------------------------

    /// The error a decode of invalid UTF-8 starting at `position` produces.
    fn unsupported_utf8_err(position: usize) -> ProtocolError {
        ProtocolError::Escaping {
            position,
            reason: REASON_NOT_UTF8,
        }
    }

    #[test]
    fn test_percent_decode_truncated_utf8_sequence_errors() {
        // 0xC3 starts a two-byte sequence; 0x28 (`(`) is not a continuation.
        assert_eq!(
            percent_decode("%C3%28").unwrap_err(),
            unsupported_utf8_err(0)
        );
    }

    #[test]
    fn test_percent_decode_lone_continuation_byte_errors() {
        assert_eq!(percent_decode("%80").unwrap_err(), unsupported_utf8_err(0));
    }

    #[test]
    fn test_percent_decode_invalid_utf8_position_is_an_input_offset() {
        // `%C3%A9` decodes to two valid bytes, so `valid_up_to` is 2 in the
        // decoded buffer — but byte 2 of the *input* is inside the first
        // escape. The reported position must be 6, where the bad sequence
        // actually starts on the wire.
        assert_eq!(
            percent_decode("%C3%A9%C3%28").unwrap_err(),
            unsupported_utf8_err(6)
        );
    }

    #[test]
    fn test_percent_decode_invalid_utf8_after_literals_position_is_an_input_offset() {
        // Two literal bytes, then a lone 0xFF.
        assert_eq!(
            percent_decode("ab%FF").unwrap_err(),
            unsupported_utf8_err(2)
        );
    }

    #[test]
    fn test_percent_decode_surrogate_encoding_errors() {
        // 0xED 0xA0 0x80 is the CESU-8 encoding of a UTF-16 surrogate; it is
        // not valid UTF-8, and §7.1 mandates UTF-8.
        assert_eq!(
            percent_decode("%ED%A0%80").unwrap_err(),
            unsupported_utf8_err(0)
        );
    }

    // ---- encode_form_value: the borrow path --------------------------------

    #[test]
    fn test_encode_form_value_unreserved_only_borrows() {
        let encoded = encode_form_value("abcXYZ019-._~");
        assert!(matches!(encoded, Cow::Borrowed(_)));
        assert_eq!(encoded, "abcXYZ019-._~");
    }

    #[test]
    fn test_encode_form_value_empty_input_borrows() {
        let encoded = encode_form_value("");
        assert!(matches!(encoded, Cow::Borrowed(_)));
        assert_eq!(encoded, "");
    }

    #[test]
    fn test_encode_form_value_session_id_borrows() {
        // The session ID given verbatim by the spec
        // [docs/spec/01-foundations.md §6.1.1, p.11] is all-unreserved.
        let encoded = encode_form_value("S1aa6c792585db57aT1726545");
        assert!(matches!(encoded, Cow::Borrowed(_)));
    }

    #[test]
    fn test_encode_form_value_spec_group_name_borrows() {
        // `NASDAQ-100` [docs/spec/01-foundations.md §4.1, p.9].
        assert!(matches!(
            encode_form_value("NASDAQ-100"),
            Cow::Borrowed("NASDAQ-100")
        ));
    }

    // ---- encode_form_value: the reserved set -------------------------------

    #[test]
    fn test_encode_form_value_spec_reserved_characters_are_escaped() {
        // The set §6.1.3 / §6.2.4 states as reserved: CR, LF, `&`, `=`, `%`,
        // `+` [docs/spec/01-foundations.md §6.1.3, p.12].
        assert_eq!(encode_form_value("\r"), "%0D");
        assert_eq!(encode_form_value("\n"), "%0A");
        assert_eq!(encode_form_value("&"), "%26");
        assert_eq!(encode_form_value("="), "%3D");
        assert_eq!(encode_form_value("%"), "%25");
        assert_eq!(encode_form_value("+"), "%2B");
        assert_eq!(encode_form_value("\r\n&=%+"), "%0D%0A%26%3D%25%2B");
    }

    #[test]
    fn test_encode_form_value_space_becomes_percent_20() {
        // SPEC-AMBIGUITY (`+` as space, §6.1.3): never emit `+` for a space.
        // `AAPL MSFT GOOGL` is the spec's own second-level group syntax
        // [docs/spec/01-foundations.md §4.1, p.9].
        assert_eq!(encode_form_value("AAPL MSFT GOOGL"), "AAPL%20MSFT%20GOOGL");
    }

    #[test]
    fn test_encode_form_value_conservative_set_escapes_beyond_the_spec_list() {
        // The defensive superset: anything not unreserved per RFC 3986 §2.3.
        assert_eq!(encode_form_value(","), "%2C");
        assert_eq!(encode_form_value("|"), "%7C");
        assert_eq!(encode_form_value("#"), "%23");
        assert_eq!(encode_form_value("$"), "%24");
        assert_eq!(encode_form_value("^"), "%5E");
        assert_eq!(encode_form_value("/"), "%2F");
        assert_eq!(encode_form_value("?"), "%3F");
        assert_eq!(encode_form_value(":"), "%3A");
        assert_eq!(encode_form_value("\0"), "%00");
    }

    #[test]
    fn test_encode_form_value_hex_digits_are_uppercase() {
        // SPEC-AMBIGUITY (hex digit case): uppercase per RFC 3986 §2.1.
        let encoded = encode_form_value("~\u{7f}");
        assert_eq!(encoded, "~%7F");
        assert!(!encoded.contains('f'));
    }

    #[test]
    fn test_encode_form_value_multibyte_is_escaped_per_utf8_byte() {
        // §6.1.3: further percent-encoding is supported "provided that it is
        // based on the UTF-8 character set".
        assert_eq!(encode_form_value("é"), "%C3%A9");
        assert_eq!(encode_form_value("€"), "%E2%82%AC");
        assert_eq!(encode_form_value("🚀"), "%F0%9F%9A%80");
        assert_eq!(encode_form_value("café"), "caf%C3%A9");
    }

    #[test]
    fn test_encode_form_value_mixed_input_escapes_only_what_is_needed() {
        assert_eq!(encode_form_value("LS_data=a&b c"), "LS_data%3Da%26b%20c");
    }

    // ---- round trips -------------------------------------------------------

    #[test]
    fn test_encode_then_decode_round_trips() {
        let cases = [
            "",
            "plain",
            "NASDAQ-100",
            "AAPL MSFT GOOGL",
            "a&b=c%d+e",
            "\r\n",
            "café €5 🚀",
            "#$^|,",
            "S1aa6c792585db57aT1726545",
            "100% sure",
        ];
        for case in cases {
            let encoded = encode_form_value(case);
            let decoded = percent_decode(&encoded).unwrap();
            assert_eq!(decoded, case, "round trip failed for {case:?}");
        }
    }

    // ---- helper units ------------------------------------------------------

    #[test]
    fn test_hex_value_accepts_both_cases_and_rejects_others() {
        assert_eq!(hex_value(b'0'), Some(0));
        assert_eq!(hex_value(b'9'), Some(9));
        assert_eq!(hex_value(b'a'), Some(10));
        assert_eq!(hex_value(b'f'), Some(15));
        assert_eq!(hex_value(b'A'), Some(10));
        assert_eq!(hex_value(b'F'), Some(15));
        assert_eq!(hex_value(b'g'), None);
        assert_eq!(hex_value(b'G'), None);
        assert_eq!(hex_value(b'%'), None);
        assert_eq!(hex_value(0xC3), None);
    }

    #[test]
    fn test_hex_digit_maps_every_nibble_to_uppercase() {
        let digits: String = (0u8..16).map(hex_digit).collect();
        assert_eq!(digits, "0123456789ABCDEF");
    }

    #[test]
    fn test_is_unreserved_matches_rfc_3986_set() {
        for byte in 0u8..=255 {
            let expected = byte.is_ascii_alphanumeric() || b"-._~".contains(&byte);
            assert_eq!(is_unreserved(byte), expected, "byte {byte:#04X}");
        }
    }
}
