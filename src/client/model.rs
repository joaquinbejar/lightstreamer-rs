/******************************************************************************
   Author: Joaquín Béjar García
   Email: jb@taunais.com
   Date: 16/5/25
******************************************************************************/

/// Represents the current status of the `LightstreamerClient`.
pub enum ClientStatus {
    Connecting,
    Connected(ConnectionType),
    Stalled,
    Disconnected(DisconnectionType),
}

pub enum ConnectionType {
    HttpPolling,
    HttpStreaming,
    StreamSensing,
    WsPolling,
    WsStreaming,
}

pub enum DisconnectionType {
    WillRetry,
    TryingRecovery,
}

pub enum LogType {
    TracingLogs,
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
#[derive(Debug, PartialEq)]
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
