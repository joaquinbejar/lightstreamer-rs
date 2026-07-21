//! The public error taxonomy.
//!
//! One variant per condition a caller can meaningfully react to. Codes and
//! messages supplied by the server are preserved as structured fields, never
//! flattened into a string — see `docs/spec/05-error-codes.md`.

use crate::protocol::ProtocolError;

/// The result type returned throughout this crate.
pub type Result<T> = std::result::Result<T, Error>;

/// Anything that can go wrong in a Lightstreamer client.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The server sent something this client could not interpret as valid
    /// TLCP, or the client was asked to build a request it cannot encode.
    #[error("protocol error: {0}")]
    Protocol(#[from] ProtocolError),

    /// Something went wrong moving bytes — connecting, sending, or receiving.
    #[error("transport error: {0}")]
    Transport(#[from] crate::transport::TransportError),
}
