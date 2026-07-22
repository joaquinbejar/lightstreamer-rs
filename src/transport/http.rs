//! The HTTP streaming and long-polling transport.
//!
//! HTTP is half-duplex, so a session over it needs **two kinds of connection**
//! [`docs/spec/01-foundations.md` §3]:
//!
//! - the **stream connection** — a single `POST` whose response body is the
//!   sequence of notifications, "of (almost) infinite length" for streaming and
//!   one polling cycle's worth for long polling;
//! - a **separate connection per control request** — its own `POST`, whose
//!   response body carries the `REQOK`/`REQERR` for that request.
//!
//! The port hides that split entirely. ADR-0007's invariant is that **every
//! server line surfaces through [`Transport::next_line`]**, control responses
//! included, so the session layer sees one ordered line stream and never learns
//! how many sockets exist (`docs/adr/0007-transport-port-shape.md`). This
//! module honours it by reading each control response to completion inside
//! [`Transport::send_control`] and queuing its lines ahead of the stream, where
//! [`Transport::next_line`] drains them first. The session correlates them by
//! `LS_reqId` exactly as it does on WebSocket.
//!
//! # Framing, by hand
//!
//! There is no HTTP client here. A request is a `POST` line, a handful of
//! headers, and the parameter line as the body, written straight to a
//! `TcpStream` (wrapped in TLS for `https`/`wss`). The response **head** is
//! parsed with `httparse`; the **body** is de-framed by hand — `Content-Length`,
//! `Transfer-Encoding: chunked`, or read-until-close — and fed line by line to
//! the shared [`LineBuffer`], which is the same reassembly the WebSocket
//! transport uses [`super::framing`].
//!
//! # What this module must never do
//!
//! Interpret a tag. `CONOK`, `LOOP`, `REQOK`, `END` are all just lines here;
//! their meaning belongs to [`crate::protocol::response`] and the session
//! machine. The only bytes this module reasons about are HTTP framing and line
//! terminators.
//!
//! # Untrusted server
//!
//! Every dimension a server controls is bounded: the response head
//! ([`MAX_HEAD_BYTES`]), a chunk-size line ([`MAX_CHUNK_HEADER_BYTES`]), a
//! control response body ([`MAX_CONTROL_BODY_BYTES`]), and every line
//! ([`super::framing::MAX_LINE_BYTES`]). A hostile or broken peer cannot make
//! this client grow memory without limit; it is refused with
//! [`TransportError::Capacity`] instead.
//!
//! # Secrets
//!
//! `create_session` carries `LS_user` and `LS_password`, and every control
//! request body carries `LS_session` [`docs/spec/03-requests.md` §2.1, §5.1]. No
//! request body is ever logged or embedded in an error by this module — only
//! request names, byte counts and status codes. Connection targets pass through
//! [`redact`](super::framing::redact) first.

use std::collections::VecDeque;
use std::fmt;
use std::io;
use std::time::Duration;

use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::TcpStream;
use tokio::time::timeout;

use super::framing::{CONTROL_LINK_SAME, LineBuffer, SCHEME_SEPARATOR, redact};
use super::{EncodedRequest, StreamOpen, Transport, TransportError, TransportProperties};
use crate::protocol::request::{BindSession, CreateSession, PROTOCOL_VERSION, TlcpRequest as _};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// The `Content-Type` set on every request body.
///
/// The specification does not state one — chapter 1 says only that the body may
/// be in any Server-supported charset, UTF-8 recommended, and defers headers to
/// Appendix C [`docs/spec/01-foundations.md` §6.1.3, flagged ambiguity]. The
/// body is a `<param>=<value>&…` line whose reserved characters are already
/// percent-encoded by [`crate::protocol::request`], so it contains no literal
/// `+` or space; `text/plain` therefore cannot be mis-decoded as a form, which
/// is the one behaviour the spec leaves open. This is a defensive choice, not a
/// protocol requirement.
//
// SPEC-AMBIGUITY: `docs/spec/01-foundations.md` §6.1.3 does not state which
// `Content-Type` a client must set. `text/plain; charset=UTF-8` is chosen
// because the encoder never emits a literal `+` or space, so form-vs-text
// decoding cannot change the bytes the Server sees either way.
const CONTENT_TYPE: &str = "text/plain; charset=UTF-8";

/// The default port for a cleartext (`http`/`ws`) endpoint.
const DEFAULT_PORT_PLAIN: u16 = 80;

/// The default port for a TLS (`https`/`wss`) endpoint.
const DEFAULT_PORT_TLS: u16 = 443;

/// The largest HTTP response head this client will buffer, in bytes.
///
/// A well-formed head is a status line and a few short headers. A server that
/// never terminates the head with a blank line is refused rather than allowed
/// to make this client allocate without limit.
const MAX_HEAD_BYTES: usize = 64 * 1024;

/// The largest number of headers this client will parse in a response head.
const MAX_HEADERS: usize = 64;

/// The largest chunk-size line (or trailer line) this client will hold while
/// de-framing a chunked body, in bytes.
const MAX_CHUNK_HEADER_BYTES: usize = 8 * 1024;

/// The largest control-response body this client will buffer, in bytes.
///
/// A control response is a single short line — `REQOK,<id>` or
/// `REQERR,<id>,<code>,<message>` [`docs/spec/01-foundations.md` §7.3]. The
/// ceiling is far above that so a normal response is never refused, yet a server
/// that streams forever on a control connection cannot make this client
/// allocate without limit.
const MAX_CONTROL_BODY_BYTES: usize = 1024 * 1024;

/// How many bytes to ask the socket for in one read.
const READ_BUFFER_BYTES: usize = 16 * 1024;

/// The longest this transport will spend establishing a stream connection and
/// reading its response head.
///
/// A safety net below the session layer, which imposes its own
/// `open_timeout` on the whole create/bind round trip
/// [`docs/spec/02-session-lifecycle.md` §8.7]; this only guards the socket-level
/// work so a black-holed host cannot wedge the driver. A choice of this crate.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(30);

/// The longest a single control request's round trip may take — connect, write,
/// and read the whole response body.
///
/// A control POST that never answered would otherwise block the driver, since
/// [`Transport::send_control`] reads the response before returning. A choice of
/// this crate.
const CONTROL_TIMEOUT: Duration = Duration::from_secs(30);

// ---------------------------------------------------------------------------
// Byte stream
// ---------------------------------------------------------------------------

/// One TCP connection, optionally wrapped in TLS.
///
/// An enum rather than a boxed trait object so that no `dyn` dispatch and no
/// `unsafe` pin projection are needed: both variants are `Unpin`, so the
/// `AsyncRead`/`AsyncWrite` extension methods are called on the concrete type.
enum Conn {
    /// A cleartext connection (`http` / `ws`).
    Plain(TcpStream),
    /// A TLS connection (`https` / `wss`).
    Tls(Box<tokio_native_tls::TlsStream<TcpStream>>),
}

impl Conn {
    /// Reads into `buf`, returning the number of bytes read (`0` at EOF).
    ///
    /// Cancellation-safe: `AsyncReadExt::read` is, and nothing is buffered
    /// before it resolves.
    async fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            Self::Plain(stream) => stream.read(buf).await,
            Self::Tls(stream) => stream.read(buf).await,
        }
    }

    /// Writes all of `buf`.
    async fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        match self {
            Self::Plain(stream) => stream.write_all(buf).await,
            Self::Tls(stream) => stream.write_all(buf).await,
        }
    }

    /// Flushes the write side.
    async fn flush(&mut self) -> io::Result<()> {
        match self {
            Self::Plain(stream) => stream.flush().await,
            Self::Tls(stream) => stream.flush().await,
        }
    }
}

impl fmt::Debug for Conn {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Plain(_) => f.write_str("Conn::Plain"),
            Self::Tls(_) => f.write_str("Conn::Tls"),
        }
    }
}

// ---------------------------------------------------------------------------
// Endpoint resolution
// ---------------------------------------------------------------------------

/// Where and how to open a connection.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Endpoint {
    /// Whether the connection is wrapped in TLS.
    tls: bool,
    /// The host to resolve and connect to, and to present for SNI.
    host: String,
    /// The TCP port.
    port: u16,
    /// The authority for the `Host` header — `host` or `host:port`.
    authority: String,
    /// A redacted rendering for logs and errors, credential-free.
    redacted: String,
}

/// Builds the error for an address this transport cannot use.
#[cold]
#[inline(never)]
fn connect_error(target: &str, reason: impl Into<String>) -> TransportError {
    TransportError::Connect {
        target: target.to_owned(),
        source: Box::new(io::Error::new(io::ErrorKind::InvalidInput, reason.into())),
    }
}

/// Strips a `userinfo@` prefix and any `/path` suffix from an authority.
fn bare_authority(authority: &str) -> &str {
    let host = authority
        .split_once('/')
        .map_or(authority, |(head, _)| head);
    host.rsplit_once('@').map_or(host, |(_, host)| host)
}

/// Splits an authority into host and port, applying the scheme's default port.
///
/// # Errors
///
/// [`TransportError::Connect`] if a stated port is not a number, or an IPv6
/// literal is malformed.
fn split_host_port(
    authority: &str,
    tls: bool,
    redacted: &str,
) -> Result<(String, u16), TransportError> {
    let default = if tls {
        DEFAULT_PORT_TLS
    } else {
        DEFAULT_PORT_PLAIN
    };

    // A bracketed IPv6 literal: `[::1]` or `[::1]:443`.
    if let Some(rest) = authority.strip_prefix('[') {
        let Some((host, tail)) = rest.split_once(']') else {
            return Err(connect_error(redacted, "malformed IPv6 host: missing `]`"));
        };
        let port = match tail.strip_prefix(':') {
            Some(port) => port
                .parse()
                .map_err(|_| connect_error(redacted, format!("invalid port `{port}`")))?,
            None if tail.is_empty() => default,
            None => return Err(connect_error(redacted, "malformed IPv6 authority")),
        };
        return Ok((host.to_owned(), port));
    }

    match authority.rsplit_once(':') {
        Some((host, port)) if !host.is_empty() => {
            let port = port
                .parse()
                .map_err(|_| connect_error(redacted, format!("invalid port `{port}`")))?;
            Ok((host.to_owned(), port))
        }
        _ => Ok((authority.to_owned(), default)),
    }
}

/// Resolves the endpoint a connection should target.
///
/// The configured address supplies the scheme (and therefore whether TLS is
/// used); `http`/`https` and `ws`/`wss` are accepted as synonyms because a
/// Lightstreamer address is conventionally written either way. A control link,
/// when set, replaces the **authority** only and inherits the scheme, so a
/// server-supplied value can never downgrade a TLS session to cleartext
/// [`docs/spec/02-session-lifecycle.md` §3.1].
///
/// # Errors
///
/// [`TransportError::Connect`] if the address carries no scheme, a scheme this
/// transport cannot speak, no host, or an unparseable port.
//
// SPEC-AMBIGUITY: `docs/spec/02-session-lifecycle.md` §3.1 flags that
// `<control-link>` is "the address (IP address, or hostname, and port)" with no
// grammar, and never says whether the scheme is inherited. This inherits it and
// strips any scheme the link carries, matching the WebSocket transport, so a
// control link cannot turn `https` into `http`.
fn resolve(address: &str, control_link: Option<&str>) -> Result<Endpoint, TransportError> {
    let address = address.trim();
    let redacted = redact(address);
    let Some((scheme, rest)) = address.split_once(SCHEME_SEPARATOR) else {
        return Err(connect_error(
            &redacted,
            "server address must start with http://, https://, ws:// or wss://",
        ));
    };
    let tls = match scheme.to_ascii_lowercase().as_str() {
        "http" | "ws" => false,
        "https" | "wss" => true,
        other => {
            return Err(connect_error(
                &redacted,
                format!("unsupported scheme `{other}`; expected http, https, ws or wss"),
            ));
        }
    };

    let configured = bare_authority(rest);
    let authority = match control_link {
        Some(link) if !link.trim().is_empty() && link.trim() != CONTROL_LINK_SAME => {
            let link = link.trim();
            // Inherit the scheme: keep only what follows one the link may carry.
            let link = link
                .split_once(SCHEME_SEPARATOR)
                .map_or(link, |(_, rest)| rest);
            bare_authority(link)
        }
        _ => configured,
    };
    let authority = authority.trim_end_matches('/');
    if authority.is_empty() {
        return Err(connect_error(&redacted, "server address has no host"));
    }

    let redacted_authority = redact(&format!("{scheme}{SCHEME_SEPARATOR}{authority}"));
    let (host, port) = split_host_port(authority, tls, &redacted_authority)?;
    Ok(Endpoint {
        tls,
        host,
        port,
        authority: authority.to_owned(),
        redacted: redacted_authority,
    })
}

// ---------------------------------------------------------------------------
// Response head
// ---------------------------------------------------------------------------

/// How the body of one HTTP response is framed.
#[derive(Debug)]
enum BodyFraming {
    /// Exactly this many body octets remain to be read.
    Length(u64),
    /// A `Transfer-Encoding: chunked` body, mid-decode.
    Chunked(Chunked),
    /// The body runs until the connection closes (no length, not chunked).
    UntilClose,
}

/// A parsed HTTP response head.
#[derive(Debug)]
struct Head {
    /// The HTTP status code.
    code: u16,
    /// How the body is framed.
    framing: BodyFraming,
    /// Body octets that arrived in the same read as the head.
    leftover: Vec<u8>,
}

/// Parses a response head, if it is complete in `buf`.
///
/// Returns `Ok(None)` while the head is still incomplete, so the caller reads
/// more and tries again.
///
/// # Errors
///
/// [`TransportError::MalformedFrame`] if the head is not valid HTTP, or a
/// framing header is malformed.
fn parse_head(buf: &[u8]) -> Result<Option<Head>, TransportError> {
    let mut headers = [httparse::EMPTY_HEADER; MAX_HEADERS];
    let mut response = httparse::Response::new(&mut headers);
    match response.parse(buf) {
        Ok(httparse::Status::Complete(head_len)) => {
            let code = response
                .code
                .ok_or_else(|| TransportError::MalformedFrame {
                    reason: "HTTP response head had no status code".to_owned(),
                })?;
            let framing = framing_from_headers(response.headers)?;
            let leftover = buf.get(head_len..).unwrap_or_default().to_vec();
            Ok(Some(Head {
                code,
                framing,
                leftover,
            }))
        }
        Ok(httparse::Status::Partial) => Ok(None),
        Err(error) => Err(TransportError::MalformedFrame {
            reason: format!("invalid HTTP response head: {error}"),
        }),
    }
}

/// Derives the body framing from the response headers.
///
/// `Transfer-Encoding: chunked` takes precedence over `Content-Length`, per
/// RFC 9112; with neither, the body runs until the connection closes.
///
/// # Errors
///
/// [`TransportError::MalformedFrame`] if a `Content-Length` is not a number.
fn framing_from_headers(headers: &[httparse::Header]) -> Result<BodyFraming, TransportError> {
    let mut chunked = false;
    let mut length: Option<u64> = None;
    for header in headers {
        if header.name.eq_ignore_ascii_case("transfer-encoding") {
            let value = String::from_utf8_lossy(header.value).to_ascii_lowercase();
            if value.split(',').any(|coding| coding.trim() == "chunked") {
                chunked = true;
            }
        } else if header.name.eq_ignore_ascii_case("content-length") {
            let text =
                std::str::from_utf8(header.value).map_err(|_| TransportError::MalformedFrame {
                    reason: "Content-Length was not ASCII".to_owned(),
                })?;
            let value = text
                .trim()
                .parse::<u64>()
                .map_err(|_| TransportError::MalformedFrame {
                    reason: format!("invalid Content-Length `{text}`"),
                })?;
            length = Some(value);
        }
    }
    if chunked {
        Ok(BodyFraming::Chunked(Chunked::default()))
    } else if let Some(value) = length {
        Ok(BodyFraming::Length(value))
    } else {
        Ok(BodyFraming::UntilClose)
    }
}

// ---------------------------------------------------------------------------
// Chunked transfer decoding
// ---------------------------------------------------------------------------

/// The position of a chunked-body decoder between reads.
#[derive(Debug, Default, PartialEq, Eq)]
enum ChunkState {
    /// Reading the hexadecimal chunk-size line.
    #[default]
    Size,
    /// Reading this many octets of chunk data.
    Data(u64),
    /// Reading the CR-LF that terminates a chunk's data.
    DataCrlf,
    /// Past the terminating zero-size chunk: reading trailer lines until a
    /// blank one.
    Trailer,
    /// The terminating zero-size chunk and its trailers have been consumed.
    Done,
}

/// An incremental `Transfer-Encoding: chunked` decoder.
///
/// Consumes complete chunk-framing units from the front of a byte buffer,
/// appending decoded body octets to an output buffer and leaving any incomplete
/// unit in place for the next read. Every framing line it scans is bounded by
/// [`MAX_CHUNK_HEADER_BYTES`].
#[derive(Debug, Default)]
struct Chunked {
    state: ChunkState,
}

impl Chunked {
    /// Whether the terminating zero-size chunk has been fully consumed.
    #[must_use]
    #[inline]
    fn is_done(&self) -> bool {
        matches!(self.state, ChunkState::Done)
    }

    /// Decodes as much as possible from the front of `pending`.
    ///
    /// Appends decoded body octets to `out`, drains the consumed prefix from
    /// `pending`, and returns whether the body is now complete.
    ///
    /// # Errors
    ///
    /// [`TransportError::MalformedFrame`] for an invalid chunk size, or
    /// [`TransportError::Capacity`] for a framing line past
    /// [`MAX_CHUNK_HEADER_BYTES`].
    fn decode(&mut self, pending: &mut Vec<u8>, out: &mut Vec<u8>) -> Result<bool, TransportError> {
        let mut pos = 0usize;
        loop {
            match self.state {
                ChunkState::Size => {
                    let rest = pending.get(pos..).unwrap_or_default();
                    let Some(eol) = find_crlf(rest) else {
                        check_header_bound(rest.len())?;
                        break;
                    };
                    let line = rest.get(..eol).unwrap_or_default();
                    let size = parse_chunk_size(line)?;
                    pos = pos.saturating_add(eol).saturating_add(2);
                    self.state = if size == 0 {
                        ChunkState::Trailer
                    } else {
                        ChunkState::Data(size)
                    };
                }
                ChunkState::Data(remaining) => {
                    let rest = pending.get(pos..).unwrap_or_default();
                    let take = remaining.min(rest.len() as u64);
                    let taken = usize::try_from(take).unwrap_or(rest.len());
                    if let Some(chunk) = rest.get(..taken) {
                        out.extend_from_slice(chunk);
                    }
                    pos = pos.saturating_add(taken);
                    let left = remaining.saturating_sub(take);
                    if left == 0 {
                        self.state = ChunkState::DataCrlf;
                    } else {
                        self.state = ChunkState::Data(left);
                        break;
                    }
                }
                ChunkState::DataCrlf => {
                    let rest = pending.get(pos..).unwrap_or_default();
                    if rest.len() < 2 {
                        break;
                    }
                    // Tolerate whatever the two octets are; the length already
                    // told us where the data ended.
                    pos = pos.saturating_add(2);
                    self.state = ChunkState::Size;
                }
                ChunkState::Trailer => {
                    let rest = pending.get(pos..).unwrap_or_default();
                    let Some(eol) = find_crlf(rest) else {
                        check_header_bound(rest.len())?;
                        break;
                    };
                    if eol == 0 {
                        pos = pos.saturating_add(2);
                        self.state = ChunkState::Done;
                    } else {
                        // A trailer line, ignored.
                        pos = pos.saturating_add(eol).saturating_add(2);
                    }
                }
                ChunkState::Done => break,
            }
        }
        pending.drain(..pos.min(pending.len()));
        Ok(self.is_done())
    }
}

/// Refuses a framing line that has grown past its ceiling with no terminator.
///
/// # Errors
///
/// [`TransportError::Capacity`] if `held` exceeds [`MAX_CHUNK_HEADER_BYTES`].
fn check_header_bound(held: usize) -> Result<(), TransportError> {
    if held > MAX_CHUNK_HEADER_BYTES {
        return Err(TransportError::Capacity {
            limit_bytes: MAX_CHUNK_HEADER_BYTES,
            reason: format!("{held} bytes of a chunk framing line with no terminator"),
        });
    }
    Ok(())
}

/// Finds the index of the first `CR-LF` in `bytes`.
#[must_use]
fn find_crlf(bytes: &[u8]) -> Option<usize> {
    bytes.windows(2).position(|pair| pair == b"\r\n")
}

/// Parses a chunk size: hexadecimal digits, up to an optional `;`-delimited
/// chunk extension.
///
/// # Errors
///
/// [`TransportError::MalformedFrame`] if the size is not valid hexadecimal.
fn parse_chunk_size(line: &[u8]) -> Result<u64, TransportError> {
    let hex = match line.iter().position(|&byte| byte == b';') {
        Some(index) => line.get(..index).unwrap_or_default(),
        None => line,
    };
    let text = std::str::from_utf8(hex)
        .map_err(|_| TransportError::MalformedFrame {
            reason: "chunk size was not ASCII".to_owned(),
        })?
        .trim();
    u64::from_str_radix(text, 16).map_err(|_| TransportError::MalformedFrame {
        reason: format!("invalid chunk size `{text}`"),
    })
}

// ---------------------------------------------------------------------------
// Body reading
// ---------------------------------------------------------------------------

/// The result of decoding one step of a response body.
#[derive(Debug)]
enum BodyRead {
    /// Some body octets, with more of the body still to come.
    Progress(Vec<u8>),
    /// The final body octets; the body is now complete.
    Ended(Vec<u8>),
}

/// Decodes as much of the body as the currently buffered bytes allow.
///
/// # Errors
///
/// As [`Chunked::decode`].
fn decode_body(
    framing: &mut BodyFraming,
    pending: &mut Vec<u8>,
    out: &mut Vec<u8>,
) -> Result<bool, TransportError> {
    match framing {
        BodyFraming::Length(remaining) => {
            let take = (*remaining).min(pending.len() as u64);
            let taken = usize::try_from(take).unwrap_or(pending.len());
            if let Some(chunk) = pending.get(..taken) {
                out.extend_from_slice(chunk);
            }
            pending.drain(..taken.min(pending.len()));
            *remaining = remaining.saturating_sub(take);
            Ok(*remaining == 0)
        }
        BodyFraming::UntilClose => {
            out.append(pending);
            Ok(false)
        }
        BodyFraming::Chunked(chunked) => chunked.decode(pending, out),
    }
}

/// A live stream connection: its socket, its body framing, and the octets read
/// but not yet decoded.
#[derive(Debug)]
struct StreamConn {
    /// The socket carrying the response body.
    socket: Conn,
    /// How the body is framed.
    framing: BodyFraming,
    /// Octets read from the socket but not yet de-framed.
    pending: Vec<u8>,
}

impl StreamConn {
    /// Advances the body by at most one socket read, returning the decoded
    /// octets.
    ///
    /// Decodes anything already buffered before touching the socket, so a body
    /// delivered in the same packet as the head is not left waiting on a read
    /// that may never come. The single await is the socket read, and it is
    /// cancellation-safe: nothing is consumed unless the read resolves.
    ///
    /// # Errors
    ///
    /// [`TransportError::ConnectionLost`] if the socket fails or closes before
    /// a length- or chunk-framed body completes, or a framing/`Capacity` error
    /// from [`decode_body`].
    async fn read_chunk(&mut self) -> Result<BodyRead, TransportError> {
        let mut out = Vec::new();

        // What is already buffered, first — a body may arrive with its head.
        if decode_body(&mut self.framing, &mut self.pending, &mut out)? {
            return Ok(BodyRead::Ended(out));
        }
        if !out.is_empty() {
            return Ok(BodyRead::Progress(out));
        }

        let mut buffer = [0u8; READ_BUFFER_BYTES];
        let read = self.socket.read(&mut buffer).await.map_err(|error| {
            TransportError::ConnectionLost {
                reason: error.to_string(),
            }
        })?;
        if read == 0 {
            return match &self.framing {
                BodyFraming::UntilClose | BodyFraming::Length(0) => Ok(BodyRead::Ended(Vec::new())),
                BodyFraming::Length(_) => Err(TransportError::ConnectionLost {
                    reason: "stream connection closed before its Content-Length".to_owned(),
                }),
                BodyFraming::Chunked(chunked) if chunked.is_done() => {
                    Ok(BodyRead::Ended(Vec::new()))
                }
                BodyFraming::Chunked(_) => Err(TransportError::ConnectionLost {
                    reason: "chunked stream connection closed mid-body".to_owned(),
                }),
            };
        }
        if let Some(slice) = buffer.get(..read) {
            self.pending.extend_from_slice(slice);
        }
        let complete = decode_body(&mut self.framing, &mut self.pending, &mut out)?;
        Ok(if complete {
            BodyRead::Ended(out)
        } else {
            BodyRead::Progress(out)
        })
    }
}

// ---------------------------------------------------------------------------
// Connecting and requesting
// ---------------------------------------------------------------------------

/// Opens a TCP connection to `endpoint`, wrapping it in TLS when required.
///
/// # Errors
///
/// [`TransportError::Connect`] if the TCP connection or the TLS handshake fails.
async fn dial(endpoint: &Endpoint) -> Result<Conn, TransportError> {
    let connect = |source: Box<dyn std::error::Error + Send + Sync>| TransportError::Connect {
        target: endpoint.redacted.clone(),
        source,
    };

    let tcp = TcpStream::connect((endpoint.host.as_str(), endpoint.port))
        .await
        .map_err(|error| connect(Box::new(error)))?;
    let _ = tcp.set_nodelay(true);

    if endpoint.tls {
        let connector =
            native_tls::TlsConnector::new().map_err(|error| connect(Box::new(error)))?;
        let connector = tokio_native_tls::TlsConnector::from(connector);
        let tls = connector
            .connect(&endpoint.host, tcp)
            .await
            .map_err(|error| connect(Box::new(error)))?;
        Ok(Conn::Tls(Box::new(tls)))
    } else {
        Ok(Conn::Plain(tcp))
    }
}

/// Writes one `POST` request: the request line, the fixed headers, and the body.
///
/// `body` is never logged: it may carry `LS_password` or `LS_session`
/// [`docs/spec/03-requests.md` §2.1, §5.1].
async fn write_request(
    socket: &mut Conn,
    target: &str,
    authority: &str,
    body: &str,
) -> io::Result<()> {
    // `POST /lightstreamer/<request-name>.txt?LS_protocol=TLCP-2.5.0`
    // [`docs/spec/01-foundations.md` §6.1.1]. The line separator is CR-LF, as
    // per HTTP 1.x (§6.1.1), and the body follows the empty line.
    let head = format!(
        "POST {target} HTTP/1.1\r\n\
         Host: {authority}\r\n\
         Content-Type: {CONTENT_TYPE}\r\n\
         Content-Length: {}\r\n\
         Connection: close\r\n\
         \r\n",
        body.len()
    );
    socket.write_all(head.as_bytes()).await?;
    socket.write_all(body.as_bytes()).await?;
    socket.flush().await
}

/// Reads and parses the response head, returning it with any leftover body
/// octets.
///
/// # Errors
///
/// [`TransportError::Capacity`] if the head exceeds [`MAX_HEAD_BYTES`],
/// [`TransportError::ConnectionLost`] if the socket closes first, or
/// [`TransportError::MalformedFrame`] for an invalid head.
async fn read_head(socket: &mut Conn) -> Result<Head, TransportError> {
    let mut buffer = Vec::new();
    let mut scratch = [0u8; READ_BUFFER_BYTES];
    loop {
        if let Some(head) = parse_head(&buffer)? {
            return Ok(head);
        }
        if buffer.len() > MAX_HEAD_BYTES {
            return Err(TransportError::Capacity {
                limit_bytes: MAX_HEAD_BYTES,
                reason: format!(
                    "{} bytes of an unterminated HTTP response head",
                    buffer.len()
                ),
            });
        }
        let read =
            socket
                .read(&mut scratch)
                .await
                .map_err(|error| TransportError::ConnectionLost {
                    reason: error.to_string(),
                })?;
        if read == 0 {
            return Err(TransportError::ConnectionLost {
                reason: "connection closed before the HTTP response head".to_owned(),
            });
        }
        if let Some(slice) = scratch.get(..read) {
            buffer.extend_from_slice(slice);
        }
    }
}

/// Connects to `endpoint`, writes the `POST`, and reads the response head.
///
/// Leaves the socket positioned at the start of the body.
async fn open_post(
    endpoint: &Endpoint,
    target: &str,
    body: &str,
) -> Result<(Conn, Head), TransportError> {
    let mut socket = dial(endpoint).await?;
    write_request(&mut socket, target, &endpoint.authority, body)
        .await
        .map_err(|error| TransportError::ConnectionLost {
            reason: error.to_string(),
        })?;
    let head = read_head(&mut socket).await?;
    Ok((socket, head))
}

/// Whether a status code is a success.
#[must_use]
#[inline]
const fn is_success(code: u16) -> bool {
    code >= 200 && code < 300
}

/// Splits a fully-read response body into its lines.
///
/// End of body terminates the last line, so a body without a trailing CR-LF —
/// which the spec permits, "trim the trailing line terminator (it may not be
/// there)" [`docs/spec/01-foundations.md` §7.4] — still yields it. Empty lines
/// are dropped: a line has a non-empty tag [`docs/spec/01-foundations.md`
/// §7.1].
///
/// # Errors
///
/// [`TransportError::MalformedFrame`] if a line is not valid UTF-8.
fn split_body_lines(body: &[u8]) -> Result<Vec<String>, TransportError> {
    let mut lines = Vec::new();
    for segment in body.split(|&byte| byte == b'\n') {
        let line = segment.strip_suffix(b"\r").unwrap_or(segment);
        if line.is_empty() {
            continue;
        }
        let line = std::str::from_utf8(line).map_err(|error| TransportError::MalformedFrame {
            reason: format!("a control response line was not valid UTF-8: {error}"),
        })?;
        lines.push(line.to_owned());
    }
    Ok(lines)
}

// ---------------------------------------------------------------------------
// The transport
// ---------------------------------------------------------------------------

/// Whether an HTTP transport streams or long-polls.
///
/// The two differ only in their declared [`TransportProperties`]: how the
/// session layer treats the end of a stream connection
/// [`docs/spec/02-session-lifecycle.md` §2.2, T3/T4]. The wire work — a `POST`,
/// a response body read line by line, a separate connection per control request
/// — is identical.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HttpMode {
    /// The response body is of near-infinite length; it ends when its content
    /// length is reached, forcing a rebind (transition T3).
    Streaming,
    /// Each connection returns one polling cycle's notifications and closes;
    /// the session rebinds each cycle (transition T4).
    Polling,
}

/// A TLCP transport over HTTP — streaming or long polling.
///
/// Owns at most one live stream connection and no spawned tasks: every await is
/// driven by the caller through [`Transport::next_line`],
/// [`Transport::send_control`] or [`Transport::close`]. A control request opens,
/// uses and closes its own connection within [`Transport::send_control`], and
/// its response lines are queued for [`Transport::next_line`]. Dropping the
/// transport drops the stream socket and so closes the connection.
pub(crate) struct HttpTransport {
    /// Whether this transport streams or polls.
    mode: HttpMode,
    /// The configured server address, scheme included.
    address: String,
    /// The control link last supplied by the server, if any. Redirects the
    /// next stream connection and every control request
    /// [`docs/spec/02-session-lifecycle.md` §3.1].
    control_link: Option<String>,
    /// The live stream connection, absent before the first open and after any
    /// close or end.
    stream: Option<StreamConn>,
    /// Stream-body lines received but not yet yielded.
    lines: LineBuffer,
    /// Control-response lines queued to be yielded ahead of the stream, so
    /// every server line surfaces on one ordered sequence
    /// (`docs/adr/0007-transport-port-shape.md`).
    control_lines: VecDeque<String>,
    /// The final line of a cleanly ended body, held until every earlier line
    /// has been yielded.
    pending_final: Option<Result<String, TransportError>>,
}

impl fmt::Debug for HttpTransport {
    /// Deliberately omits the address' path and any buffered bytes: the address
    /// is redacted, and body bytes are not this type's to expose.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HttpTransport")
            .field("mode", &self.mode)
            .field("target", &redact(&self.address))
            .field("control_link", &self.control_link.as_deref().map(redact))
            .field("stream_open", &self.stream.is_some())
            .field("queued_control_lines", &self.control_lines.len())
            .finish()
    }
}

impl HttpTransport {
    /// Creates an HTTP transport for a server address such as
    /// `https://push.example.com` or `http://push.example.com:8080`.
    ///
    /// # Errors
    ///
    /// [`TransportError::Connect`] if the address cannot be turned into an
    /// endpoint — see [`resolve`]. Validating here means a mistyped address
    /// fails at construction rather than at the first connection.
    #[must_use = "a transport does nothing until a stream is opened"]
    pub(crate) fn try_new(
        address: impl Into<String>,
        mode: HttpMode,
    ) -> Result<Self, TransportError> {
        let address = address.into();
        resolve(&address, None)?;
        Ok(Self {
            mode,
            address,
            control_link: None,
            stream: None,
            lines: LineBuffer::default(),
            control_lines: VecDeque::new(),
            pending_final: None,
        })
    }

    /// The endpoint the next connection should target, control link applied.
    fn endpoint(&self) -> Result<Endpoint, TransportError> {
        resolve(&self.address, self.control_link.as_deref())
    }

    /// Drops the live stream and everything buffered for it.
    ///
    /// The bytes are suspect after a failure, so nothing about the old
    /// connection is carried into the next.
    fn reset_stream(&mut self) {
        self.stream = None;
        self.lines.clear();
        self.pending_final = None;
    }

    /// Reports the last line of a cleanly ended body, or a truncated one.
    ///
    /// A `Content-Length` body may legitimately omit the trailing CR-LF on its
    /// last line [`docs/spec/01-foundations.md` §7.4]; that fragment is the
    /// final line, flushed here. (An *abrupt* end never reaches this: it
    /// surfaces as [`TransportError::ConnectionLost`] from
    /// [`StreamConn::read_chunk`] instead.)
    fn take_final_line(&mut self) {
        self.pending_final = self.lines.flush_partial();
    }

    /// Reads one control request's response body to completion, returning its
    /// lines.
    ///
    /// # Errors
    ///
    /// [`TransportError::Send`] for any failure of the round trip; a control
    /// response that could not be obtained is not the session's to recover from,
    /// it is a failed request.
    async fn control_round_trip(
        &self,
        request: &EncodedRequest,
    ) -> Result<Vec<String>, TransportError> {
        let endpoint = self.endpoint().map_err(|error| TransportError::Send {
            name: request.name,
            reason: error.to_string(),
        })?;
        let target = format!("{}?LS_protocol={PROTOCOL_VERSION}", request.path);

        let (mut socket, head) = open_post(&endpoint, &target, &request.parameters)
            .await
            .map_err(|error| TransportError::Send {
                name: request.name,
                reason: error.to_string(),
            })?;
        if !is_success(head.code) {
            return Err(TransportError::Send {
                name: request.name,
                reason: format!(
                    "server answered the control request with HTTP {}",
                    head.code
                ),
            });
        }

        let mut framing = head.framing;
        let mut pending = head.leftover;
        let mut body = Vec::new();
        loop {
            let mut out = Vec::new();
            let complete = match decode_body(&mut framing, &mut pending, &mut out) {
                Ok(complete) => complete,
                Err(error) => {
                    return Err(TransportError::Send {
                        name: request.name,
                        reason: error.to_string(),
                    });
                }
            };
            body.extend_from_slice(&out);
            if body.len() > MAX_CONTROL_BODY_BYTES {
                return Err(TransportError::Send {
                    name: request.name,
                    reason: format!(
                        "control response exceeded this client's limit of {MAX_CONTROL_BODY_BYTES} bytes"
                    ),
                });
            }
            if complete {
                break;
            }
            if out.is_empty() {
                let mut scratch = [0u8; READ_BUFFER_BYTES];
                let read =
                    socket
                        .read(&mut scratch)
                        .await
                        .map_err(|error| TransportError::Send {
                            name: request.name,
                            reason: error.to_string(),
                        })?;
                if read == 0 {
                    // A close ends the body — for `Content-Length`/chunked this
                    // may be premature, but the lines gathered so far are still
                    // parsed; a missing response then simply leaves the request
                    // pending, which the session times out like any other.
                    break;
                }
                if let Some(slice) = scratch.get(..read) {
                    pending.extend_from_slice(slice);
                }
            }
        }

        split_body_lines(&body).map_err(|error| TransportError::Send {
            name: request.name,
            reason: error.to_string(),
        })
    }
}

impl Transport for HttpTransport {
    fn properties(&self) -> TransportProperties {
        match self.mode {
            HttpMode::Streaming => TransportProperties {
                // "HTTP is half-duplex, so each control request requires a
                // separate connection" [`docs/spec/01-foundations.md` §3].
                control_shares_stream: false,
                // The stream ends when its content length is reached, producing
                // a `LOOP` and forcing a rebind
                // [`docs/spec/02-session-lifecycle.md` §2.2, T3].
                ends_on_content_length: true,
                is_polling: false,
            },
            HttpMode::Polling => TransportProperties {
                control_shares_stream: false,
                ends_on_content_length: false,
                // Each connection returns one polling cycle and ends
                // [`docs/spec/02-session-lifecycle.md` §2.2, T4; §7.1].
                is_polling: true,
            },
        }
    }

    /// Records the control link for the next connection this transport opens.
    ///
    /// Unlike WebSocket — where control requests ride the stream connection and
    /// there is nothing to redirect — HTTP opens a fresh connection for the next
    /// stream rebind and for **every** control request, so the control link
    /// takes real effect: "the address … to which every following session
    /// rebind and control request must be sent … in case this requires the
    /// establishment of a new socket" [`docs/spec/02-session-lifecycle.md`
    /// §3.1]. Both are new sockets on HTTP.
    ///
    /// `None` and the special value `*` both restore the configured address.
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
                .map(redact)
                .as_deref()
                .unwrap_or("<configured address>"),
            "control link recorded; applies to the next HTTP connection"
        );
    }

    /// Opens a fresh stream connection and sends the creation or bind request.
    ///
    /// Every HTTP stream connection is a new `POST`; there is nothing to reuse,
    /// even for polling, where "a new stream connection is immediately
    /// re-established by the client with a binding request"
    /// [`docs/spec/02-session-lifecycle.md` §7.1].
    async fn open_stream(&mut self, request: StreamOpen) -> Result<(), TransportError> {
        // Encode first: a request that cannot be framed must not leave a
        // half-open socket behind.
        let (target, body) = match &request {
            StreamOpen::Create(create) => {
                let body = create.http_body().map_err(|_| TransportError::Send {
                    name: CreateSession::NAME,
                    // Deliberately opaque: this request carries `LS_password`
                    // [`docs/spec/03-requests.md` §2.1] and an encoder message
                    // may quote the offending value.
                    reason: "session creation parameters cannot be encoded".to_owned(),
                })?;
                (CreateSession::http_target(), body)
            }
            StreamOpen::Bind(bind) => {
                let body = bind.http_body().map_err(|error| TransportError::Send {
                    name: BindSession::NAME,
                    // `bind_session` accepts no credentials
                    // [`docs/spec/03-requests.md` §3], so the encoder's own
                    // message is safe to surface.
                    reason: error.to_string(),
                })?;
                (BindSession::http_target(), body)
            }
        };

        let endpoint = self.endpoint()?;
        // A new connection every time; whatever the previous one held is gone.
        self.reset_stream();

        let (socket, head) = timeout(HANDSHAKE_TIMEOUT, open_post(&endpoint, &target, &body))
            .await
            .map_err(|_| {
                connect_error(&endpoint.redacted, "stream connection handshake timed out")
            })??;
        if !is_success(head.code) {
            return Err(connect_error(
                &endpoint.redacted,
                format!("server answered the stream request with HTTP {}", head.code),
            ));
        }

        self.stream = Some(StreamConn {
            socket,
            framing: head.framing,
            pending: head.leftover,
        });
        Ok(())
    }

    /// Yields the next TLCP line — a control response first, then a stream line.
    ///
    /// Control responses are queued ahead of the stream because
    /// [`Transport::send_control`] has already read them in full; the session
    /// correlates every line by `LS_reqId`, so their order relative to
    /// unrelated notifications does not matter
    /// [`docs/spec/01-foundations.md` §5.3].
    ///
    /// Returns `None` when the stream body has ended cleanly: transition T3 for
    /// streaming (after a `LOOP` line) or T4 for polling
    /// [`docs/spec/02-session-lifecycle.md` §2.2]. An abrupt end is
    /// [`TransportError::ConnectionLost`] (T5), after which recovery is still
    /// permitted.
    ///
    /// Cancellation-safe: the single await is one socket read, and no line is
    /// lost if the future is dropped for another branch.
    async fn next_line(&mut self) -> Option<Result<String, TransportError>> {
        loop {
            // Control responses first, then complete stream lines, then the
            // last line of a cleanly ended body, then the end of the stream.
            if let Some(line) = self.control_lines.pop_front() {
                return Some(Ok(line));
            }
            if let Some(line) = self.lines.pop_line() {
                return Some(line);
            }
            if let Some(line) = self.pending_final.take() {
                return Some(line);
            }
            let has_stream = self.stream.is_some();
            if !has_stream {
                return None;
            }

            // The only await; borrows `self.stream` alone, released before the
            // buffers below are touched.
            let outcome = match self.stream.as_mut() {
                Some(stream) => stream.read_chunk().await,
                None => return None,
            };
            match outcome {
                Ok(BodyRead::Progress(bytes)) => {
                    if let Err(error) = self.lines.push_bytes(&bytes) {
                        self.reset_stream();
                        return Some(Err(error));
                    }
                }
                Ok(BodyRead::Ended(bytes)) => {
                    let pushed = self.lines.push_bytes(&bytes);
                    self.stream = None;
                    if let Err(error) = pushed {
                        self.reset_stream();
                        return Some(Err(error));
                    }
                    // Deliver any complete lines, then the flushed final one,
                    // then `None` on the next turns.
                    self.take_final_line();
                }
                Err(error) => {
                    self.reset_stream();
                    return Some(Err(error));
                }
            }
        }
    }

    /// Sends a control request on its own connection and queues the response.
    ///
    /// HTTP is half-duplex, so the control request "requires a separate
    /// connection, parallel to the main stream connection"
    /// [`docs/spec/01-foundations.md` §3]; its response arrives "on that same
    /// connection" (§5.2). This reads that response to completion and forwards
    /// its lines onto the one line stream [`Transport::next_line`] drains, so
    /// the session never learns a second socket was involved
    /// (`docs/adr/0007-transport-port-shape.md`).
    ///
    /// # Errors
    ///
    /// [`TransportError::Send`] if the request could not be delivered or its
    /// response could not be read within [`CONTROL_TIMEOUT`].
    async fn send_control(&mut self, request: EncodedRequest) -> Result<(), TransportError> {
        // The body is already encoded — the session layer encodes it with
        // `TransportKind::Http` because this transport declares
        // `control_shares_stream = false` [`docs/spec/03-requests.md` §5.1;
        // `docs/adr/0007-transport-port-shape.md`]. This function only delivers
        // it and surfaces the reply; it never re-encodes.
        let lines = timeout(CONTROL_TIMEOUT, self.control_round_trip(&request))
            .await
            .map_err(|_| TransportError::Send {
                name: request.name,
                reason: "control request timed out".to_owned(),
            })??;
        self.control_lines.extend(lines);
        Ok(())
    }

    /// Closes the stream connection and releases every resource it owns.
    ///
    /// Dropping the socket closes the TCP (and TLS) connection; the Server
    /// "automatically detects the connection termination and destroys the
    /// session" [`docs/spec/02-session-lifecycle.md` §6.3]. Idempotent: calling
    /// it without a live stream is a no-op.
    async fn close(&mut self) -> Result<(), TransportError> {
        self.reset_stream();
        self.control_lines.clear();
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    #![allow(clippy::indexing_slicing)]

    use super::*;
    use std::net::SocketAddr;
    use tokio::net::TcpListener;

    // -- endpoint resolution ------------------------------------------------

    #[test]
    fn test_resolve_defaults_the_port_by_scheme() {
        let http = resolve("http://push.example.com", None).unwrap();
        assert_eq!(
            (http.tls, http.host.as_str(), http.port),
            (false, "push.example.com", 80)
        );
        let https = resolve("https://push.example.com", None).unwrap();
        assert_eq!(
            (https.tls, https.host.as_str(), https.port),
            (true, "push.example.com", 443)
        );
    }

    #[test]
    fn test_resolve_accepts_ws_schemes_as_synonyms() {
        assert!(!resolve("ws://h", None).unwrap().tls);
        assert!(resolve("wss://h", None).unwrap().tls);
    }

    #[test]
    fn test_resolve_reads_an_explicit_port() {
        let endpoint = resolve("http://push.example.com:8080/lightstreamer", None).unwrap();
        assert_eq!(
            (endpoint.host.as_str(), endpoint.port),
            ("push.example.com", 8080)
        );
        assert_eq!(endpoint.authority, "push.example.com:8080");
    }

    #[test]
    fn test_resolve_rejects_a_missing_or_unknown_scheme() {
        assert!(matches!(
            resolve("push.example.com", None),
            Err(TransportError::Connect { .. })
        ));
        assert!(matches!(
            resolve("ftp://push.example.com", None),
            Err(TransportError::Connect { .. })
        ));
    }

    #[test]
    fn test_resolve_rejects_a_bad_port() {
        assert!(matches!(
            resolve("http://push.example.com:notaport", None),
            Err(TransportError::Connect { .. })
        ));
    }

    /// A control link replaces the authority and inherits the scheme, so it
    /// cannot downgrade TLS [`docs/spec/02-session-lifecycle.md` §3.1].
    #[test]
    fn test_resolve_control_link_replaces_authority_and_keeps_tls() {
        let endpoint = resolve("https://push.example.com", Some("node7.example.com:8443")).unwrap();
        assert!(endpoint.tls, "the TLS scheme is inherited, not the link's");
        assert_eq!(
            (endpoint.host.as_str(), endpoint.port),
            ("node7.example.com", 8443)
        );
    }

    #[test]
    fn test_resolve_control_link_star_keeps_the_configured_address() {
        let endpoint = resolve("https://push.example.com", Some(CONTROL_LINK_SAME)).unwrap();
        assert_eq!(endpoint.host, "push.example.com");
    }

    #[test]
    fn test_resolve_control_link_cannot_downgrade_to_cleartext() {
        let endpoint =
            resolve("https://push.example.com", Some("http://node7.example.com")).unwrap();
        assert!(endpoint.tls);
        assert_eq!(endpoint.host, "node7.example.com");
    }

    #[test]
    fn test_resolve_strips_userinfo_from_the_redacted_target() {
        let endpoint = resolve("https://user:secret@push.example.com", None).unwrap();
        assert!(
            !endpoint.redacted.contains("secret"),
            "{}",
            endpoint.redacted
        );
        assert_eq!(endpoint.host, "push.example.com");
    }

    #[test]
    fn test_resolve_handles_a_bracketed_ipv6_literal() {
        let endpoint = resolve("http://[::1]:8080", None).unwrap();
        assert_eq!((endpoint.host.as_str(), endpoint.port), ("::1", 8080));
        let no_port = resolve("https://[2001:db8::1]", None).unwrap();
        assert_eq!((no_port.host.as_str(), no_port.port), ("2001:db8::1", 443));
    }

    // -- head parsing -------------------------------------------------------

    #[test]
    fn test_parse_head_is_partial_until_the_blank_line() {
        assert!(
            parse_head(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn test_parse_head_reads_status_and_content_length() {
        let head = parse_head(b"HTTP/1.1 200 OK\r\nContent-Length: 9\r\n\r\nREQOK,1\r\n")
            .unwrap()
            .unwrap();
        assert_eq!(head.code, 200);
        assert!(matches!(head.framing, BodyFraming::Length(9)));
        assert_eq!(head.leftover, b"REQOK,1\r\n");
    }

    #[test]
    fn test_parse_head_detects_chunked() {
        let head = parse_head(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n")
            .unwrap()
            .unwrap();
        assert!(matches!(head.framing, BodyFraming::Chunked(_)));
    }

    #[test]
    fn test_parse_head_without_framing_is_until_close() {
        let head = parse_head(b"HTTP/1.1 200 OK\r\n\r\n").unwrap().unwrap();
        assert!(matches!(head.framing, BodyFraming::UntilClose));
    }

    #[test]
    fn test_parse_head_rejects_a_non_numeric_content_length() {
        assert!(matches!(
            parse_head(b"HTTP/1.1 200 OK\r\nContent-Length: eight\r\n\r\n"),
            Err(TransportError::MalformedFrame { .. })
        ));
    }

    // -- chunk decoding -----------------------------------------------------

    #[test]
    fn test_chunked_decodes_a_whole_body() {
        let mut chunked = Chunked::default();
        let mut pending = b"7\r\nREQOK,1\r\n0\r\n\r\n".to_vec();
        let mut out = Vec::new();
        let done = chunked.decode(&mut pending, &mut out).unwrap();
        assert!(done);
        assert_eq!(out, b"REQOK,1");
        assert!(pending.is_empty());
    }

    /// A chunk split across two reads is held and completed, decoding nothing
    /// twice.
    #[test]
    fn test_chunked_reassembles_across_reads() {
        let mut chunked = Chunked::default();
        let mut out = Vec::new();

        let mut pending = b"5\r\nU,1,".to_vec();
        assert!(!chunked.decode(&mut pending, &mut out).unwrap());
        assert_eq!(out, b"U,1,");

        pending.extend_from_slice(b"1\r\n0\r\n\r\n");
        assert!(chunked.decode(&mut pending, &mut out).unwrap());
        assert_eq!(out, b"U,1,1");
    }

    #[test]
    fn test_chunked_handles_multiple_chunks() {
        let mut chunked = Chunked::default();
        let mut pending = b"3\r\nU,1\r\n4\r\n,1,x\r\n0\r\n\r\n".to_vec();
        let mut out = Vec::new();
        assert!(chunked.decode(&mut pending, &mut out).unwrap());
        assert_eq!(out, b"U,1,1,x");
    }

    #[test]
    fn test_chunked_ignores_a_chunk_extension() {
        let mut chunked = Chunked::default();
        let mut pending = b"7;ext=1\r\nREQOK,9\r\n0\r\n\r\n".to_vec();
        let mut out = Vec::new();
        assert!(chunked.decode(&mut pending, &mut out).unwrap());
        assert_eq!(out, b"REQOK,9");
    }

    #[test]
    fn test_chunked_rejects_an_invalid_size() {
        let mut chunked = Chunked::default();
        let mut pending = b"zz\r\n".to_vec();
        let mut out = Vec::new();
        assert!(matches!(
            chunked.decode(&mut pending, &mut out),
            Err(TransportError::MalformedFrame { .. })
        ));
    }

    #[test]
    fn test_chunked_refuses_an_unterminated_size_line() {
        let mut chunked = Chunked::default();
        let mut pending = vec![b'a'; MAX_CHUNK_HEADER_BYTES + 1];
        let mut out = Vec::new();
        assert!(matches!(
            chunked.decode(&mut pending, &mut out),
            Err(TransportError::Capacity { .. })
        ));
    }

    // -- length framing -----------------------------------------------------

    #[test]
    fn test_decode_body_length_stops_at_the_content_length() {
        let mut framing = BodyFraming::Length(7);
        // More bytes than the length: only the first 7 are body.
        let mut pending = b"REQOK,1\r\nspurious".to_vec();
        let mut out = Vec::new();
        assert!(decode_body(&mut framing, &mut pending, &mut out).unwrap());
        assert_eq!(out, b"REQOK,1");
    }

    #[test]
    fn test_decode_body_until_close_takes_everything() {
        let mut framing = BodyFraming::UntilClose;
        let mut pending = b"U,1,1,x\r\n".to_vec();
        let mut out = Vec::new();
        assert!(!decode_body(&mut framing, &mut pending, &mut out).unwrap());
        assert_eq!(out, b"U,1,1,x\r\n");
        assert!(pending.is_empty());
    }

    // -- body line splitting ------------------------------------------------

    #[test]
    fn test_split_body_lines_keeps_an_unterminated_last_line() {
        // No trailing CR-LF — permitted for the last line [§7.4].
        assert_eq!(split_body_lines(b"REQOK,1").unwrap(), vec!["REQOK,1"]);
        assert_eq!(
            split_body_lines(b"REQERR,1,17,denied\r\n").unwrap(),
            vec!["REQERR,1,17,denied"]
        );
    }

    #[test]
    fn test_split_body_lines_drops_empty_lines() {
        assert_eq!(
            split_body_lines(b"\r\nREQOK,1\r\n\r\n").unwrap(),
            vec!["REQOK,1"]
        );
    }

    // -- transport surface --------------------------------------------------

    #[test]
    fn test_properties_declare_streaming_behaviour() {
        let transport =
            HttpTransport::try_new("https://push.example.com", HttpMode::Streaming).unwrap();
        assert_eq!(
            transport.properties(),
            TransportProperties {
                control_shares_stream: false,
                ends_on_content_length: true,
                is_polling: false,
            }
        );
    }

    #[test]
    fn test_properties_declare_polling_behaviour() {
        let transport =
            HttpTransport::try_new("https://push.example.com", HttpMode::Polling).unwrap();
        assert_eq!(
            transport.properties(),
            TransportProperties {
                control_shares_stream: false,
                ends_on_content_length: false,
                is_polling: true,
            }
        );
    }

    #[test]
    fn test_try_new_rejects_an_unusable_address() {
        assert!(HttpTransport::try_new("push.example.com", HttpMode::Streaming).is_err());
    }

    #[test]
    fn test_set_control_link_records_and_clears() {
        let mut transport =
            HttpTransport::try_new("https://push.example.com", HttpMode::Streaming).unwrap();
        transport.set_control_link(Some("node4.example.com"));
        assert_eq!(transport.control_link.as_deref(), Some("node4.example.com"));
        transport.set_control_link(Some(CONTROL_LINK_SAME));
        assert_eq!(transport.control_link, None);
        transport.set_control_link(Some("node4.example.com"));
        transport.set_control_link(None);
        assert_eq!(transport.control_link, None);
    }

    #[tokio::test]
    async fn test_close_without_a_stream_is_a_no_op() {
        let mut transport =
            HttpTransport::try_new("https://push.example.com", HttpMode::Streaming).unwrap();
        assert!(transport.close().await.is_ok());
        assert!(transport.next_line().await.is_none());
    }

    // -- loopback server ----------------------------------------------------
    //
    // These drive a real POST and a real response against an in-process server
    // bound to 127.0.0.1:0. Nothing leaves the machine, and no TLS is involved.

    async fn bind_loopback() -> (TcpListener, SocketAddr) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        (listener, address)
    }

    /// Reads one HTTP request (head + body) from a server-side socket, so a
    /// test can assert what the client actually sent.
    async fn read_request(socket: &mut TcpStream) -> String {
        let mut buffer = Vec::new();
        let mut scratch = [0u8; 4096];
        loop {
            // Enough to have the whole small request; the client sends head and
            // body back to back and does not close its write side.
            let head_end = buffer.windows(4).position(|w| w == b"\r\n\r\n");
            if let Some(end) = head_end {
                let head = String::from_utf8_lossy(&buffer[..end]);
                let content_length = head
                    .lines()
                    .find_map(|line| {
                        line.strip_prefix("Content-Length: ")
                            .or_else(|| line.strip_prefix("content-length: "))
                    })
                    .and_then(|value| value.trim().parse::<usize>().ok())
                    .unwrap_or(0);
                if buffer.len() >= end + 4 + content_length {
                    return String::from_utf8_lossy(&buffer).into_owned();
                }
            }
            let read = socket.read(&mut scratch).await.unwrap();
            if read == 0 {
                return String::from_utf8_lossy(&buffer).into_owned();
            }
            buffer.extend_from_slice(&scratch[..read]);
        }
    }

    /// A streaming body: the client reads the response line by line until the
    /// server closes it, and reports the end of the stream as `None`.
    #[tokio::test]
    async fn test_streaming_reads_the_body_line_by_line_then_ends() {
        let (listener, address) = bind_loopback().await;
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let request = read_request(&mut socket).await;
            // The body is 53 bytes; a streaming response is content-length
            // framed (the mechanism behind transition T3).
            socket
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 53\r\n\r\n\
                      CONOK,S1,50000,5000,*\r\nSERVNAME,LS\r\nU,1,1,x\r\nLOOP,0\r\n",
                )
                .await
                .unwrap();
            socket.flush().await.unwrap();
            drop(socket);
            request
        });

        let mut transport =
            HttpTransport::try_new(format!("http://{address}"), HttpMode::Streaming).unwrap();
        transport
            .open_stream(StreamOpen::Create(Box::default()))
            .await
            .unwrap();

        assert_eq!(
            transport.next_line().await.unwrap().unwrap(),
            "CONOK,S1,50000,5000,*"
        );
        assert_eq!(transport.next_line().await.unwrap().unwrap(), "SERVNAME,LS");
        assert_eq!(transport.next_line().await.unwrap().unwrap(), "U,1,1,x");
        assert_eq!(transport.next_line().await.unwrap().unwrap(), "LOOP,0");
        assert!(
            transport.next_line().await.is_none(),
            "the body ends the stream"
        );

        let request = server.await.unwrap();
        assert!(
            request.starts_with(
                "POST /lightstreamer/create_session.txt?LS_protocol=TLCP-2.5.0 HTTP/1.1\r\n"
            ),
            "unexpected request line: {request:?}"
        );
        assert!(
            request.contains("LS_cid="),
            "the body carries the request parameters"
        );
    }

    /// A `Content-Length`-bounded body whose last line omits the trailing
    /// CR-LF is still yielded whole [§7.4], then the stream ends.
    #[tokio::test]
    async fn test_content_length_bounded_body_flushes_its_last_line() {
        let (listener, address) = bind_loopback().await;
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let _ = read_request(&mut socket).await;
            // Exactly 9 bytes: "U,1,1,abc" with no trailing terminator.
            socket
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 9\r\n\r\nU,1,1,abc")
                .await
                .unwrap();
            socket.flush().await.unwrap();
            drop(socket);
        });

        let mut transport =
            HttpTransport::try_new(format!("http://{address}"), HttpMode::Streaming).unwrap();
        transport
            .open_stream(StreamOpen::Create(Box::default()))
            .await
            .unwrap();
        assert_eq!(transport.next_line().await.unwrap().unwrap(), "U,1,1,abc");
        assert!(transport.next_line().await.is_none());
        server.await.unwrap();
    }

    /// A polling cycle: the response carries the ready notifications and closes;
    /// the transport ends the stream so the session rebinds (T4).
    #[tokio::test]
    async fn test_polling_cycle_ends_the_stream() {
        let (listener, address) = bind_loopback().await;
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let _ = read_request(&mut socket).await;
            socket
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 9\r\n\r\nU,1,1,x\r\n")
                .await
                .unwrap();
            socket.flush().await.unwrap();
            drop(socket);
        });

        let mut transport =
            HttpTransport::try_new(format!("http://{address}"), HttpMode::Polling).unwrap();
        transport
            .open_stream(StreamOpen::Create(Box::default()))
            .await
            .unwrap();
        assert_eq!(transport.next_line().await.unwrap().unwrap(), "U,1,1,x");
        assert!(
            transport.next_line().await.is_none(),
            "one poll cycle, then rebind"
        );
        server.await.unwrap();
    }

    /// A chunked streaming body is de-framed and delivered line by line.
    #[tokio::test]
    async fn test_streaming_decodes_a_chunked_body() {
        let (listener, address) = bind_loopback().await;
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let _ = read_request(&mut socket).await;
            socket
                .write_all(
                    b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n\
                      17\r\nCONOK,S1,50000,5000,*\r\n\r\n8\r\nLOOP,0\r\n\r\n0\r\n\r\n",
                )
                .await
                .unwrap();
            socket.flush().await.unwrap();
            drop(socket);
        });

        let mut transport =
            HttpTransport::try_new(format!("http://{address}"), HttpMode::Streaming).unwrap();
        transport
            .open_stream(StreamOpen::Create(Box::default()))
            .await
            .unwrap();
        assert_eq!(
            transport.next_line().await.unwrap().unwrap(),
            "CONOK,S1,50000,5000,*"
        );
        assert_eq!(transport.next_line().await.unwrap().unwrap(), "LOOP,0");
        assert!(transport.next_line().await.is_none());
        server.await.unwrap();
    }

    /// The load-bearing HTTP case: a control request opens its **own**
    /// connection, and its response line surfaces through `next_line`
    /// (`docs/adr/0007-transport-port-shape.md`).
    #[tokio::test]
    async fn test_control_response_is_forwarded_onto_the_line_stream() {
        let (stream_listener, stream_address) = bind_loopback().await;
        let (control_listener, control_address) = bind_loopback().await;

        // The stream connection: answers, then stays quiet until closed.
        let stream_server = tokio::spawn(async move {
            let (mut socket, _) = stream_listener.accept().await.unwrap();
            let _ = read_request(&mut socket).await;
            socket
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 23\r\n\r\nCONOK,S1,50000,5000,*\r\n",
                )
                .await
                .unwrap();
            socket.flush().await.unwrap();
            // Keep the (already content-length-complete) socket alive a moment.
            tokio::time::sleep(Duration::from_millis(50)).await;
        });

        // The separate control connection: answers `REQOK,7` and closes.
        let control_server = tokio::spawn(async move {
            let (mut socket, _) = control_listener.accept().await.unwrap();
            let request = read_request(&mut socket).await;
            socket
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 9\r\n\r\nREQOK,7\r\n")
                .await
                .unwrap();
            socket.flush().await.unwrap();
            drop(socket);
            request
        });

        let mut transport =
            HttpTransport::try_new(format!("http://{stream_address}"), HttpMode::Streaming)
                .unwrap();
        transport
            .open_stream(StreamOpen::Create(Box::default()))
            .await
            .unwrap();
        assert_eq!(
            transport.next_line().await.unwrap().unwrap(),
            "CONOK,S1,50000,5000,*"
        );

        // The control link points the control POST at the *second* server.
        transport.set_control_link(Some(&control_address.to_string()));
        transport
            .send_control(EncodedRequest {
                name: "control",
                path: "/lightstreamer/control.txt",
                parameters: "LS_reqId=7&LS_op=add&LS_subId=1&LS_group=item1&LS_schema=x&LS_mode=MERGE&LS_session=S1".to_owned(),
            })
            .await
            .unwrap();

        // The control response surfaces on the same line stream.
        assert_eq!(transport.next_line().await.unwrap().unwrap(), "REQOK,7");

        let control_request = control_server.await.unwrap();
        assert!(
            control_request
                .starts_with("POST /lightstreamer/control.txt?LS_protocol=TLCP-2.5.0 HTTP/1.1\r\n"),
            "the control POST went to the control link: {control_request:?}"
        );
        assert!(control_request.contains("LS_reqId=7"));
        stream_server.await.unwrap();
    }

    /// A control POST reaching a host that answers non-2xx is a failed send,
    /// not a stream failure.
    #[tokio::test]
    async fn test_control_request_non_success_status_is_a_send_error() {
        let (listener, address) = bind_loopback().await;
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let _ = read_request(&mut socket).await;
            socket
                .write_all(b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\n\r\n")
                .await
                .unwrap();
            socket.flush().await.unwrap();
            drop(socket);
        });

        let mut transport =
            HttpTransport::try_new(format!("http://{address}"), HttpMode::Streaming).unwrap();
        let error = transport
            .send_control(EncodedRequest {
                name: "control",
                path: "/lightstreamer/control.txt",
                parameters: "LS_reqId=1&LS_op=delete&LS_subId=1&LS_session=S1".to_owned(),
            })
            .await
            .unwrap_err();
        assert!(
            matches!(error, TransportError::Send { .. }),
            "got {error:?}"
        );
        server.await.unwrap();
    }

    /// A response head that never terminates is refused rather than buffered
    /// without limit.
    #[tokio::test]
    async fn test_oversized_response_head_is_refused() {
        let (listener, address) = bind_loopback().await;
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let _ = read_request(&mut socket).await;
            socket.write_all(b"HTTP/1.1 200 OK\r\n").await.unwrap();
            // A header line that never ends, past the ceiling.
            let filler = vec![b'x'; MAX_HEAD_BYTES + 4096];
            socket.write_all(b"X-Filler: ").await.unwrap();
            let _ = socket.write_all(&filler).await;
            let _ = socket.flush().await;
        });

        let mut transport =
            HttpTransport::try_new(format!("http://{address}"), HttpMode::Streaming).unwrap();
        let error = transport
            .open_stream(StreamOpen::Create(Box::default()))
            .await
            .unwrap_err();
        assert!(
            matches!(error, TransportError::Capacity { .. }),
            "got {error:?}"
        );
        drop(server);
    }

    /// A non-2xx status on the stream request fails the open.
    #[tokio::test]
    async fn test_stream_open_non_success_status_is_a_connect_error() {
        let (listener, address) = bind_loopback().await;
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let _ = read_request(&mut socket).await;
            socket
                .write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n")
                .await
                .unwrap();
            socket.flush().await.unwrap();
            drop(socket);
        });

        let mut transport =
            HttpTransport::try_new(format!("http://{address}"), HttpMode::Streaming).unwrap();
        let error = transport
            .open_stream(StreamOpen::Create(Box::default()))
            .await
            .unwrap_err();
        assert!(
            matches!(error, TransportError::Connect { .. }),
            "got {error:?}"
        );
        server.await.unwrap();
    }

    /// An abrupt close before a `Content-Length` body completes is a lost
    /// connection (T5), not a clean end.
    #[tokio::test]
    async fn test_truncated_content_length_body_is_connection_lost() {
        let (listener, address) = bind_loopback().await;
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let _ = read_request(&mut socket).await;
            // Promises 100 bytes, delivers 8, then drops.
            socket
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\nU,1,1,x\r\n")
                .await
                .unwrap();
            socket.flush().await.unwrap();
            drop(socket);
        });

        let mut transport =
            HttpTransport::try_new(format!("http://{address}"), HttpMode::Streaming).unwrap();
        transport
            .open_stream(StreamOpen::Create(Box::default()))
            .await
            .unwrap();
        assert_eq!(transport.next_line().await.unwrap().unwrap(), "U,1,1,x");
        let error = transport.next_line().await.unwrap().unwrap_err();
        assert!(
            matches!(error, TransportError::ConnectionLost { .. }),
            "got {error:?}"
        );
        server.await.unwrap();
    }
}
