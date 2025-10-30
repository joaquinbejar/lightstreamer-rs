/******************************************************************************
   Author: Joaquín Béjar García
   Email: jb@taunais.com
   Date: 16/5/25
******************************************************************************/

/// Represents the current status of the `LightstreamerClient`.
#[derive(Debug, Clone)]
pub enum ClientStatus {
    /// The client is attempting to connect to the Lightstreamer Server.
    Connecting,
    /// The client has successfully connected to the Lightstreamer Server.
    /// Contains the type of connection established.
    Connected(ConnectionType),
    /// The connection has been temporarily interrupted.
    /// The client will automatically try to recover the connection.
    Stalled,
    /// The client is disconnected from the Lightstreamer Server.
    /// Contains information about the disconnection type.
    Disconnected(DisconnectionType),
}

impl std::fmt::Display for ClientStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ClientStatus::Connecting => write!(f, "CONNECTING"),
            ClientStatus::Connected(connection_type) => match connection_type {
                ConnectionType::HttpPolling => write!(f, "CONNECTED:HTTP-POLLING"),
                ConnectionType::HttpStreaming => write!(f, "CONNECTED:HTTP-STREAMING"),
                ConnectionType::StreamSensing => write!(f, "CONNECTED:STREAM-SENSING"),
                ConnectionType::WsPolling => write!(f, "CONNECTED:WS-POLLING"),
                ConnectionType::WsStreaming => write!(f, "CONNECTED:WS-STREAMING"),
            },
            ClientStatus::Stalled => write!(f, "STALLED"),
            ClientStatus::Disconnected(disconnection_type) => match disconnection_type {
                DisconnectionType::WillRetry => write!(f, "DISCONNECTED:WILL-RETRY"),
                DisconnectionType::TryingRecovery => write!(f, "DISCONNECTED:TRYING-RECOVERY"),
            },
        }
    }
}

/// Represents the type of connection established with the Lightstreamer Server.
///
/// This enum indicates the specific transport protocol and connection mode being used
/// for communication with the server.
#[derive(Debug, Clone)]
pub enum ConnectionType {
    /// Connection established using HTTP polling transport.
    HttpPolling,
    /// Connection established using HTTP streaming transport.
    HttpStreaming,
    /// Connection in stream-sensing mode, where the client is determining the best
    /// transport to use.
    StreamSensing,
    /// Connection established using WebSocket polling transport.
    WsPolling,
    /// Connection established using WebSocket streaming transport.
    WsStreaming,
}

/// Represents the type of disconnection that occurred with the Lightstreamer Server.
///
/// This enum provides information about the disconnection state and what actions
/// the client will take following the disconnection.
#[derive(Debug, Clone)]
pub enum DisconnectionType {
    /// The client will automatically try to reconnect to the server.
    WillRetry,
    /// The client is attempting to recover the previous session.
    /// This happens when a temporary disconnection is detected and the client
    /// is trying to restore the previous session without losing subscriptions.
    TryingRecovery,
}

/// Represents the type of logging to be used by the LightstreamerClient.
///
/// This enum determines how log messages from the client will be handled and output.
pub enum LogType {
    /// Use the tracing crate for logging.
    /// This provides structured, leveled logging with spans and events.
    TracingLogs,
    /// Use standard output (stdout/stderr) for logging.
    /// This provides simpler logging directly to the console.
    StdLogs,
}

/// The transport type to be used by the client.
/// - WS: the Stream-Sense algorithm is enabled as in the `None` case but the client will
///   only use WebSocket based connections. If a connection over WebSocket is not possible
///   because of the environment the client will not connect at all.
/// - HTTP: the Stream-Sense algorithm is enabled as in the `None` case but the client
///   will only use HTTP based connections. If a connection over HTTP is not possible because
///   of the environment the client will not connect at all.
/// - WS-STREAMING: the Stream-Sense algorithm is disabled and the client will only connect
///   on Streaming over WebSocket. If Streaming over WebSocket is not possible because of
///   the environment the client will not connect at all.
/// - HTTP-STREAMING: the Stream-Sense algorithm is disabled and the client will only
///   connect on Streaming over HTTP. If Streaming over HTTP is not possible because of the
///   browser/environment the client will not connect at all.
/// - WS-POLLING: the Stream-Sense algorithm is disabled and the client will only connect
///   on Polling over WebSocket. If Polling over WebSocket is not possible because of the
///   environment the client will not connect at all.
/// - HTTP-POLLING: the Stream-Sense algorithm is disabled and the client will only connect
///   on Polling over HTTP. If Polling over HTTP is not possible because of the environment
///   the client will not connect at all.
#[derive(Debug, PartialEq, Clone, Copy, Eq, Hash)]
pub enum Transport {
    /// WebSocket transport with Stream-Sense algorithm enabled. The client will only use WebSocket-based connections.
    Ws,
    /// HTTP transport with Stream-Sense algorithm enabled. The client will only use HTTP-based connections.
    Http,
    /// WebSocket Streaming transport with Stream-Sense algorithm disabled. The client will only connect on Streaming over WebSocket.
    WsStreaming,
    /// HTTP Streaming transport with Stream-Sense algorithm disabled. The client will only connect on Streaming over HTTP.
    HttpStreaming,
    /// WebSocket Polling transport with Stream-Sense algorithm disabled. The client will only connect on Polling over WebSocket.
    WsPolling,
    /// HTTP Polling transport with Stream-Sense algorithm disabled. The client will only connect on Polling over HTTP.
    HttpPolling,
}
