//! The public error taxonomy.
//!
//! There is one variant per condition a caller can meaningfully react to, and
//! nothing is stringly typed: a code the server supplied is kept as a number
//! in a structured field so it can be matched on, never folded into a message
//! [`docs/spec/05-error-codes.md` §1].
//!
//! # No dependency's error type is exposed
//!
//! Every error this crate produces is one of its own. The single exception is
//! [`TransportError::Connect`], which boxes the underlying failure behind
//! `dyn std::error::Error` — a box, not a named type, so no third-party crate's
//! semantic version becomes part of this crate's. That was a deliberate
//! decision recorded on the type itself.
//!
//! # The two code catalogs
//!
//! TLCP has exactly two catalogs of numeric codes, and they overlap: fourteen
//! numbers appear in both with different meanings
//! [`docs/spec/05-error-codes.md` §2]. Which catalog a code belongs to is
//! therefore not something a caller can work out from the number, so this
//! taxonomy encodes it in the variant:
//!
//! | Variant | Catalog | Carried by |
//! |---|---|---|
//! | [`Error::Session`] | Appendix A, *Session Error Codes* | `CONERR`, `END` |
//! | [`Error::Request`] | Appendix B, *Control Error Codes* | `REQERR`, `ERROR`, `MSGFAIL` |
//!
//! Matching on a code without knowing its catalog is a bug waiting to happen —
//! code `20` means "session not found on a bind request" in one and "session
//! not found" in the other, and code `48` means "maximum session duration
//! reached" in one and "MPN device suspended" in the other.

use std::fmt;

use crate::config::ConfigError;
use crate::protocol::ProtocolError;
use crate::session::{ServerCause, SessionClosed};
use crate::transport::TransportError;

/// The result type returned throughout this crate.
pub type Result<T> = std::result::Result<T, Error>;

/// A code and message exactly as the server supplied them.
///
/// The code is preserved as a number so a caller can branch on it; the message
/// is whatever human-readable text accompanied it, and may be empty. Which
/// catalog the code comes from is determined by the [`Error`] variant carrying
/// this value — see the module documentation.
///
/// # Examples
///
/// ```
/// use lightstreamer_rs::Error;
///
/// fn is_bad_credentials(error: &Error) -> bool {
///     // Appendix A code 1 is "user/password check failed".
///     matches!(error, Error::Session(cause) if cause.code() == 1)
/// }
/// ```
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ServerError {
    code: i64,
    message: String,
}

impl ServerError {
    /// Wraps a code and message as the server sent them.
    #[must_use]
    pub fn new(code: i64, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    /// The numeric code, exactly as received.
    #[must_use]
    #[inline]
    pub const fn code(&self) -> i64 {
        self.code
    }

    /// The accompanying text, which the server is allowed to leave empty.
    #[must_use]
    #[inline]
    pub fn message(&self) -> &str {
        &self.message
    }

    /// Whether this code came from the server's **Metadata Adapter** rather
    /// than from the protocol's own catalog.
    ///
    /// A code of `0` or below is supplied by the Adapter and its meaning is
    /// entirely application-specific: no interpretation in
    /// `docs/spec/05-error-codes.md` applies to it, and only the application
    /// that wrote the Adapter knows what it means
    /// [`docs/spec/05-error-codes.md` §2].
    #[must_use]
    #[inline]
    pub const fn is_adapter_defined(&self) -> bool {
        self.code <= 0
    }
}

impl fmt::Display for ServerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.message.is_empty() {
            write!(f, "code {}", self.code)
        } else {
            write!(f, "code {}: {}", self.code, self.message)
        }
    }
}

impl From<ServerCause> for ServerError {
    fn from(cause: ServerCause) -> Self {
        Self {
            code: cause.code,
            message: cause.message,
        }
    }
}

/// Anything that can go wrong in a Lightstreamer client.
///
/// The variants are grouped by what a caller can do about them:
///
/// - [`Error::Config`] — fix the configuration; nothing was sent.
/// - [`Error::Session`], [`Error::Request`] — the server said no, and told you
///   why with a code. Branch on the code.
/// - [`Error::ReconnectExhausted`] — the connection kept failing and this
///   crate has stopped trying. Build a new client when you want to try again.
/// - [`Error::Disconnected`] — the client is shut down. Nothing is wrong; it
///   is simply over.
/// - [`Error::Transport`] — bytes could not be moved. Usually a network or
///   TLS problem.
/// - [`Error::Protocol`] — a request could not be encoded, or a line from the
///   server could not be understood.
/// - [`Error::Internal`] — a bug in this crate. Please report it.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// A configuration value could not be used as given. Nothing was sent to
    /// any server.
    #[error("configuration error: {0}")]
    Config(#[from] ConfigError),

    /// The server refused to create or bind the session, or ended one that was
    /// running, with a code from **Appendix A**
    /// [`docs/spec/05-error-codes.md` §2].
    ///
    /// This crate reached this error only after deciding the code admits no
    /// retry — for the codes the specification marks as temporary it keeps
    /// trying, and you learn about that through the session event stream
    /// instead.
    #[error("session error: {0}")]
    Session(ServerError),

    /// The server refused a control request — a subscription, an
    /// unsubscription, a reconfiguration or a message — with a code from
    /// **Appendix B** [`docs/spec/05-error-codes.md` §2].
    ///
    /// The session itself is unaffected and remains usable.
    #[error("request rejected: {0}")]
    Request(ServerError),

    /// The connection kept failing and the reconnection budget ran out. The
    /// session is definitively lost and this client will not try again.
    ///
    /// The budget is [`RetryPolicy`](crate::RetryPolicy); raise it, or set it
    /// to unlimited, if giving up is not what you want.
    #[error("gave up reconnecting after {attempts} consecutive failed attempts")]
    ReconnectExhausted {
        /// How many consecutive attempts failed.
        attempts: u32,
    },

    /// The client is no longer connected, because it was disconnected or
    /// dropped.
    ///
    /// Not a failure in itself: it is what every pending operation returns
    /// once the session has stopped.
    #[error("the client is disconnected")]
    Disconnected,

    /// Something went wrong moving bytes — connecting, sending, or receiving.
    #[error("transport error: {0}")]
    Transport(#[from] TransportError),

    /// The server sent something this client could not interpret as valid
    /// TLCP, or the client was asked to build a request it cannot encode.
    #[error("protocol error: {0}")]
    Protocol(#[from] ProtocolError),

    /// This crate could not do something it should always be able to do.
    ///
    /// A bug, never a server condition or a configuration mistake.
    #[error("internal error: {reason}")]
    Internal {
        /// What failed. Never contains a credential.
        reason: String,
    },
}

impl Error {
    /// The server's code and message, when the server supplied one.
    ///
    /// A convenience for the common "log the code, then decide" shape. Use the
    /// variant itself when you need to know which catalog the code came from.
    ///
    /// # Examples
    ///
    /// ```
    /// use lightstreamer_rs::{Error, ServerError};
    ///
    /// let error = Error::Request(ServerError::new(19, "Specified subscription not found"));
    /// assert_eq!(error.server_error().map(ServerError::code), Some(19));
    ///
    /// assert!(Error::Disconnected.server_error().is_none());
    /// ```
    #[must_use]
    #[inline]
    pub const fn server_error(&self) -> Option<&ServerError> {
        match self {
            Self::Session(cause) | Self::Request(cause) => Some(cause),
            _ => None,
        }
    }

    /// Whether the session behind this error is gone for good.
    ///
    /// True for a server-side refusal, an exhausted reconnection budget and a
    /// disconnected client; false for a rejected control request, which leaves
    /// the session running, and for a configuration mistake, which never
    /// reached a server.
    #[must_use]
    #[inline]
    pub const fn is_session_terminal(&self) -> bool {
        matches!(
            self,
            Self::Session(_) | Self::ReconnectExhausted { .. } | Self::Disconnected
        )
    }
}

impl From<SessionClosed> for Error {
    /// Maps the session layer's terminal reason onto the public taxonomy.
    ///
    /// Total by construction: every way a session can end has exactly one
    /// error a caller can act on.
    fn from(closed: SessionClosed) -> Self {
        match closed {
            SessionClosed::ByClient { .. } => Self::Disconnected,
            SessionClosed::ByServer { cause } => Self::Session(cause.into()),
            SessionClosed::RetriesExhausted { attempts, .. } => {
                Self::ReconnectExhausted { attempts }
            }
            SessionClosed::Internal { reason } => Self::Internal { reason },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_server_error_preserves_the_code_as_a_number() {
        let cause = ServerError::new(48, "maximum session duration reached");
        assert_eq!(cause.code(), 48);
        assert_eq!(cause.message(), "maximum session duration reached");
        assert!(!cause.is_adapter_defined());
    }

    #[test]
    fn test_server_error_recognizes_adapter_defined_codes() {
        // "If the code is 0 or negative, it has been supplied by the Metadata
        // Adapter" [`docs/spec/05-error-codes.md` §2].
        assert!(ServerError::new(0, "").is_adapter_defined());
        assert!(ServerError::new(-7, "custom").is_adapter_defined());
        assert!(!ServerError::new(1, "").is_adapter_defined());
    }

    #[test]
    fn test_server_error_display_omits_an_empty_message() {
        assert_eq!(ServerError::new(20, "").to_string(), "code 20");
        assert_eq!(ServerError::new(20, "gone").to_string(), "code 20: gone");
    }

    #[test]
    fn test_error_exposes_the_server_error_from_both_catalogs() {
        let session = Error::Session(ServerError::new(1, "auth failed"));
        let request = Error::Request(ServerError::new(23, "bad field schema"));
        assert_eq!(session.server_error().map(ServerError::code), Some(1));
        assert_eq!(request.server_error().map(ServerError::code), Some(23));
        assert!(Error::Disconnected.server_error().is_none());
        assert!(
            Error::ReconnectExhausted { attempts: 3 }
                .server_error()
                .is_none()
        );
    }

    #[test]
    fn test_error_knows_which_conditions_end_the_session() {
        assert!(Error::Session(ServerError::new(1, "")).is_session_terminal());
        assert!(Error::ReconnectExhausted { attempts: 8 }.is_session_terminal());
        assert!(Error::Disconnected.is_session_terminal());
        assert!(!Error::Request(ServerError::new(19, "")).is_session_terminal());
        assert!(!Error::Config(ConfigError::EmptyServerAddress).is_session_terminal());
    }

    #[test]
    fn test_session_closed_maps_onto_the_public_taxonomy() {
        let by_client = Error::from(SessionClosed::ByClient {
            destroy_confirmed: true,
            cause: None,
        });
        assert!(matches!(by_client, Error::Disconnected));

        let by_server = Error::from(SessionClosed::ByServer {
            cause: ServerCause {
                code: 2,
                message: "Requested Adapter Set not available".to_owned(),
            },
        });
        assert!(matches!(by_server, Error::Session(cause) if cause.code() == 2));

        let exhausted = Error::from(SessionClosed::RetriesExhausted {
            attempts: 8,
            last: None,
        });
        assert!(matches!(
            exhausted,
            Error::ReconnectExhausted { attempts: 8 }
        ));

        let internal = Error::from(SessionClosed::Internal {
            reason: "cannot encode".to_owned(),
        });
        assert!(matches!(internal, Error::Internal { .. }));
    }

    #[test]
    fn test_config_error_converts_into_the_public_error() {
        let error: Error = ConfigError::EmptyServerAddress.into();
        assert!(matches!(
            error,
            Error::Config(ConfigError::EmptyServerAddress)
        ));
    }
}
