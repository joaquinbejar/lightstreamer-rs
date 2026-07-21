//! The WebSocket transport.
//!
//! WS is full-duplex, so "control requests may be sent directly on the stream
//! connection, eliminating the connection overhead of HTTP"
//! [`docs/spec/01-foundations.md` §3]. This transport therefore owns exactly
//! one socket: the stream connection *is* the control connection, and every
//! server line — notifications and control responses alike — surfaces through
//! [`Transport::next_line`] [`docs/adr/0007-transport-port-shape.md`].
//!
//! # Framing
//!
//! A request is `<request-name>` CR-LF `<parameter-line>`, and "each request
//! must be sent in a single WS message" [`docs/spec/01-foundations.md` §6.2.2].
//! In the other direction "the response will be received as a WS **text
//! message**, or a **sequence** of them in case of a session creation /
//! binding request" (§6.2.5), and every response or notification line is
//! terminated by CR-LF (§7.1). Message boundaries and line boundaries are
//! therefore *not* the same thing: one text message may carry several lines.
//! [`LineBuffer`] reassembles the byte stream and hands out one line at a
//! time, terminator stripped.
//!
//! # What this module must never do
//!
//! Interpret a tag. `WSOK`, `CONOK`, `LOOP` and `END` are all just lines here;
//! their meaning belongs to [`crate::protocol::response`] and the session
//! machine. The only server bytes this module reasons about are line
//! terminators.
//!
//! # Secrets
//!
//! `create_session` carries `LS_user` and `LS_password`
//! [`docs/spec/03-requests.md` §2.1]. No encoded request body is ever logged
//! or embedded in an error by this module — only request names and byte
//! counts. Connection targets are passed through [`redact`] first, so
//! userinfo in a configured address cannot reach a log line either.

use std::collections::VecDeque;

use futures_util::{SinkExt as _, StreamExt as _};
use tokio::net::TcpStream;
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream, connect_async_with_config,
    tungstenite::{
        Error as WsError, Message,
        client::IntoClientRequest as _,
        http::{HeaderValue, header::SEC_WEBSOCKET_PROTOCOL},
        protocol::WebSocketConfig,
    },
};

use super::{EncodedRequest, StreamOpen, Transport, TransportError, TransportProperties};
use crate::protocol::request::{TlcpRequest as _, WsOk, encode_ws_batch};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// The path of the WebSocket endpoint on a Lightstreamer server
/// [`docs/spec/01-foundations.md` §6.2.1; `docs/spec/03-requests.md` §1.2].
const STREAM_PATH: &str = "/lightstreamer";

/// The value of the `Sec-WebSocket-Protocol` header the handshake must carry
/// [`docs/spec/01-foundations.md` §6.2.1; `docs/spec/03-requests.md` §1.2].
///
/// It embeds the protocol version, which is why the WS request form has no
/// `LS_protocol` parameter of its own (§6.2, flagged ambiguity).
const SUBPROTOCOL: &str = "TLCP-2.5.0.lightstreamer.com";

/// The `<control-link>` value meaning "keep using the address this stream
/// connection was opened to" [`docs/spec/02-session-lifecycle.md` §3.1].
const CONTROL_LINK_SAME: &str = "*";

/// The scheme prefix separator of an absolute URL.
const SCHEME_SEPARATOR: &str = "://";

/// The largest WebSocket message this transport will accept, in bytes.
///
/// The specification sets no limit on a notification's length, and a server
/// that never sends a line terminator would otherwise be able to make this
/// client allocate until the process dies. The ceiling is deliberately far
/// above anything TLCP produces — `LS_content_length` bounds a whole stream
/// connection, not one message, and a single update line carries one item's
/// fields [`docs/spec/04-notifications.md` §3.2] — so it is a defence against
/// a hostile or broken peer rather than a protocol limit.
const MAX_MESSAGE_BYTES: usize = 8 * 1024 * 1024;

/// The largest run of bytes this transport will hold waiting for a line
/// terminator, in bytes.
///
/// Distinct from [`MAX_MESSAGE_BYTES`] because a fragment may legitimately
/// straddle messages: it is the total held across them, not within one. Every
/// line is CR-LF terminated [`docs/spec/01-foundations.md` §7.1], so a
/// fragment this long is not one this client will ever be able to complete.
const MAX_PARTIAL_LINE_BYTES: usize = MAX_MESSAGE_BYTES;

// ---------------------------------------------------------------------------
// Line reassembly
// ---------------------------------------------------------------------------

/// Turns a sequence of WS text messages into a sequence of TLCP lines.
///
/// Two facts force this to be a buffer rather than a `split`:
///
/// - a single text message may carry **several** lines — the spec says a
///   session creation or binding response is "a WS text message, **or a
///   sequence of them**" and each line is CR-LF terminated
///   [`docs/spec/01-foundations.md` §6.2.5, §7.1], so `CONOK`, `SERVNAME`,
///   `CLIENTIP` and `CONS` may all arrive in one message;
/// - the spec never promises that a message boundary falls *on* a line
///   boundary, so a line may in principle straddle two messages.
///
/// A trailing fragment with no terminator is therefore held back until the
/// rest of it arrives, and discarded if the connection ends before it does
/// (see [`LineBuffer::discard_partial`]).
///
/// # Bounded by construction
///
/// Everything a peer controls here is capped. The fragment held across
/// messages cannot exceed [`MAX_PARTIAL_LINE_BYTES`], so a server that sends
/// bytes and never a terminator is refused rather than allowed to allocate
/// until the process dies. Scanning is linear in the bytes received, not
/// quadratic: each message is scanned once from where the previous scan
/// stopped, and the buffer is compacted once per message rather than once per
/// line.
//
// SPEC-AMBIGUITY: `docs/spec/01-foundations.md` §6.2.5 states that a
// creation/binding response may span a sequence of text messages but does not
// say whether a *line* may span two of them. Holding an unterminated fragment
// is the defensive reading: emitting it immediately would hand a truncated
// line to the parser if the split is real, whereas holding it merely defers a
// line that §7.1 says must be CR-LF terminated anyway.
#[derive(Debug, Default)]
struct LineBuffer {
    /// Bytes received after the last line terminator.
    partial: String,
    /// How much of `partial` has already been searched for a terminator.
    /// Prevents rescanning a long fragment once per message.
    scanned: usize,
    /// Complete lines, terminator already stripped, in arrival order.
    ready: VecDeque<String>,
}

impl LineBuffer {
    /// Absorbs the payload of one WS text message.
    ///
    /// Splits on `LF` and strips a preceding `CR`. The line terminator is
    /// CR-LF [`docs/spec/01-foundations.md` §7.1], but that section flags that
    /// the spec never says whether a receiver must reject a bare LF; tolerating
    /// one costs nothing and cannot corrupt a well-formed line.
    ///
    /// # Errors
    ///
    /// [`TransportError::Capacity`] if the peer has sent more than
    /// [`MAX_PARTIAL_LINE_BYTES`] with no terminator between them. The buffer
    /// is left cleared: there is nothing recoverable in a line that long.
    //
    // SPEC-AMBIGUITY: `docs/spec/01-foundations.md` §7.1 mandates CR-LF but
    // leaves "reject or tolerate a bare LF/CR" open. This accepts bare LF.
    //
    // SPEC-AMBIGUITY: empty lines are dropped rather than yielded. A line is
    // `<tag>,<argument1>,...` with a non-empty uppercase tag
    // [`docs/spec/01-foundations.md` §7.1], so an empty line cannot be one;
    // yielding it would push a guaranteed parse failure up to the session
    // layer. Dropping is framing, not interpretation.
    fn push_message(&mut self, text: &str) -> Result<(), TransportError> {
        self.partial.push_str(text);

        // One pass over the bytes that have not been looked at yet, and one
        // compaction at the end. Draining the front per line would rewrite the
        // whole buffer once per line, which is quadratic in a message carrying
        // many of them.
        let mut consumed = 0;
        let mut cursor = self.scanned;
        while let Some(offset) = self.partial.get(cursor..).and_then(|rest| rest.find('\n')) {
            let end = cursor.saturating_add(offset);
            let raw = self.partial.get(consumed..end).unwrap_or_default();
            let line = raw.strip_suffix('\r').unwrap_or(raw);
            if !line.is_empty() {
                self.ready.push_back(line.to_owned());
            }
            consumed = end.saturating_add(1);
            cursor = consumed;
        }
        if consumed > 0 {
            self.partial.drain(..consumed);
        }
        self.scanned = self.partial.len();

        if self.partial.len() > MAX_PARTIAL_LINE_BYTES {
            let held = self.partial.len();
            self.clear();
            return Err(TransportError::Capacity {
                limit_bytes: MAX_PARTIAL_LINE_BYTES,
                reason: format!("{held} bytes were received with no line terminator"),
            });
        }
        Ok(())
    }

    /// Removes and returns the oldest complete line, if any.
    fn pop_line(&mut self) -> Option<String> {
        self.ready.pop_front()
    }

    /// Discards a held-back unterminated fragment, reporting whether there was
    /// one.
    ///
    /// Every line "is terminated by CR-LF" [`docs/spec/01-foundations.md`
    /// §7.1], and the specification states no exception for the last line
    /// before a close — not for a clean close and not for an abrupt one. A
    /// fragment left over when the connection ends is therefore a truncated
    /// line, and promoting it to a notification would mean parsing something
    /// the wire contract says cannot occur. It is reported instead, so the
    /// session layer treats the connection as failed and recovers from the
    /// progressive it has, rather than acting on half a line.
    fn discard_partial(&mut self) -> Option<usize> {
        let held = self.partial.len();
        self.partial.clear();
        self.scanned = 0;
        (held > 0).then_some(held)
    }

    /// Discards everything buffered.
    fn clear(&mut self) {
        self.partial.clear();
        self.scanned = 0;
        self.ready.clear();
    }
}

// ---------------------------------------------------------------------------
// Message classification
// ---------------------------------------------------------------------------

/// What one received WS message meant to this transport.
#[derive(Debug, PartialEq, Eq)]
enum Reception {
    /// A text message whose lines are now in the buffer.
    Buffered,
    /// A frame this transport deliberately ignores.
    Ignored,
    /// The peer started the closing handshake.
    PeerClosed {
        /// The close frame rendered for logging, if the peer sent one. Server
        /// supplied and free of client credentials.
        description: Option<String>,
    },
}

/// Classifies one received message, buffering its lines when it carries any.
///
/// # Errors
///
/// [`TransportError::MalformedFrame`] for anything that is not text: "the WS
/// messages used to carry requests must be of **text** (as opposed to binary)
/// type, hence they must be based on the UTF-8 character set"
/// [`docs/spec/01-foundations.md` §6.2.4], and responses come back as text
/// messages (§6.2.5). A binary message is not TLCP.
fn receive(buffer: &mut LineBuffer, message: Message) -> Result<Reception, TransportError> {
    match message {
        Message::Text(text) => {
            buffer.push_message(text.as_str())?;
            Ok(Reception::Buffered)
        }
        // Ping and pong are transport plumbing, not TLCP. `tungstenite` has
        // already queued the mandatory pong reply for us — see the comment on
        // `WsTransport::next_line`.
        Message::Ping(_) | Message::Pong(_) => Ok(Reception::Ignored),
        Message::Close(frame) => Ok(Reception::PeerClosed {
            description: frame.map(|frame| frame.to_string()),
        }),
        Message::Binary(payload) => Err(TransportError::MalformedFrame {
            reason: format!(
                "binary websocket message of {} bytes; TLCP is text only",
                payload.len()
            ),
        }),
        // Only produced by the raw-frame API, which this transport never uses.
        Message::Frame(_) => Err(TransportError::MalformedFrame {
            reason: "raw websocket frame".to_owned(),
        }),
    }
}

/// Translates a `tungstenite` failure into the port's error taxonomy.
///
/// The distinction that matters is clean-versus-abrupt: an abrupt end is
/// transition T5, after which "the session is not closed, hence the Client is
/// still allowed to issue a rebind"
/// [`docs/spec/02-session-lifecycle.md` §2.2]. `tokio-tungstenite` reports a
/// completed closing handshake by ending its stream, never as an error, so
/// every error reaching here is either a bad frame or a lost connection.
#[cold]
#[inline(never)]
fn map_ws_error(error: &WsError) -> TransportError {
    match error {
        // A text frame whose bytes are not valid UTF-8. §6.2.4 requires UTF-8.
        WsError::Utf8(reason) => TransportError::MalformedFrame {
            reason: format!("invalid UTF-8 in text message: {reason}"),
        },
        other => TransportError::ConnectionLost {
            reason: other.to_string(),
        },
    }
}

/// Whether a failure leaves the socket unusable.
///
/// A capacity violation rejects one message; everything else here has either
/// torn the connection down or violated the WebSocket protocol on it, and RFC
/// 6455 requires failing the connection in that case.
#[must_use]
fn is_fatal(error: &WsError) -> bool {
    !matches!(error, WsError::Capacity(_) | WsError::WriteBufferFull(_))
}

// ---------------------------------------------------------------------------
// URL handling
// ---------------------------------------------------------------------------

/// Builds the error for an address this transport cannot use.
#[cold]
#[inline(never)]
fn connect_error(target: &str, reason: impl Into<String>) -> TransportError {
    TransportError::Connect {
        target: target.to_owned(),
        source: Box::new(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            reason.into(),
        )),
    }
}

/// Removes any `userinfo@` component from a URL.
///
/// A configured address of the form `wss://user:secret@host` would otherwise
/// carry a credential into every connect log line and every connect error.
#[must_use]
fn redact(uri: &str) -> String {
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

/// Derives the WebSocket URI to connect to.
///
/// The endpoint is the configured server address with the path
/// `/lightstreamer` [`docs/spec/01-foundations.md` §6.2.1]. `http`/`https` are
/// accepted as synonyms of `ws`/`wss` because a Lightstreamer server address is
/// conventionally written that way; the scheme must be explicit, because
/// guessing it would mean guessing whether the link is encrypted.
///
/// When a control link is set it replaces the **authority** only
/// [`docs/spec/02-session-lifecycle.md` §3.1].
///
/// # Errors
///
/// [`TransportError::Connect`] if the address carries no scheme or a scheme
/// this transport cannot speak.
//
// SPEC-AMBIGUITY: `docs/spec/02-session-lifecycle.md` §3.1 flags that
// `<control-link>` is "the address (IP address, or hostname, and port)" with no
// grammar, and that the spec never says whether the scheme or path is
// inherited from the original connection. This inherits both, and strips a
// scheme the control link may carry, so that a server-supplied value can never
// downgrade a `wss` session to plaintext `ws`.
fn stream_uri(address: &str, control_link: Option<&str>) -> Result<String, TransportError> {
    let address = address.trim();
    let redacted = redact(address);
    let Some((scheme, rest)) = address.split_once(SCHEME_SEPARATOR) else {
        return Err(connect_error(
            &redacted,
            "server address must start with ws://, wss://, http:// or https://",
        ));
    };
    let scheme = match scheme.to_ascii_lowercase().as_str() {
        "ws" | "http" => "ws",
        "wss" | "https" => "wss",
        other => {
            return Err(connect_error(
                &redacted,
                format!("unsupported scheme `{other}`; expected ws, wss, http or https"),
            ));
        }
    };

    let authority = match control_link {
        Some(link) if !link.trim().is_empty() && link.trim() != CONTROL_LINK_SAME => {
            let link = link.trim();
            // Inherit the scheme: use only what follows one the link may carry.
            link.split_once(SCHEME_SEPARATOR)
                .map_or(link, |(_, rest)| rest)
        }
        _ => rest,
    };
    let authority = authority.trim_end_matches('/');
    if authority.is_empty() {
        return Err(connect_error(&redacted, "server address has no host"));
    }

    // Tolerate an address that already names the endpoint.
    if authority.ends_with(STREAM_PATH) {
        Ok(format!("{scheme}{SCHEME_SEPARATOR}{authority}"))
    } else {
        Ok(format!(
            "{scheme}{SCHEME_SEPARATOR}{authority}{STREAM_PATH}"
        ))
    }
}

// ---------------------------------------------------------------------------
// The transport
// ---------------------------------------------------------------------------

/// The socket type `connect_async` hands back: TCP, wrapped in TLS for `wss`.
type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// A TLCP transport over a single WebSocket.
///
/// Owns one socket at a time and no spawned tasks: every await point is driven
/// by the caller through [`Transport::next_line`], [`Transport::send_control`]
/// or [`Transport::close`]. Dropping the transport drops the socket and so
/// closes the underlying connection; it cannot perform the closing handshake
/// from `Drop`, which is why callers should reach [`Transport::close`] on the
/// ordered shutdown path.
#[derive(Debug)]
pub(crate) struct WsTransport {
    /// The configured server address, scheme included.
    address: String,
    /// The control link last supplied by the server, if any. Affects only the
    /// establishment of a *new* socket
    /// [`docs/spec/02-session-lifecycle.md` §3.1].
    control_link: Option<String>,
    /// The live socket, absent before the first open and after any close.
    socket: Option<Socket>,
    /// Lines received but not yet yielded.
    buffer: LineBuffer,
    /// A complaint about a line the peer left unterminated, held until every
    /// line that *was* complete has been yielded.
    truncated: Option<TransportError>,
}

impl WsTransport {
    /// Creates a transport for a server address such as
    /// `wss://push.example.com` or `https://push.example.com:443`.
    ///
    /// # Errors
    ///
    /// [`TransportError::Connect`] if the address cannot be turned into a
    /// WebSocket endpoint — see [`stream_uri`]. Validating here means a
    /// mistyped address fails at construction rather than at the first
    /// reconnect attempt.
    #[must_use = "a transport does nothing until a stream is opened"]
    pub(crate) fn try_new(address: impl Into<String>) -> Result<Self, TransportError> {
        let address = address.into();
        // Validate eagerly; the result is recomputed on every connect because
        // the control link may change in between.
        stream_uri(&address, None)?;
        Ok(Self {
            address,
            control_link: None,
            socket: None,
            buffer: LineBuffer::default(),
            truncated: None,
        })
    }

    /// Establishes the WebSocket and issues the establishment check.
    ///
    /// The handshake uses the URI and subprotocol of
    /// [`docs/spec/01-foundations.md` §6.2.1]. `wsok` follows immediately:
    /// it "is only allowed on a WebSocket and it is typically expected to be
    /// the first one issued on the WebSocket", and its response "is guaranteed
    /// to precede the responses of any other subsequent requests"
    /// [`docs/spec/03-requests.md` §15]. That guarantee is why this does not
    /// wait for `WSOK`: the session request may follow at once, and the `WSOK`
    /// line will still arrive first through [`Transport::next_line`].
    async fn connect(&mut self) -> Result<(), TransportError> {
        let uri = stream_uri(&self.address, self.control_link.as_deref())?;
        let target = redact(&uri);

        let mut request =
            uri.as_str()
                .into_client_request()
                .map_err(|error| TransportError::Connect {
                    target: target.clone(),
                    source: Box::new(error),
                })?;
        request.headers_mut().insert(
            SEC_WEBSOCKET_PROTOCOL,
            HeaderValue::from_static(SUBPROTOCOL),
        );

        tracing::debug!(target = %target, subprotocol = SUBPROTOCOL, "opening TLCP websocket");
        // The frame and message ceilings are the peer-controlled dimensions
        // this transport refuses to let a server choose for it; see
        // [`MAX_MESSAGE_BYTES`].
        let config = WebSocketConfig::default()
            .max_message_size(Some(MAX_MESSAGE_BYTES))
            .max_frame_size(Some(MAX_MESSAGE_BYTES));
        let (socket, response) = connect_async_with_config(request, Some(config), false)
            .await
            .map_err(|error| TransportError::Connect {
                target: target.clone(),
                source: Box::new(error),
            })?;

        // The subprotocol is not decoration: it "embeds the protocol version",
        // which is why the WS request form carries no `LS_protocol` parameter
        // of its own [`docs/spec/01-foundations.md` §6.2.1;
        // `docs/spec/03-requests.md` §1.2]. A handshake that did not negotiate
        // it agreed to no version of TLCP, so there is nothing this client can
        // safely assume about what comes back on it.
        //
        // Both refusals are the same refusal. RFC 6455 §4.1 lets a server
        // answer with one of the offered subprotocols or with none, and
        // requires a client that needs one to fail the connection when it does
        // not get it — which is this client, since the specification states the
        // header as part of the WS request and gives no alternative way to
        // agree a version. Continuing without it would be a compatibility
        // behaviour with no source behind it.
        match response.headers().get(SEC_WEBSOCKET_PROTOCOL) {
            Some(value) if value.as_bytes() == SUBPROTOCOL.as_bytes() => {}
            Some(other) => {
                return Err(connect_error(
                    &target,
                    format!(
                        "server negotiated subprotocol `{}`, expected `{SUBPROTOCOL}`",
                        String::from_utf8_lossy(other.as_bytes())
                    ),
                ));
            }
            None => {
                return Err(connect_error(
                    &target,
                    format!("server negotiated no subprotocol, expected `{SUBPROTOCOL}`"),
                ));
            }
        }

        self.socket = Some(socket);
        self.send_message(WsOk::NAME, WsOk::ws_message().to_owned())
            .await
    }

    /// Sends one already-framed request as a single WS text message.
    ///
    /// "Each request must be sent in a single WS message"
    /// [`docs/spec/01-foundations.md` §6.2.2].
    ///
    /// `message` is never logged and never reaches an error: for
    /// `create_session` it contains `LS_password`
    /// [`docs/spec/03-requests.md` §2.1].
    async fn send_message(
        &mut self,
        name: &'static str,
        message: String,
    ) -> Result<(), TransportError> {
        let bytes = message.len();
        let socket = self.socket.as_mut().ok_or_else(|| TransportError::Send {
            name,
            reason: "no websocket is open".to_owned(),
        })?;

        match socket.send(Message::text(message)).await {
            Ok(()) => {
                tracing::trace!(request = name, bytes, "sent TLCP request");
                Ok(())
            }
            Err(error) => {
                let reason = error.to_string();
                if is_fatal(&error) {
                    // The socket is gone; buffered lines are kept, because
                    // they were legitimately received before the failure.
                    self.socket = None;
                }
                Err(TransportError::Send { name, reason })
            }
        }
    }

    /// Drops the socket without a handshake, after an unrecoverable failure.
    fn abandon_socket(&mut self) {
        self.socket = None;
        // A trailing fragment cannot be trusted after an abrupt end: it may be
        // a line the server was still writing.
        self.buffer.clear();
        self.truncated = None;
    }

    /// Completes the closing handshake the peer started, then forgets the
    /// socket.
    ///
    /// `close` on the sink flushes the close frame `tungstenite` queued when it
    /// read the peer's; it does not read, so it cannot block waiting for a
    /// peer that has already said everything it is going to say.
    ///
    /// Returns the malformed-fragment error, if the peer left a line
    /// unterminated. See [`LineBuffer::discard_partial`].
    async fn acknowledge_peer_close(&mut self) -> Option<TransportError> {
        if let Some(mut socket) = self.socket.take()
            && let Err(error) = socket.close(None).await
        {
            tracing::debug!(%error, "closing handshake did not complete");
        }
        self.truncated_line()
    }

    /// Reports a line the peer left unterminated when the connection ended.
    fn truncated_line(&mut self) -> Option<TransportError> {
        let held = self.buffer.discard_partial()?;
        tracing::warn!(
            bytes = held,
            "the connection ended on a line with no CR-LF; discarding it"
        );
        Some(TransportError::MalformedFrame {
            reason: format!("the connection ended mid-line, after {held} bytes with no CR-LF"),
        })
    }
}

impl Transport for WsTransport {
    fn properties(&self) -> TransportProperties {
        TransportProperties {
            // "WS is full-duplex, so control requests may be sent directly on
            // the stream connection" [`docs/spec/01-foundations.md` §3;
            // `docs/spec/02-session-lifecycle.md` §1], and the response comes
            // back "on the stream connection" (§5.2).
            control_shares_stream: true,
            // Content-length termination is the HTTP mechanism behind
            // transition T3 [`docs/spec/02-session-lifecycle.md` §2.2]; on WS
            // the server "sends the Loop command exactly as in the HTTP case,
            // but it does not close the connection" (§4.3).
            ends_on_content_length: false,
            // Polling on WS is a per-request choice (`LS_polling` on
            // `create_session`/`bind_session`), not a property of the
            // transport: "this communication mode is transport independent and
            // may also be applied to WS under particular circumstances"
            // [`docs/spec/02-session-lifecycle.md` §7.1]. The transport itself
            // never ends a cycle on its own.
            is_polling: false,
        }
    }

    /// Records the control link for the next socket this transport opens.
    ///
    /// It deliberately has **no effect on control requests**. On WS control
    /// requests travel on the stream connection
    /// [`docs/spec/01-foundations.md` §3;
    /// `docs/spec/02-session-lifecycle.md` §1] and their responses come back on
    /// it [`docs/spec/01-foundations.md` §5.2], so there is no second endpoint
    /// to redirect.
    ///
    /// It is not a no-op, though. The control link is "the address … to which
    /// every following session rebind and control request must be sent to,
    /// **in case this requires the establishment of a new socket (possibly a
    /// websocket)**" [`docs/spec/02-session-lifecycle.md` §3.1] — so the next
    /// [`Transport::open_stream`] that has to dial gets it, which is what
    /// preserves session affinity in a cluster. A rebind that reuses the live
    /// socket is by definition not establishing one and is unaffected.
    ///
    /// `None` and the special value `*` — "the client should send all the
    /// session rebind and control requests to the same address to which it
    /// opened this stream connection" (§3.1) — both restore the configured
    /// address.
    fn set_control_link(&mut self, host: Option<&str>) {
        self.control_link = match host {
            Some(host) if !host.trim().is_empty() && host.trim() != CONTROL_LINK_SAME => {
                Some(host.trim().to_owned())
            }
            _ => None,
        };
        tracing::debug!(
            control_link = self
                .control_link
                .as_deref()
                .unwrap_or("<configured address>"),
            "control link recorded; applies to the next websocket established"
        );
    }

    /// Opens the stream connection and sends the creation or bind request.
    ///
    /// A `bind_session` reuses the live socket when there is one: on WS the
    /// server "does not close the connection" on `LOOP` and "the client is free
    /// to choose whether to rebind the session to the same stream connection or
    /// open a new one" [`docs/spec/02-session-lifecycle.md` §4.3]. Reusing it
    /// is what makes WS long polling (§7) affordable.
    ///
    /// A `create_session` always dials afresh. "As long as a session is bound
    /// to a WS, no other session can be bound"
    /// [`docs/spec/01-foundations.md` §3], and this transport cannot know
    /// whether the previous session was unbound.
    //
    // SPEC-AMBIGUITY: `docs/spec/01-foundations.md` §3 forbids binding a second
    // session to a WS that still has one bound, but does not say how a client
    // may legitimately create a new session on an existing WS. Opening a new
    // socket satisfies the invariant unconditionally.
    async fn open_stream(&mut self, request: StreamOpen) -> Result<(), TransportError> {
        // Encode first: a request that cannot be framed must not leave a
        // half-open socket behind.
        let (name, message) = match &request {
            StreamOpen::Create(create) => {
                let message = create.ws_message().map_err(|_| TransportError::Send {
                    name: crate::protocol::request::CreateSession::NAME,
                    // Deliberately opaque. This request carries `LS_password`
                    // [`docs/spec/03-requests.md` §2.1] and an encoder message
                    // may quote the offending value.
                    reason: "session creation parameters cannot be encoded".to_owned(),
                })?;
                (crate::protocol::request::CreateSession::NAME, message)
            }
            StreamOpen::Bind(bind) => {
                let message = bind.ws_message().map_err(|error| TransportError::Send {
                    name: crate::protocol::request::BindSession::NAME,
                    // `bind_session` accepts no credentials
                    // [`docs/spec/03-requests.md` §3], so the encoder's own
                    // message is safe to surface.
                    reason: error.to_string(),
                })?;
                (crate::protocol::request::BindSession::NAME, message)
            }
        };

        let reuse = self.socket.is_some() && matches!(request, StreamOpen::Bind(_));
        if !reuse {
            if let Some(mut socket) = self.socket.take()
                && let Err(error) = socket.close(None).await
            {
                tracing::debug!(%error, "could not close the previous websocket cleanly");
            }
            // Lines belonging to the previous connection must not be read as
            // the new session's.
            self.buffer.clear();
            self.truncated = None;
            self.connect().await?;
        } else {
            tracing::debug!("rebinding on the live websocket");
        }

        self.send_message(name, message).await
    }

    /// Yields the next TLCP line.
    ///
    /// Ping frames need no work here: `tungstenite` queues the mandatory pong
    /// itself when it reads a ping and flushes it on the next read or write —
    /// its `WebSocketContext::read` is documented as sending "pong and close
    /// responses automatically … you should not respond to ping frames
    /// manually", and `tokio_tungstenite::WebSocketStream::poll_next` is a thin
    /// wrapper over that `read`. Verified against `tungstenite` 0.29 sources
    /// (`src/protocol/mod.rs`), not assumed. The consequence for callers is
    /// that pings are answered only while `next_line` is being polled, which is
    /// the session layer's steady state.
    ///
    /// Returns `None` for a clean close and
    /// [`TransportError::ConnectionLost`] for an abrupt one — the latter is
    /// transition T5, after which recovery is still permitted
    /// [`docs/spec/02-session-lifecycle.md` §2.2].
    async fn next_line(&mut self) -> Option<Result<String, TransportError>> {
        loop {
            if let Some(line) = self.buffer.pop_line() {
                return Some(Ok(line));
            }
            // Complete lines first, then the complaint about the incomplete
            // one, then the end of the stream.
            if let Some(error) = self.truncated.take() {
                return Some(Err(error));
            }
            // No socket and no buffered line: the stream has ended.
            let socket = self.socket.as_mut()?;

            match socket.next().await {
                Some(Ok(message)) => match receive(&mut self.buffer, message) {
                    Ok(Reception::Buffered | Reception::Ignored) => {}
                    Ok(Reception::PeerClosed { description }) => {
                        tracing::debug!(
                            close = description.as_deref().unwrap_or("<no close frame>"),
                            "peer closed the websocket"
                        );
                        // Lines already complete when the peer closed are
                        // still delivered, ahead of any complaint about a
                        // fragment that never was.
                        if let Some(error) = self.acknowledge_peer_close().await {
                            self.truncated = Some(error);
                        }
                    }
                    Err(error) => {
                        self.abandon_socket();
                        return Some(Err(error));
                    }
                },
                Some(Err(error)) => {
                    let error = map_ws_error(&error);
                    self.abandon_socket();
                    return Some(Err(error));
                }
                // `tokio-tungstenite` ends its stream only once the closing
                // handshake has completed; an abrupt end surfaces as an error
                // above.
                None => {
                    self.socket = None;
                    if let Some(error) = self.truncated_line() {
                        self.truncated = Some(error);
                    }
                }
            }
        }
    }

    /// Sends a control request on the stream connection.
    ///
    /// Framed with [`encode_ws_batch`], which produces the `<request-name>`
    /// CR-LF `<parameter-line>` form of a single request and the identical
    /// shape of a one-line batch [`docs/spec/01-foundations.md` §6.2.2,
    /// §6.2.3].
    ///
    /// No response is returned: it arrives on this same connection
    /// [`docs/spec/01-foundations.md` §5.2], interspersed with notifications
    /// (§6.2), and is correlated by `LS_reqId` by the session layer.
    async fn send_control(&mut self, request: EncodedRequest) -> Result<(), TransportError> {
        let message = encode_ws_batch(request.name, std::slice::from_ref(&request.parameters))
            .map_err(|error| TransportError::Send {
                name: request.name,
                // The only failure `encode_ws_batch` has is an empty batch; it
                // quotes no parameter value.
                reason: error.to_string(),
            })?;
        self.send_message(request.name, message).await
    }

    /// Performs the WebSocket closing handshake and releases the socket.
    ///
    /// # Errors
    ///
    /// [`TransportError::ConnectionLost`] if the close frame could not be
    /// written. The socket is released either way, so the call is idempotent
    /// and leaves nothing behind.
    async fn close(&mut self) -> Result<(), TransportError> {
        self.buffer.clear();
        self.truncated = None;
        let Some(mut socket) = self.socket.take() else {
            return Ok(());
        };
        tracing::debug!("closing the TLCP websocket");
        match socket.close(None).await {
            // `poll_close` already reports an already-closed connection as
            // success; anything else means the frame never left.
            Ok(()) => Ok(()),
            Err(error) => Err(TransportError::ConnectionLost {
                reason: error.to_string(),
            }),
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    // `tungstenite`'s server handshake callback returns its own large
    // `ErrorResponse` type; the shape is not ours to change.
    #![allow(clippy::result_large_err)]

    use super::*;
    use crate::protocol::request::PROTOCOL_VERSION;
    use std::net::SocketAddr;
    use tokio::net::TcpListener;
    use tokio_tungstenite::{
        accept_hdr_async,
        tungstenite::handshake::server::{Request as ServerRequest, Response as ServerResponse},
    };

    // -- line reassembly ----------------------------------------------------

    /// Absorbs a message that is expected to stay within every limit.
    fn push(buffer: &mut LineBuffer, text: &str) {
        assert!(
            buffer.push_message(text).is_ok(),
            "the message is well within the configured limits"
        );
    }

    #[test]
    fn test_line_buffer_single_line_yields_that_line() {
        let mut buffer = LineBuffer::default();
        push(&mut buffer, "WSOK\r\n");
        assert_eq!(buffer.pop_line().as_deref(), Some("WSOK"));
        assert_eq!(buffer.pop_line(), None);
    }

    /// The load-bearing case: a creation response arrives as one text message
    /// carrying `CONOK` plus its head notifications
    /// [`docs/spec/02-session-lifecycle.md` §3.1].
    #[test]
    fn test_line_buffer_multi_line_message_yields_every_line_in_order() {
        let mut buffer = LineBuffer::default();
        push(
            &mut buffer,
            "CONOK,S1aa6c792585db57aT1726545,50000,5000,*\r\n\
             SERVNAME,Lightstreamer HTTP Server\r\n\
             CLIENTIP,127.0.0.1\r\n\
             CONS,unlimited\r\n",
        );
        assert_eq!(
            buffer.pop_line().as_deref(),
            Some("CONOK,S1aa6c792585db57aT1726545,50000,5000,*")
        );
        assert_eq!(
            buffer.pop_line().as_deref(),
            Some("SERVNAME,Lightstreamer HTTP Server")
        );
        assert_eq!(buffer.pop_line().as_deref(), Some("CLIENTIP,127.0.0.1"));
        assert_eq!(buffer.pop_line().as_deref(), Some("CONS,unlimited"));
        assert_eq!(buffer.pop_line(), None);
    }

    #[test]
    fn test_line_buffer_line_split_across_messages_is_reassembled() {
        let mut buffer = LineBuffer::default();
        push(&mut buffer, "U,1,1,20.4");
        assert_eq!(buffer.pop_line(), None, "no terminator yet");
        push(&mut buffer, "2|EUR\r\nU,1,2,");
        assert_eq!(buffer.pop_line().as_deref(), Some("U,1,1,20.42|EUR"));
        assert_eq!(buffer.pop_line(), None);
        push(&mut buffer, "3\r\n");
        assert_eq!(buffer.pop_line().as_deref(), Some("U,1,2,3"));
    }

    #[test]
    fn test_line_buffer_bare_lf_is_tolerated() {
        let mut buffer = LineBuffer::default();
        push(&mut buffer, "PROBE\nLOOP,0\r\n");
        assert_eq!(buffer.pop_line().as_deref(), Some("PROBE"));
        assert_eq!(buffer.pop_line().as_deref(), Some("LOOP,0"));
    }

    #[test]
    fn test_line_buffer_empty_lines_are_dropped() {
        let mut buffer = LineBuffer::default();
        push(&mut buffer, "\r\n\r\nEND,31,bye\r\n\r\n");
        assert_eq!(buffer.pop_line().as_deref(), Some("END,31,bye"));
        assert_eq!(buffer.pop_line(), None);
    }

    #[test]
    fn test_line_buffer_terminated_message_leaves_no_partial() {
        let mut buffer = LineBuffer::default();
        push(&mut buffer, "SYNC,12\r\n");
        assert_eq!(buffer.pop_line().as_deref(), Some("SYNC,12"));
        assert_eq!(buffer.discard_partial(), None);
    }

    /// C-15. Every line "is terminated by CR-LF"
    /// [`docs/spec/01-foundations.md` §7.1] and the specification states no
    /// exception for the last one before a close. An unterminated tail is
    /// therefore a truncated line, not a notification.
    #[test]
    fn test_line_buffer_unterminated_tail_is_discarded_not_promoted() {
        let mut buffer = LineBuffer::default();
        push(&mut buffer, "LOOP,0");
        assert_eq!(buffer.pop_line(), None);
        assert_eq!(buffer.discard_partial(), Some("LOOP,0".len()));
        assert_eq!(buffer.discard_partial(), None);
        assert_eq!(
            buffer.pop_line(),
            None,
            "the fragment must not become a line"
        );
    }

    /// C-14. A peer that sends bytes and never a terminator is refused rather
    /// than allowed to make this client allocate without limit.
    #[test]
    fn test_line_buffer_refuses_an_unterminated_fragment_past_its_limit() {
        let mut buffer = LineBuffer::default();
        let chunk = "x".repeat(1024 * 1024);
        let mut outcome = Ok(());
        // Comfortably past the ceiling, in messages that are each acceptable.
        for _ in 0..=(MAX_PARTIAL_LINE_BYTES / chunk.len()) {
            outcome = buffer.push_message(&chunk);
            if outcome.is_err() {
                break;
            }
        }
        match outcome {
            Err(TransportError::Capacity { limit_bytes, .. }) => {
                assert_eq!(limit_bytes, MAX_PARTIAL_LINE_BYTES);
            }
            other => panic!("expected a capacity refusal, got {other:?}"),
        }
        // Nothing is retained: there is no usable line in what was refused.
        assert_eq!(buffer.pop_line(), None);
        assert_eq!(buffer.discard_partial(), None);
    }

    /// C-14. Many small fragments must stay linear: the buffer is compacted
    /// once per message, not once per line.
    #[test]
    fn test_line_buffer_handles_many_lines_in_one_message() {
        let mut buffer = LineBuffer::default();
        let message: String = (0..10_000)
            .map(|index| format!("PROBE,{index}\r\n"))
            .collect();
        push(&mut buffer, &message);
        for index in 0..10_000 {
            assert_eq!(buffer.pop_line(), Some(format!("PROBE,{index}")));
        }
        assert_eq!(buffer.pop_line(), None);
        assert_eq!(buffer.discard_partial(), None);
    }

    /// A line's payload is not touched: commas and percent escapes are the
    /// parser's business [`docs/spec/01-foundations.md` §7.1, §7.2].
    #[test]
    fn test_line_buffer_preserves_line_payload_verbatim() {
        let mut buffer = LineBuffer::default();
        push(&mut buffer, "U,3,1,a%2Cb|#|^Pxyz\r\n");
        assert_eq!(buffer.pop_line().as_deref(), Some("U,3,1,a%2Cb|#|^Pxyz"));
    }

    #[test]
    fn test_line_buffer_clear_discards_everything() {
        let mut buffer = LineBuffer::default();
        push(&mut buffer, "CONOK,S1,50000,5000,*\r\nSERVNAME");
        buffer.clear();
        assert_eq!(buffer.pop_line(), None);
        assert_eq!(buffer.discard_partial(), None);
    }

    // -- message classification --------------------------------------------

    #[test]
    fn test_receive_text_buffers_its_lines() {
        let mut buffer = LineBuffer::default();
        let outcome = receive(
            &mut buffer,
            Message::text("WSOK\r\nCONOK,S1,50000,5000,*\r\n"),
        );
        assert!(matches!(outcome, Ok(Reception::Buffered)));
        assert_eq!(buffer.pop_line().as_deref(), Some("WSOK"));
        assert_eq!(buffer.pop_line().as_deref(), Some("CONOK,S1,50000,5000,*"));
    }

    #[test]
    fn test_receive_binary_message_is_malformed_frame() {
        let mut buffer = LineBuffer::default();
        let outcome = receive(&mut buffer, Message::binary(vec![0x00, 0x01, 0x02]));
        assert!(matches!(
            outcome,
            Err(TransportError::MalformedFrame { .. })
        ));
    }

    #[test]
    fn test_receive_ping_and_pong_are_ignored() {
        let mut buffer = LineBuffer::default();
        assert!(matches!(
            receive(&mut buffer, Message::Ping(Vec::new().into())),
            Ok(Reception::Ignored)
        ));
        assert!(matches!(
            receive(&mut buffer, Message::Pong(Vec::new().into())),
            Ok(Reception::Ignored)
        ));
        assert_eq!(buffer.pop_line(), None);
    }

    #[test]
    fn test_receive_close_reports_peer_close() {
        let mut buffer = LineBuffer::default();
        assert!(matches!(
            receive(&mut buffer, Message::Close(None)),
            Ok(Reception::PeerClosed { description: None })
        ));
    }

    #[test]
    fn test_receive_raw_frame_is_malformed_frame() {
        use tokio_tungstenite::tungstenite::protocol::frame::Frame;

        let mut buffer = LineBuffer::default();
        let outcome = receive(&mut buffer, Message::Frame(Frame::close(None)));
        assert!(matches!(
            outcome,
            Err(TransportError::MalformedFrame { .. })
        ));
    }

    #[test]
    fn test_map_ws_error_reset_without_handshake_is_connection_lost() {
        let error = WsError::Protocol(
            tokio_tungstenite::tungstenite::error::ProtocolError::ResetWithoutClosingHandshake,
        );
        assert!(matches!(
            map_ws_error(&error),
            TransportError::ConnectionLost { .. }
        ));
        assert!(is_fatal(&error));
    }

    #[test]
    fn test_map_ws_error_invalid_utf8_is_malformed_frame() {
        let error = WsError::Utf8("invalid utf-8 sequence".to_owned());
        assert!(matches!(
            map_ws_error(&error),
            TransportError::MalformedFrame { .. }
        ));
    }

    #[test]
    fn test_map_ws_error_io_failure_is_connection_lost() {
        let error = WsError::Io(std::io::Error::new(
            std::io::ErrorKind::ConnectionReset,
            "reset",
        ));
        assert!(matches!(
            map_ws_error(&error),
            TransportError::ConnectionLost { .. }
        ));
    }

    #[test]
    fn test_is_fatal_capacity_error_does_not_kill_the_socket() {
        let error = WsError::Capacity(
            tokio_tungstenite::tungstenite::error::CapacityError::MessageTooLong {
                size: 10,
                max_size: 5,
            },
        );
        assert!(!is_fatal(&error));
    }

    // -- URL handling -------------------------------------------------------

    #[test]
    fn test_subprotocol_carries_the_encoder_protocol_version() {
        assert!(
            SUBPROTOCOL.starts_with(PROTOCOL_VERSION),
            "subprotocol and LS_protocol must name the same TLCP version"
        );
        assert_eq!(SUBPROTOCOL, "TLCP-2.5.0.lightstreamer.com");
    }

    #[test]
    fn test_stream_uri_appends_the_lightstreamer_path() {
        assert_eq!(
            stream_uri("wss://push.example.com", None).unwrap(),
            "wss://push.example.com/lightstreamer"
        );
    }

    #[test]
    fn test_stream_uri_maps_https_to_wss_and_http_to_ws() {
        assert_eq!(
            stream_uri("https://push.example.com:443", None).unwrap(),
            "wss://push.example.com:443/lightstreamer"
        );
        assert_eq!(
            stream_uri("HTTP://push.example.com:8080", None).unwrap(),
            "ws://push.example.com:8080/lightstreamer"
        );
    }

    #[test]
    fn test_stream_uri_ignores_a_trailing_slash() {
        assert_eq!(
            stream_uri("wss://push.example.com/", None).unwrap(),
            "wss://push.example.com/lightstreamer"
        );
    }

    #[test]
    fn test_stream_uri_does_not_duplicate_an_explicit_endpoint_path() {
        assert_eq!(
            stream_uri("wss://push.example.com/lightstreamer", None).unwrap(),
            "wss://push.example.com/lightstreamer"
        );
    }

    #[test]
    fn test_stream_uri_without_scheme_is_rejected() {
        let error = stream_uri("push.example.com", None).unwrap_err();
        assert!(matches!(error, TransportError::Connect { .. }));
    }

    #[test]
    fn test_stream_uri_with_unsupported_scheme_is_rejected() {
        let error = stream_uri("ftp://push.example.com", None).unwrap_err();
        assert!(matches!(error, TransportError::Connect { .. }));
    }

    #[test]
    fn test_stream_uri_control_link_replaces_the_authority() {
        assert_eq!(
            stream_uri("wss://push.example.com", Some("node4.example.com:443")).unwrap(),
            "wss://node4.example.com:443/lightstreamer"
        );
    }

    #[test]
    fn test_stream_uri_control_link_star_keeps_the_configured_address() {
        assert_eq!(
            stream_uri("wss://push.example.com", Some(CONTROL_LINK_SAME)).unwrap(),
            "wss://push.example.com/lightstreamer"
        );
    }

    /// A control link may not downgrade the connection: the scheme is always
    /// inherited from the configured address.
    #[test]
    fn test_stream_uri_control_link_cannot_change_the_scheme() {
        assert_eq!(
            stream_uri("wss://push.example.com", Some("ws://node4.example.com")).unwrap(),
            "wss://node4.example.com/lightstreamer"
        );
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
            redact("wss://push.example.com/lightstreamer"),
            "wss://push.example.com/lightstreamer"
        );
    }

    // -- transport surface --------------------------------------------------

    #[test]
    fn test_properties_declare_ws_behavior() {
        let transport = WsTransport::try_new("wss://push.example.com").unwrap();
        assert_eq!(
            transport.properties(),
            TransportProperties {
                control_shares_stream: true,
                ends_on_content_length: false,
                is_polling: false,
            }
        );
    }

    #[test]
    fn test_try_new_rejects_an_unusable_address() {
        assert!(WsTransport::try_new("push.example.com").is_err());
    }

    #[test]
    fn test_set_control_link_records_and_clears() {
        let mut transport = WsTransport::try_new("wss://push.example.com").unwrap();
        transport.set_control_link(Some("node4.example.com"));
        assert_eq!(transport.control_link.as_deref(), Some("node4.example.com"));
        transport.set_control_link(Some(CONTROL_LINK_SAME));
        assert_eq!(transport.control_link, None);
        transport.set_control_link(Some("node4.example.com"));
        transport.set_control_link(None);
        assert_eq!(transport.control_link, None);
    }

    #[tokio::test]
    async fn test_send_control_without_a_socket_fails_without_panicking() {
        let mut transport = WsTransport::try_new("wss://push.example.com").unwrap();
        let error = transport
            .send_control(EncodedRequest {
                name: "control",
                path: "/lightstreamer/control.txt",
                parameters: "LS_reqId=1&LS_op=delete&LS_subId=1".to_owned(),
            })
            .await
            .unwrap_err();
        assert!(matches!(error, TransportError::Send { .. }));
    }

    #[tokio::test]
    async fn test_close_without_a_socket_is_a_no_op() {
        let mut transport = WsTransport::try_new("wss://push.example.com").unwrap();
        assert!(transport.close().await.is_ok());
    }

    #[tokio::test]
    async fn test_next_line_without_a_socket_is_end_of_stream() {
        let mut transport = WsTransport::try_new("wss://push.example.com").unwrap();
        assert!(transport.next_line().await.is_none());
    }

    // -- loopback server ----------------------------------------------------
    //
    // These drive a real handshake and real frames against an in-process
    // server bound to 127.0.0.1:0. Nothing leaves the machine.

    /// Echoes the TLCP subprotocol, as a Lightstreamer server does
    /// [`docs/spec/01-foundations.md` §6.2.1].
    fn echo_subprotocol(
        _request: &ServerRequest,
        mut response: ServerResponse,
    ) -> Result<ServerResponse, tokio_tungstenite::tungstenite::handshake::server::ErrorResponse>
    {
        response.headers_mut().insert(
            SEC_WEBSOCKET_PROTOCOL,
            HeaderValue::from_static(SUBPROTOCOL),
        );
        Ok(response)
    }

    async fn bind_loopback() -> (TcpListener, SocketAddr) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        (listener, address)
    }

    /// The whole happy path: handshake, `wsok` first, then `create_session`,
    /// then a multi-line response message, then a clean close.
    #[tokio::test]
    async fn test_open_stream_sends_wsok_then_creation_and_yields_each_line() {
        let (listener, address) = bind_loopback().await;
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = accept_hdr_async(stream, echo_subprotocol).await.unwrap();
            let first = socket.next().await.unwrap().unwrap();
            let second = socket.next().await.unwrap().unwrap();
            socket
                .send(Message::text(
                    "WSOK\r\nCONOK,S1aa6c792585db57aT1726545,50000,5000,*\r\n",
                ))
                .await
                .unwrap();
            socket
                .send(Message::text("SERVNAME,Lightstreamer HTTP Server\r\n"))
                .await
                .unwrap();
            socket.close(None).await.unwrap();
            // Stay alive until the client's close echo arrives.
            while socket.next().await.is_some() {}
            (
                first.into_text().unwrap().as_str().to_owned(),
                second.into_text().unwrap().as_str().to_owned(),
            )
        });

        let mut transport = WsTransport::try_new(format!("ws://{address}")).unwrap();
        transport
            .open_stream(StreamOpen::Create(Box::default()))
            .await
            .unwrap();

        assert_eq!(transport.next_line().await.unwrap().unwrap(), "WSOK");
        assert_eq!(
            transport.next_line().await.unwrap().unwrap(),
            "CONOK,S1aa6c792585db57aT1726545,50000,5000,*"
        );
        assert_eq!(
            transport.next_line().await.unwrap().unwrap(),
            "SERVNAME,Lightstreamer HTTP Server"
        );
        assert!(
            transport.next_line().await.is_none(),
            "a clean close ends the stream with None"
        );

        let (wsok, creation) = server.await.unwrap();
        assert_eq!(wsok, "wsok", "the establishment check goes first (§15)");
        assert!(
            creation.starts_with("create_session\r\n"),
            "request name on its own line (§6.2.2), got {creation:?}"
        );
    }

    /// A connection dropped without a closing handshake is transition T5, and
    /// must be distinguishable from a clean close.
    #[tokio::test]
    async fn test_abrupt_disconnect_reports_connection_lost() {
        let (listener, address) = bind_loopback().await;
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = accept_hdr_async(stream, echo_subprotocol).await.unwrap();
            let _wsok = socket.next().await.unwrap().unwrap();
            let _create = socket.next().await.unwrap().unwrap();
            socket.send(Message::text("WSOK\r\n")).await.unwrap();
            // Drop the socket without a close frame.
        });

        let mut transport = WsTransport::try_new(format!("ws://{address}")).unwrap();
        transport
            .open_stream(StreamOpen::Create(Box::default()))
            .await
            .unwrap();
        assert_eq!(transport.next_line().await.unwrap().unwrap(), "WSOK");
        let error = transport.next_line().await.unwrap().unwrap_err();
        assert!(
            matches!(error, TransportError::ConnectionLost { .. }),
            "expected ConnectionLost, got {error:?}"
        );
        server.await.unwrap();
    }

    /// A binary message is not TLCP [`docs/spec/01-foundations.md` §6.2.4].
    #[tokio::test]
    async fn test_binary_message_from_server_is_malformed_frame() {
        let (listener, address) = bind_loopback().await;
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = accept_hdr_async(stream, echo_subprotocol).await.unwrap();
            let _wsok = socket.next().await.unwrap().unwrap();
            let _create = socket.next().await.unwrap().unwrap();
            socket
                .send(Message::binary(vec![0xDE, 0xAD, 0xBE, 0xEF]))
                .await
                .unwrap();
            while socket.next().await.is_some() {}
        });

        let mut transport = WsTransport::try_new(format!("ws://{address}")).unwrap();
        transport
            .open_stream(StreamOpen::Create(Box::default()))
            .await
            .unwrap();
        let error = transport.next_line().await.unwrap().unwrap_err();
        assert!(
            matches!(error, TransportError::MalformedFrame { .. }),
            "expected MalformedFrame, got {error:?}"
        );
        drop(server);
    }

    /// A server that negotiates a different subprotocol is an interfering
    /// intermediary; refuse the connection rather than speak TLCP into it.
    #[tokio::test]
    async fn test_wrong_negotiated_subprotocol_is_refused() {
        let (listener, address) = bind_loopback().await;
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let _ = accept_hdr_async(stream, |_: &ServerRequest, mut response: ServerResponse| {
                response.headers_mut().insert(
                    SEC_WEBSOCKET_PROTOCOL,
                    HeaderValue::from_static("something-else"),
                );
                Ok(response)
            })
            .await;
        });

        let mut transport = WsTransport::try_new(format!("ws://{address}")).unwrap();
        let error = transport
            .open_stream(StreamOpen::Create(Box::default()))
            .await
            .unwrap_err();
        assert!(
            matches!(error, TransportError::Connect { .. }),
            "expected Connect, got {error:?}"
        );
        drop(server);
    }

    /// C-16. The subprotocol "embeds the protocol version"
    /// [`docs/spec/01-foundations.md` §6.2.1], so a handshake that negotiated
    /// none agreed to no version of TLCP. Continuing anyway would be a
    /// compatibility behaviour with no source behind it.
    #[tokio::test]
    async fn test_absent_negotiated_subprotocol_is_refused() {
        let (listener, address) = bind_loopback().await;
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            // A handshake that simply does not answer with a subprotocol.
            let _ = accept_hdr_async(stream, |_: &ServerRequest, response: ServerResponse| {
                Ok(response)
            })
            .await;
        });

        let mut transport = WsTransport::try_new(format!("ws://{address}")).unwrap();
        let error = transport
            .open_stream(StreamOpen::Create(Box::default()))
            .await
            .unwrap_err();
        assert!(
            matches!(error, TransportError::Connect { .. }),
            "expected Connect, got {error:?}"
        );
        drop(server);
    }

    /// C-15. Every line "is terminated by CR-LF"
    /// [`docs/spec/01-foundations.md` §7.1], with no exception for the last one
    /// before a close. A truncated tail is reported, not parsed — and the lines
    /// that *were* complete are still delivered first.
    #[tokio::test]
    async fn test_a_clean_close_after_an_unterminated_line_is_malformed() {
        let (listener, address) = bind_loopback().await;
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = accept_hdr_async(stream, echo_subprotocol).await.unwrap();
            let _wsok = socket.next().await.unwrap().unwrap();
            let _create = socket.next().await.unwrap().unwrap();
            socket
                .send(Message::text("WSOK\r\nEND,31,session destro"))
                .await
                .unwrap();
            socket.close(None).await.unwrap();
            while socket.next().await.is_some() {}
        });

        let mut transport = WsTransport::try_new(format!("ws://{address}")).unwrap();
        transport
            .open_stream(StreamOpen::Create(Box::default()))
            .await
            .unwrap();

        assert_eq!(transport.next_line().await.unwrap().unwrap(), "WSOK");
        let error = transport.next_line().await.unwrap().unwrap_err();
        assert!(
            matches!(error, TransportError::MalformedFrame { .. }),
            "expected a malformed line, got {error:?}"
        );
        assert!(transport.next_line().await.is_none());
        server.await.unwrap();
    }

    /// A control request is framed as `<request-name>` CR-LF
    /// `<parameter-line>` and sent on the stream connection
    /// [`docs/spec/01-foundations.md` §6.2.2; §5.2].
    #[tokio::test]
    async fn test_send_control_frames_the_request_on_the_stream_connection() {
        let (listener, address) = bind_loopback().await;
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = accept_hdr_async(stream, echo_subprotocol).await.unwrap();
            let _wsok = socket.next().await.unwrap().unwrap();
            let _create = socket.next().await.unwrap().unwrap();
            let control = socket.next().await.unwrap().unwrap();
            socket.send(Message::text("REQOK,1\r\n")).await.unwrap();
            while socket.next().await.is_some() {}
            control.into_text().unwrap().as_str().to_owned()
        });

        let mut transport = WsTransport::try_new(format!("ws://{address}")).unwrap();
        transport
            .open_stream(StreamOpen::Create(Box::default()))
            .await
            .unwrap();
        transport
            .send_control(EncodedRequest {
                name: "control",
                path: "/lightstreamer/control.txt",
                parameters: "LS_reqId=1&LS_op=delete&LS_subId=1".to_owned(),
            })
            .await
            .unwrap();
        assert_eq!(transport.next_line().await.unwrap().unwrap(), "REQOK,1");
        transport.close().await.unwrap();

        assert_eq!(
            server.await.unwrap(),
            "control\r\nLS_reqId=1&LS_op=delete&LS_subId=1"
        );
    }
}
