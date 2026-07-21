//! The Transport port and its implementations.
//!
//! A transport moves bytes and frames lines. It **never interprets a
//! notification's meaning** — that belongs to [`crate::protocol::response`].
//! A transport module matching on a notification tag is a layering violation.
//!
//! The port's shape and the reasoning behind it are recorded in
//! `docs/adr/0007-transport-port-shape.md`. The central invariant is that
//! **every line the server produces surfaces through [`Transport::next_line`]**
//! — stream notifications and control responses alike — so the session layer
//! sees one ordered sequence of lines and never learns how many sockets are
//! involved.

// The port is deliberately wider than its single current adapter: `path` and
// `ends_on_content_length` exist for the HTTP transports ADR-0002 promises and
// ADR-0007 shaped the trait around, and the WebSocket adapter has no use for
// them. Narrowing the port to what one transport happens to need would have to
// be undone the moment the second arrives.
#![allow(dead_code)]

pub(crate) use crate::error::TransportError;
use crate::protocol::request::{BindSession, CreateSession};

pub(crate) mod ws;

/// What a transport does that the session state machine must account for.
///
/// The session machine branches on these declared properties, never on which
/// transport is in use, so that a further transport needs no change to it
/// (`docs/adr/0007-transport-port-shape.md`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TransportProperties {
    /// Whether control requests travel on the stream connection itself.
    ///
    /// True for WebSocket. False for HTTP, where the protocol is half-duplex
    /// and "each control request requires a separate connection" parallel to
    /// the stream connection [`docs/spec/02-session-lifecycle.md` §1]. Either
    /// way the response surfaces through [`Transport::next_line`]; this flag
    /// exists because the *cost* and failure modes of a control request differ.
    pub(crate) control_shares_stream: bool,

    /// Whether the stream connection terminates when its content length is
    /// reached, forcing a rebind on a new connection.
    ///
    /// True for HTTP streaming, which produces a `LOOP` at that point
    /// [`docs/spec/02-session-lifecycle.md` §2.2, transition T3]. False for
    /// WebSocket.
    pub(crate) ends_on_content_length: bool,

    /// Whether the transport polls: the stream connection is expected to end
    /// at the close of each polling cycle and be rebound
    /// [`docs/spec/02-session-lifecycle.md` §2.2, transition T4].
    pub(crate) is_polling: bool,
}

/// The request that opens a stream connection.
///
/// A session is created with one and rebound with the other; a session may
/// never be bound to more than one stream connection at a time
/// [`docs/spec/02-session-lifecycle.md` §1].
#[derive(Debug, Clone)]
pub(crate) enum StreamOpen {
    /// Obtain a new session and its initial stream connection.
    Create(Box<CreateSession>),
    /// Bind an existing session to a new stream connection, optionally
    /// recovering from a known progressive.
    Bind(Box<BindSession>),
}

/// A control request already encoded by [`crate::protocol::request`].
///
/// The port takes it pre-encoded so the trait stays non-generic: the session
/// layer owns request construction, the transport owns framing and delivery.
#[derive(Debug, Clone)]
pub(crate) struct EncodedRequest {
    /// The request name — the first line of a WS message, and the
    /// `<request-name>` of the HTTP path [`docs/spec/01-foundations.md` §6].
    pub(crate) name: &'static str,
    /// The HTTP request path, used only by the HTTP transports.
    pub(crate) path: &'static str,
    /// The encoded parameter line.
    pub(crate) parameters: String,
}

/// Moves TLCP lines between this client and a server.
///
/// Implementations own framing: CR-LF splitting, WebSocket message
/// boundaries, and chunked-transfer reassembly all happen below this port, so
/// [`Transport::next_line`] yields one complete line with its terminator
/// already stripped, ready for [`crate::protocol::response::parse_line`].
pub(crate) trait Transport {
    /// What this transport does that the session machine must account for.
    fn properties(&self) -> TransportProperties;

    /// Redirect subsequent control requests to the control link the server
    /// supplied in `CONOK` [`docs/spec/02-session-lifecycle.md` §3.1].
    ///
    /// `None` restores the original address. A transport that carries control
    /// requests on the stream connection may ignore this.
    fn set_control_link(&mut self, host: Option<&str>);

    /// Open the stream connection, sending the creation or bind request that
    /// establishes it.
    ///
    /// The response lines — `CONOK` or `CONERR`, and whatever follows —
    /// arrive through [`Transport::next_line`], not from this call.
    fn open_stream(
        &mut self,
        request: StreamOpen,
    ) -> impl Future<Output = Result<(), TransportError>> + Send;

    /// Yield the next line from the server, or `None` when the stream
    /// connection has ended cleanly.
    ///
    /// This is the single point through which **all** server output reaches
    /// the session layer, control responses included
    /// (`docs/adr/0007-transport-port-shape.md`).
    fn next_line(&mut self) -> impl Future<Output = Option<Result<String, TransportError>>> + Send;

    /// Send a control request.
    ///
    /// Deliberately returns no response: the answer surfaces through
    /// [`Transport::next_line`] like everything else, and is correlated by
    /// request id.
    fn send_control(
        &mut self,
        request: EncodedRequest,
    ) -> impl Future<Output = Result<(), TransportError>> + Send;

    /// Close the stream connection and release every resource it owns.
    fn close(&mut self) -> impl Future<Output = Result<(), TransportError>> + Send;
}

/// Every transport this crate ships, as one type.
///
/// The caller chooses a transport at run time, but `async fn` in a trait is not
/// object-safe, so a `dyn Transport` would need boxed futures and a macro
/// dependency to produce them. A **closed enum that implements the port by
/// delegation** gives the same runtime choice with neither
/// (`docs/adr/0007-transport-port-shape.md`, §3). Transports are a set this
/// crate owns and closes; the protocol, not the socket, is the extension point.
///
/// It lives below the session layer on purpose. The session layer's composition
/// root — `session::connect_configured` — is the only place that names a
/// concrete adapter, so adding one is an edit here and there and never in the
/// public façade, which is what keeps a new transport out of the semver
/// surface.
#[derive(Debug)]
pub(crate) enum AnyTransport {
    /// TLCP over WebSocket, with control requests multiplexed on the stream
    /// connection [`docs/spec/02-session-lifecycle.md` §1].
    WebSocket(ws::WsTransport),
}

impl Transport for AnyTransport {
    fn properties(&self) -> TransportProperties {
        match self {
            Self::WebSocket(transport) => transport.properties(),
        }
    }

    fn set_control_link(&mut self, host: Option<&str>) {
        match self {
            Self::WebSocket(transport) => transport.set_control_link(host),
        }
    }

    async fn open_stream(&mut self, request: StreamOpen) -> Result<(), TransportError> {
        match self {
            Self::WebSocket(transport) => transport.open_stream(request).await,
        }
    }

    async fn next_line(&mut self) -> Option<Result<String, TransportError>> {
        match self {
            Self::WebSocket(transport) => transport.next_line().await,
        }
    }

    async fn send_control(&mut self, request: EncodedRequest) -> Result<(), TransportError> {
        match self {
            Self::WebSocket(transport) => transport.send_control(request).await,
        }
    }

    async fn close(&mut self) -> Result<(), TransportError> {
        match self {
            Self::WebSocket(transport) => transport.close().await,
        }
    }
}
