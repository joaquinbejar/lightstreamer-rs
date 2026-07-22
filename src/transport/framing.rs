//! Framing primitives shared by every transport adapter.
//!
//! A TLCP line is a line whether it arrived inside a WebSocket text message or
//! as a run of octets in an HTTP response body: `<tag>,<arg>,…` terminated by
//! CR-LF [`docs/spec/01-foundations.md` §7.1]. [`LineBuffer`] reassembles a
//! byte stream into those lines and hands them out one at a time, terminator
//! stripped, so the adapter above it never reasons about anything but line
//! boundaries. The URL helpers here — [`redact`] and the two scheme constants —
//! are likewise transport-agnostic: both adapters need to keep a credential out
//! of a log line and both need to recognise the `*` control link.
//!
//! Nothing in this module interprets a tag. That belongs to
//! [`crate::protocol::response`]; a framing primitive that matched on `LOOP` or
//! `CONERR` would be a layering violation.

use std::collections::VecDeque;

use super::TransportError;

/// The scheme prefix separator of an absolute URL.
pub(crate) const SCHEME_SEPARATOR: &str = "://";

/// The `<control-link>` value meaning "keep using the address this stream
/// connection was opened to" [`docs/spec/02-session-lifecycle.md` §3.1].
pub(crate) const CONTROL_LINK_SAME: &str = "*";

/// The largest run of bytes any transport will hold waiting for a line
/// terminator.
///
/// Every line is CR-LF terminated [`docs/spec/01-foundations.md` §7.1], so a
/// fragment this long is one this client will never be able to complete; a peer
/// that sends bytes and never a terminator is refused rather than allowed to
/// make the process allocate until it dies. The ceiling is deliberately far
/// above anything TLCP produces — a single update line carries one item's
/// fields [`docs/spec/04-notifications.md` §3.2] — so it is a defence against a
/// hostile or broken peer, not a protocol limit.
pub(crate) const MAX_LINE_BYTES: usize = 8 * 1024 * 1024;

// ---------------------------------------------------------------------------
// Line reassembly
// ---------------------------------------------------------------------------

/// Turns a sequence of octet runs into a sequence of TLCP lines.
///
/// Two facts force this to be a buffer rather than a `split`:
///
/// - one push may carry **several** lines — a session creation or binding
///   response is "a WS text message, **or a sequence of them**" and each line
///   is CR-LF terminated [`docs/spec/01-foundations.md` §6.2.5, §7.1], and an
///   HTTP body arrives in whatever runs the socket hands back, unrelated to
///   line boundaries;
/// - a line may straddle two pushes — a WS message boundary need not fall on a
///   line boundary, and a socket read may split a line (or even a multi-byte
///   UTF-8 character) anywhere.
///
/// A trailing fragment with no terminator is therefore held back until the rest
/// of it arrives, and discarded if the stream ends before it does (see
/// [`LineBuffer::discard_partial`]).
///
/// # Deferred decoding
///
/// Bytes accumulate as octets and a completed line is decoded to a `String`
/// only when it is [popped](LineBuffer::pop_line). Deferring the decode is what
/// makes an HTTP body — raw octets that may split a UTF-8 character across two
/// reads — safe to feed here: the split byte simply stays in the unterminated
/// fragment until its continuation arrives, because CR and LF are ASCII and so
/// a line boundary is always a UTF-8 character boundary. It also gives the
/// right ordering when a hostile server sends an invalid line after valid ones:
/// the valid lines pop first, and the invalid one surfaces as
/// [`TransportError::MalformedFrame`] in its turn rather than pre-empting them.
///
/// # Bounded by construction
///
/// The fragment held across pushes cannot exceed [`MAX_LINE_BYTES`], and
/// scanning is linear in the bytes received: each push is scanned once from
/// where the previous scan stopped, and the buffer is compacted once per push
/// rather than once per line.
//
// SPEC-AMBIGUITY: `docs/spec/01-foundations.md` §6.2.5 states that a
// creation/binding response may span a sequence of messages but does not say
// whether a *line* may span two of them. Holding an unterminated fragment is
// the defensive reading: emitting it immediately would hand a truncated line to
// the parser if the split is real, whereas holding it merely defers a line that
// §7.1 says must be CR-LF terminated anyway.
#[derive(Debug, Default)]
pub(crate) struct LineBuffer {
    /// Bytes received after the last line terminator.
    partial: Vec<u8>,
    /// How much of `partial` has already been searched for a terminator.
    /// Prevents rescanning a long fragment once per push.
    scanned: usize,
    /// Complete lines, terminator already stripped, in arrival order. Held as
    /// octets and decoded to `String` on the way out.
    ready: VecDeque<Vec<u8>>,
}

impl LineBuffer {
    /// Absorbs a run of octets.
    ///
    /// Splits on `LF` and strips a preceding `CR`. The line terminator is
    /// CR-LF [`docs/spec/01-foundations.md` §7.1], but that section flags that
    /// the spec never says whether a receiver must reject a bare LF; tolerating
    /// one costs nothing and cannot corrupt a well-formed line.
    ///
    /// # Errors
    ///
    /// [`TransportError::Capacity`] if the peer has sent more than
    /// [`MAX_LINE_BYTES`] with no terminator between them. The buffer is left
    /// cleared: there is nothing recoverable in a line that long.
    //
    // SPEC-AMBIGUITY: `docs/spec/01-foundations.md` §7.1 mandates CR-LF but
    // leaves "reject or tolerate a bare LF/CR" open. This accepts bare LF.
    //
    // SPEC-AMBIGUITY: empty lines are dropped rather than yielded. A line is
    // `<tag>,<argument1>,...` with a non-empty uppercase tag
    // [`docs/spec/01-foundations.md` §7.1], so an empty line cannot be one;
    // yielding it would push a guaranteed parse failure up to the session
    // layer. Dropping is framing, not interpretation.
    pub(crate) fn push_bytes(&mut self, bytes: &[u8]) -> Result<(), TransportError> {
        self.partial.extend_from_slice(bytes);

        // One pass over the bytes not yet looked at, and one compaction at the
        // end. Draining the front per line would rewrite the whole buffer once
        // per line, which is quadratic in a push carrying many of them.
        let mut consumed = 0;
        let mut cursor = self.scanned;
        while let Some(offset) = self
            .partial
            .get(cursor..)
            .and_then(|rest| rest.iter().position(|&byte| byte == b'\n'))
        {
            let end = cursor.saturating_add(offset);
            let raw = self.partial.get(consumed..end).unwrap_or_default();
            let line = raw.strip_suffix(b"\r").unwrap_or(raw);
            if !line.is_empty() {
                self.ready.push_back(line.to_vec());
            }
            consumed = end.saturating_add(1);
            cursor = consumed;
        }
        if consumed > 0 {
            self.partial.drain(..consumed);
        }
        self.scanned = self.partial.len();

        if self.partial.len() > MAX_LINE_BYTES {
            let held = self.partial.len();
            self.clear();
            return Err(TransportError::Capacity {
                limit_bytes: MAX_LINE_BYTES,
                reason: format!("{held} bytes were received with no line terminator"),
            });
        }
        Ok(())
    }

    /// Absorbs the payload of one already-decoded text message.
    ///
    /// A convenience for a caller — the WebSocket transport — that already
    /// holds validated UTF-8; the bytes are the same either way.
    ///
    /// # Errors
    ///
    /// As [`LineBuffer::push_bytes`].
    #[inline]
    pub(crate) fn push_message(&mut self, text: &str) -> Result<(), TransportError> {
        self.push_bytes(text.as_bytes())
    }

    /// Removes, decodes and returns the oldest complete line, if any.
    ///
    /// # Errors
    ///
    /// [`TransportError::MalformedFrame`] if the line is not valid UTF-8. "The
    /// character set is UTF-8, for both encoded and unencoded content"
    /// [`docs/spec/01-foundations.md` §7.1], so a line that is not is not TLCP.
    pub(crate) fn pop_line(&mut self) -> Option<Result<String, TransportError>> {
        let bytes = self.ready.pop_front()?;
        Some(
            String::from_utf8(bytes).map_err(|error| TransportError::MalformedFrame {
                reason: format!("a line was not valid UTF-8: {}", error.utf8_error()),
            }),
        )
    }

    /// Takes the held-back fragment as a final line, if there is one.
    ///
    /// The counterpart to [`LineBuffer::discard_partial`], for a stream whose
    /// **clean** end is a legitimate line terminator. An HTTP response body
    /// framed by `Content-Length` may end without a trailing CR-LF on its last
    /// line — "trim the trailing line terminator (**it may not be there**)"
    /// [`docs/spec/01-foundations.md` §7.4] — so when such a body is fully read
    /// the fragment is the last line, not a truncation. A WebSocket close, by
    /// contrast, is an abrupt end mid-line and uses `discard_partial`.
    ///
    /// # Errors
    ///
    /// [`TransportError::MalformedFrame`] if the fragment is not valid UTF-8,
    /// exactly as [`LineBuffer::pop_line`].
    pub(crate) fn flush_partial(&mut self) -> Option<Result<String, TransportError>> {
        self.scanned = 0;
        if self.partial.is_empty() {
            return None;
        }
        let mut raw = std::mem::take(&mut self.partial);
        if raw.last() == Some(&b'\r') {
            raw.pop();
        }
        if raw.is_empty() {
            return None;
        }
        Some(
            String::from_utf8(raw).map_err(|error| TransportError::MalformedFrame {
                reason: format!("a line was not valid UTF-8: {}", error.utf8_error()),
            }),
        )
    }

    /// Discards a held-back unterminated fragment, reporting its length if there
    /// was one.
    ///
    /// Every line "is terminated by CR-LF" [`docs/spec/01-foundations.md`
    /// §7.1], and the specification states no exception for the last line
    /// before a close. A fragment left over when the stream ends is therefore a
    /// truncated line, and promoting it to a notification would mean parsing
    /// something the wire contract says cannot occur. It is reported instead, so
    /// the session layer treats the connection as failed and recovers from the
    /// progressive it has, rather than acting on half a line.
    pub(crate) fn discard_partial(&mut self) -> Option<usize> {
        let held = self.partial.len();
        self.partial.clear();
        self.scanned = 0;
        (held > 0).then_some(held)
    }

    /// Discards everything buffered.
    pub(crate) fn clear(&mut self) {
        self.partial.clear();
        self.scanned = 0;
        self.ready.clear();
    }
}

// ---------------------------------------------------------------------------
// URL handling
// ---------------------------------------------------------------------------

/// Removes any `userinfo@` component from a URL.
///
/// A configured address of the form `wss://user:secret@host` would otherwise
/// carry a credential into every connect log line and every connect error.
#[must_use]
pub(crate) fn redact(uri: &str) -> String {
    let Some((scheme, rest)) = uri.split_once(SCHEME_SEPARATOR) else {
        return uri.to_owned();
    };
    // Userinfo, if present, precedes the first `@`, which itself precedes the
    // first `/` of the path.
    let authority_end = rest.find('/').unwrap_or(rest.len());
    let authority = rest.get(..authority_end).unwrap_or(rest);
    let path = rest.get(authority_end..).unwrap_or("");
    match authority.rsplit_once('@') {
        Some((_, host)) => format!("{scheme}{SCHEME_SEPARATOR}***@{host}{path}"),
        None => uri.to_owned(),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    /// Absorbs a push that is expected to stay within every limit.
    fn push(buffer: &mut LineBuffer, bytes: &[u8]) {
        assert!(
            buffer.push_bytes(bytes).is_ok(),
            "the push is well within the configured limits"
        );
    }

    /// The next line, asserting it decoded.
    fn line(buffer: &mut LineBuffer) -> Option<String> {
        buffer.pop_line().map(|result| result.expect("valid UTF-8"))
    }

    #[test]
    fn test_line_buffer_single_line_yields_that_line() {
        let mut buffer = LineBuffer::default();
        push(&mut buffer, b"WSOK\r\n");
        assert_eq!(line(&mut buffer).as_deref(), Some("WSOK"));
        assert!(buffer.pop_line().is_none());
    }

    /// The load-bearing case: a creation response arrives as one run carrying
    /// `CONOK` plus its head notifications
    /// [`docs/spec/02-session-lifecycle.md` §3.1].
    #[test]
    fn test_line_buffer_multi_line_push_yields_every_line_in_order() {
        let mut buffer = LineBuffer::default();
        push(
            &mut buffer,
            b"CONOK,S1aa6c792585db57aT1726545,50000,5000,*\r\n\
              SERVNAME,Lightstreamer HTTP Server\r\n\
              CLIENTIP,127.0.0.1\r\n\
              CONS,unlimited\r\n",
        );
        assert_eq!(
            line(&mut buffer).as_deref(),
            Some("CONOK,S1aa6c792585db57aT1726545,50000,5000,*")
        );
        assert_eq!(
            line(&mut buffer).as_deref(),
            Some("SERVNAME,Lightstreamer HTTP Server")
        );
        assert_eq!(line(&mut buffer).as_deref(), Some("CLIENTIP,127.0.0.1"));
        assert_eq!(line(&mut buffer).as_deref(), Some("CONS,unlimited"));
        assert!(buffer.pop_line().is_none());
    }

    #[test]
    fn test_line_buffer_line_split_across_pushes_is_reassembled() {
        let mut buffer = LineBuffer::default();
        push(&mut buffer, b"U,1,1,20.4");
        assert!(buffer.pop_line().is_none(), "no terminator yet");
        push(&mut buffer, b"2|EUR\r\nU,1,2,");
        assert_eq!(line(&mut buffer).as_deref(), Some("U,1,1,20.42|EUR"));
        assert!(buffer.pop_line().is_none());
        push(&mut buffer, b"3\r\n");
        assert_eq!(line(&mut buffer).as_deref(), Some("U,1,2,3"));
    }

    /// A multi-byte UTF-8 character split across two byte runs must not be
    /// decoded until it is whole — the case an HTTP socket read produces and a
    /// WS text frame never does.
    #[test]
    fn test_line_buffer_utf8_split_across_pushes_is_held_until_complete() {
        let mut buffer = LineBuffer::default();
        // `€` is 0xE2 0x82 0xAC. Split it down the middle across two reads.
        push(&mut buffer, b"SERVNAME,\xE2\x82");
        assert!(buffer.pop_line().is_none(), "the character is incomplete");
        push(&mut buffer, b"\xAC\r\n");
        assert_eq!(line(&mut buffer).as_deref(), Some("SERVNAME,\u{20AC}"));
    }

    #[test]
    fn test_line_buffer_bare_lf_is_tolerated() {
        let mut buffer = LineBuffer::default();
        push(&mut buffer, b"PROBE\nLOOP,0\r\n");
        assert_eq!(line(&mut buffer).as_deref(), Some("PROBE"));
        assert_eq!(line(&mut buffer).as_deref(), Some("LOOP,0"));
    }

    #[test]
    fn test_line_buffer_empty_lines_are_dropped() {
        let mut buffer = LineBuffer::default();
        push(&mut buffer, b"\r\n\r\nEND,31,bye\r\n\r\n");
        assert_eq!(line(&mut buffer).as_deref(), Some("END,31,bye"));
        assert!(buffer.pop_line().is_none());
    }

    #[test]
    fn test_line_buffer_terminated_push_leaves_no_partial() {
        let mut buffer = LineBuffer::default();
        push(&mut buffer, b"SYNC,12\r\n");
        assert_eq!(line(&mut buffer).as_deref(), Some("SYNC,12"));
        assert_eq!(buffer.discard_partial(), None);
    }

    /// Every line "is terminated by CR-LF" [`docs/spec/01-foundations.md`
    /// §7.1] and the specification states no exception for the last one before
    /// a close. An unterminated tail is therefore a truncated line, not a
    /// notification.
    #[test]
    fn test_line_buffer_unterminated_tail_is_discarded_not_promoted() {
        let mut buffer = LineBuffer::default();
        push(&mut buffer, b"LOOP,0");
        assert!(buffer.pop_line().is_none());
        assert_eq!(buffer.discard_partial(), Some("LOOP,0".len()));
        assert_eq!(buffer.discard_partial(), None);
        assert!(
            buffer.pop_line().is_none(),
            "the fragment must not become a line"
        );
    }

    /// A peer that sends bytes and never a terminator is refused rather than
    /// allowed to make this client allocate without limit.
    #[test]
    fn test_line_buffer_refuses_an_unterminated_fragment_past_its_limit() {
        let mut buffer = LineBuffer::default();
        let chunk = vec![b'x'; 1024 * 1024];
        let mut outcome = Ok(());
        // Comfortably past the ceiling, in pushes that are each acceptable.
        for _ in 0..=(MAX_LINE_BYTES / chunk.len()) {
            outcome = buffer.push_bytes(&chunk);
            if outcome.is_err() {
                break;
            }
        }
        match outcome {
            Err(TransportError::Capacity { limit_bytes, .. }) => {
                assert_eq!(limit_bytes, MAX_LINE_BYTES);
            }
            other => panic!("expected a capacity refusal, got {other:?}"),
        }
        // Nothing is retained: there is no usable line in what was refused.
        assert!(buffer.pop_line().is_none());
        assert_eq!(buffer.discard_partial(), None);
    }

    /// Many small lines in one push must stay linear: the buffer is compacted
    /// once per push, not once per line.
    #[test]
    fn test_line_buffer_handles_many_lines_in_one_push() {
        let mut buffer = LineBuffer::default();
        let mut message = Vec::new();
        for index in 0..10_000 {
            message.extend_from_slice(format!("PROBE,{index}\r\n").as_bytes());
        }
        push(&mut buffer, &message);
        for index in 0..10_000 {
            assert_eq!(line(&mut buffer), Some(format!("PROBE,{index}")));
        }
        assert!(buffer.pop_line().is_none());
        assert_eq!(buffer.discard_partial(), None);
    }

    /// A line's payload is not touched: commas and percent escapes are the
    /// parser's business [`docs/spec/01-foundations.md` §7.1, §7.2].
    #[test]
    fn test_line_buffer_preserves_line_payload_verbatim() {
        let mut buffer = LineBuffer::default();
        push(&mut buffer, b"U,3,1,a%2Cb|#|^Pxyz\r\n");
        assert_eq!(line(&mut buffer).as_deref(), Some("U,3,1,a%2Cb|#|^Pxyz"));
    }

    /// An invalid-UTF-8 line surfaces as a malformed frame, and only when it is
    /// reached — the valid line ahead of it pops cleanly first.
    #[test]
    fn test_line_buffer_invalid_utf8_line_is_malformed_after_the_valid_one() {
        let mut buffer = LineBuffer::default();
        // A valid line, then a line with a lone 0xFF continuation byte.
        push(&mut buffer, b"SYNC,1\r\n\xFF\xFE\r\n");
        assert_eq!(line(&mut buffer).as_deref(), Some("SYNC,1"));
        assert!(matches!(
            buffer.pop_line(),
            Some(Err(TransportError::MalformedFrame { .. }))
        ));
    }

    /// A body whose last line lacks CR-LF is flushed as a final line, not
    /// discarded [`docs/spec/01-foundations.md` §7.4].
    #[test]
    fn test_line_buffer_flush_partial_yields_the_unterminated_tail() {
        let mut buffer = LineBuffer::default();
        push(&mut buffer, b"REQOK,7");
        assert!(buffer.pop_line().is_none(), "no terminator yet");
        assert_eq!(
            buffer
                .flush_partial()
                .map(|result| result.expect("valid UTF-8")),
            Some("REQOK,7".to_owned())
        );
        // Idempotent: nothing left to flush.
        assert!(buffer.flush_partial().is_none());
    }

    /// A trailing CR with nothing after it flushes as an empty line, which is
    /// dropped rather than yielded.
    #[test]
    fn test_line_buffer_flush_partial_ignores_a_bare_terminator() {
        let mut buffer = LineBuffer::default();
        push(&mut buffer, b"\r");
        assert!(buffer.flush_partial().is_none());
    }

    #[test]
    fn test_line_buffer_clear_discards_everything() {
        let mut buffer = LineBuffer::default();
        push(&mut buffer, b"CONOK,S1,50000,5000,*\r\nSERVNAME");
        buffer.clear();
        assert!(buffer.pop_line().is_none());
        assert_eq!(buffer.discard_partial(), None);
    }

    #[test]
    fn test_redact_removes_userinfo() {
        assert_eq!(
            redact("wss://user:secret@push.example.com/lightstreamer"),
            "wss://***@push.example.com/lightstreamer"
        );
    }

    #[test]
    fn test_redact_leaves_a_plain_address_alone() {
        assert_eq!(
            redact("https://push.example.com/lightstreamer"),
            "https://push.example.com/lightstreamer"
        );
    }
}
