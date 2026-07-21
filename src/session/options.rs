//! Everything the session state machine needs to be told, with its defaults.
//!
//! # On defaults
//!
//! The specification states, of every timing and limit it defines, that minima
//! and maxima are configured on the **server**, and that "no numeric default is
//! given anywhere in this document for any of these. A client must treat the
//! values echoed in `CONOK` as the only authoritative timings"
//! [`docs/spec/02-session-lifecycle.md` §8.7, ambiguity A16].
//!
//! Consequently every default below is a **choice of this crate**, documented
//! as such, and every one of them is overridable. None of them is presented as
//! a protocol requirement, and none is used where the server has told us
//! otherwise: the negotiated `<keep-alive>` from `CONOK` always wins over
//! anything requested here [`docs/spec/02-session-lifecycle.md` §3.1].

use std::num::NonZeroUsize;
use std::time::Duration;

use crate::config::ClientConfig;
use crate::protocol::request::ConnectionMode;
use crate::session::backoff::BackoffPolicy;

/// Default grace added to the negotiated keep-alive before a silent stream is
/// declared stalled.
///
/// The spec requires a client-side "configurable timeout" on top of the
/// keep-alive interval but gives no value for it
/// [`docs/spec/02-session-lifecycle.md` §8.1, ambiguity A12]. Three seconds is
/// this crate's choice: enough to absorb one late `PROBE` on a typical
/// five-second keep-alive without letting a wedged connection linger.
const DEFAULT_KEEPALIVE_SLACK: Duration = Duration::from_millis(3000);

/// Default limit on how long a `create_session` or `bind_session` may go
/// unanswered before the attempt is abandoned.
///
/// Purely a client-side limit; the spec defines none. Ten seconds is this
/// crate's choice.
const DEFAULT_OPEN_TIMEOUT: Duration = Duration::from_secs(10);

/// Default capacity of the event channel.
const DEFAULT_EVENT_CAPACITY: usize = 1024;

/// Default capacity of the command channel.
const DEFAULT_COMMAND_CAPACITY: usize = 64;

/// The credentials a session is created with.
///
/// The `Debug` implementation is written by hand so that a password can never
/// reach a log line, a panic message, or an error, however the value is
/// formatted.
#[derive(Clone, Default, PartialEq, Eq)]
pub(crate) struct Credentials {
    /// `LS_user` — the user name, interpreted by the Metadata Adapter.
    /// `None` passes a null user name, which the adapter is still asked to
    /// authenticate [`docs/spec/03-requests.md` §2.1].
    pub(crate) user: Option<String>,
    /// `LS_password` — the password, interpreted by the Metadata Adapter.
    pub(crate) password: Option<String>,
    /// `LS_adapter_set` — the Adapter Set that serves the session. `None`
    /// means the server assumes one named `DEFAULT`
    /// [`docs/spec/03-requests.md` §2.1].
    pub(crate) adapter_set: Option<String>,
}

impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Credentials")
            .field("user", &self.user)
            .field("password", &self.password.as_ref().map(|_| "<redacted>"))
            .field("adapter_set", &self.adapter_set)
            .finish()
    }
}

/// How the session state machine should behave.
///
/// Construct with [`SessionOptions::default`] and adjust fields, or use the
/// builder methods. Validation happens where the transport is known, because
/// two of these knobs are only legal for one kind of connection.
#[derive(Debug, Clone)]
pub(crate) struct SessionOptions {
    /// Who the session is opened as. Never logged, never echoed in an error.
    pub(crate) credentials: Credentials,

    /// Whether the stream connection is a streaming or a polling one, with the
    /// parameters belonging to that group
    /// [`docs/spec/03-requests.md` §2.1, §2.2].
    ///
    /// This must agree with the transport's declared
    /// [`crate::transport::TransportProperties::is_polling`]; the disagreement
    /// is rejected at construction rather than discovered on the wire.
    pub(crate) connection: ConnectionMode,

    /// `LS_content_length` — the byte budget of the stream connection, after
    /// which the server sends `LOOP` and the session must be rebound
    /// [`docs/spec/02-session-lifecycle.md` §8.5]. `None` lets the server
    /// choose from its own configuration.
    pub(crate) content_length: Option<u64>,

    /// Grace added to the negotiated `<keep-alive>` before a silent stream is
    /// declared stalled and recovered
    /// [`docs/spec/02-session-lifecycle.md` §8.1].
    ///
    /// Default: 3 s — a choice of this crate; the spec gives no value.
    pub(crate) keepalive_slack: Duration,

    /// How long to wait for `CONOK` (or `CONERR`) after opening a stream
    /// connection before abandoning the attempt.
    ///
    /// Default: 10 s — a choice of this crate; the spec defines no such limit.
    pub(crate) open_timeout: Duration,

    /// How reconnection attempts are spaced and bounded.
    pub(crate) backoff: BackoffPolicy,

    /// Capacity of the event channel handed to the caller.
    ///
    /// The channel is bounded and the driver **blocks** when it is full rather
    /// than dropping: a dropped data notification would desynchronise the
    /// recovery progressive, which is the one number that makes recovery
    /// correct [`docs/spec/02-session-lifecycle.md` §5.2]. A slow consumer
    /// therefore applies backpressure all the way down to the socket, which is
    /// the intended behaviour.
    pub(crate) event_capacity: NonZeroUsize,

    /// Capacity of the command channel. Callers block when it is full.
    pub(crate) command_capacity: NonZeroUsize,
}

impl Default for SessionOptions {
    fn default() -> Self {
        Self {
            credentials: Credentials::default(),
            connection: ConnectionMode::default(),
            content_length: None,
            keepalive_slack: DEFAULT_KEEPALIVE_SLACK,
            open_timeout: DEFAULT_OPEN_TIMEOUT,
            backoff: BackoffPolicy::default(),
            event_capacity: NonZeroUsize::new(DEFAULT_EVENT_CAPACITY).unwrap_or(NonZeroUsize::MIN),
            command_capacity: NonZeroUsize::new(DEFAULT_COMMAND_CAPACITY)
                .unwrap_or(NonZeroUsize::MIN),
        }
    }
}

impl SessionOptions {
    /// Translates a caller's [`ClientConfig`] into what the state machine
    /// expects.
    ///
    /// The translation lives here rather than on [`ClientConfig`] because
    /// `src/config/*` is a **validated leaf**: it knows what a caller may
    /// write and nothing about the wire, the transports or this state machine.
    /// Putting the mapping on the configuration would make the public
    /// vocabulary depend on two internal layers at once, which is exactly the
    /// coupling the module boundaries forbid. Everything below is therefore
    /// one-way — config in, session options out — and this function is the
    /// single point at which the two vocabularies meet.
    #[must_use]
    pub(crate) fn from_client_config(config: ClientConfig) -> Self {
        let (adapter_set, credentials, options) = config.into_parts();
        let (user, password) = credentials.into_parts();
        let retry = options.retry();
        let defaults = Self::default();

        Self {
            credentials: Credentials {
                user,
                password,
                adapter_set: adapter_set.map(|set| set.as_str().to_owned()),
            },
            connection: ConnectionMode::Streaming {
                inactivity_millis: options.inactivity_commitment().map(as_millis),
                keepalive_millis: options.keepalive_hint().map(as_millis),
                // The server's default is `true`, so the parameter is sent
                // only to switch the notifications off
                // [`docs/spec/03-requests.md` §2.1].
                send_sync: if options.send_sync() {
                    None
                } else {
                    Some(false)
                },
            },
            content_length: options.content_length().map(std::num::NonZeroU64::get),
            keepalive_slack: options.keepalive_slack(),
            open_timeout: options.open_timeout(),
            backoff: BackoffPolicy {
                initial: retry.initial_delay(),
                max: retry.max_delay(),
                max_attempts: retry.max_attempts(),
            },
            event_capacity: options.session_event_capacity(),
            command_capacity: defaults.command_capacity,
        }
    }

    /// Sets the credentials the session is created with.
    #[must_use = "builders do nothing unless the result is used"]
    pub(crate) fn with_credentials(mut self, credentials: Credentials) -> Self {
        self.credentials = credentials;
        self
    }

    /// Sets the connection mode and its parameters.
    #[must_use = "builders do nothing unless the result is used"]
    pub(crate) fn with_connection(mut self, connection: ConnectionMode) -> Self {
        self.connection = connection;
        self
    }

    /// Sets the reconnection policy.
    #[must_use = "builders do nothing unless the result is used"]
    pub(crate) fn with_backoff(mut self, backoff: BackoffPolicy) -> Self {
        self.backoff = backoff;
        self
    }

    /// Whether a polling connection was requested.
    #[must_use]
    #[inline]
    pub(crate) const fn is_polling(&self) -> bool {
        matches!(self.connection, ConnectionMode::Polling { .. })
    }

    /// How often to send a `heartbeat` when nothing else is being sent, or
    /// `None` when no `LS_inactivity_millis` commitment was made.
    ///
    /// The spec gives neither a default for `LS_inactivity_millis` nor any
    /// guidance on heartbeat cadence relative to it
    /// [`docs/spec/02-session-lifecycle.md` §8.4, ambiguity A15]. Half the
    /// committed interval is this crate's choice: it tolerates one lost or
    /// delayed heartbeat while still keeping the commitment.
    ///
    /// Always `None` on a polling connection, where `LS_inactivity_millis` is
    /// not admitted at all [`docs/spec/02-session-lifecycle.md` §8.4].
    #[must_use]
    pub(crate) fn heartbeat_interval(&self) -> Option<Duration> {
        match self.connection {
            ConnectionMode::Streaming {
                inactivity_millis, ..
            } => inactivity_millis
                .map(Duration::from_millis)
                .map(|commitment| commitment / 2),
            ConnectionMode::Polling { .. } => None,
        }
    }
}

/// Converts a duration the configuration layer already accepted into the whole
/// milliseconds the wire carries.
///
/// [`ClientConfig`] refuses at construction any duration whose millisecond
/// count does not fit a `u64`, so the fallback is unreachable; it is written as
/// the maximum rather than zero so that a hypothetical bug upstream could only
/// ever lengthen an interval, never turn a long one into an immediate deadline.
fn as_millis(value: Duration) -> u64 {
    u64::try_from(value.as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_credentials_debug_never_shows_the_password() {
        let credentials = Credentials {
            user: Some("alice".to_owned()),
            password: Some("hunter2".to_owned()),
            adapter_set: Some("WELCOME".to_owned()),
        };
        let rendered = format!("{credentials:?}");
        assert!(!rendered.contains("hunter2"), "{rendered}");
        assert!(rendered.contains("<redacted>"), "{rendered}");
        assert!(rendered.contains("alice"), "{rendered}");
    }

    #[test]
    fn test_options_heartbeat_is_half_the_inactivity_commitment() {
        let options = SessionOptions::default().with_connection(ConnectionMode::Streaming {
            inactivity_millis: Some(8000),
            keepalive_millis: None,
            send_sync: None,
        });
        assert_eq!(
            options.heartbeat_interval(),
            Some(Duration::from_millis(4000))
        );
    }

    #[test]
    fn test_options_no_heartbeat_without_a_commitment() {
        assert_eq!(SessionOptions::default().heartbeat_interval(), None);
    }

    // -- The config -> session-options boundary (R-04) -----------------------

    fn address() -> crate::config::ServerAddress {
        match crate::config::ServerAddress::try_new("wss://push.example.com") {
            Ok(address) => address,
            Err(error) => unreachable!("the fixture address is valid: {error}"),
        }
    }

    #[test]
    fn test_session_options_carry_the_configured_values() {
        use std::num::NonZeroU32;

        use crate::config::{
            AdapterSet, ClientConfig, ConnectionOptions, Credentials as PublicCredentials,
            RetryPolicy,
        };

        let adapters = match AdapterSet::try_new("DEMO") {
            Ok(set) => set,
            Err(error) => unreachable!("the fixture adapter set is valid: {error}"),
        };
        let retry = RetryPolicy::default()
            .with_initial_delay(Duration::from_millis(100))
            .with_max_delay(Duration::from_secs(2))
            .with_max_attempts(NonZeroU32::new(3));
        let config = ClientConfig::builder(address())
            .with_adapter_set(adapters)
            .with_credentials(PublicCredentials::new("alice", "hunter2"))
            .with_options(
                ConnectionOptions::default()
                    .with_inactivity_commitment(Some(Duration::from_secs(20)))
                    .with_keepalive_hint(Some(Duration::from_secs(5)))
                    .with_send_sync(false)
                    .with_retry(retry),
            )
            .build();

        let config = match config {
            Ok(config) => config,
            Err(error) => panic!("rejected: {error}"),
        };
        let session = SessionOptions::from_client_config(config);

        assert_eq!(session.credentials.user.as_deref(), Some("alice"));
        assert_eq!(session.credentials.adapter_set.as_deref(), Some("DEMO"));
        assert_eq!(session.backoff.initial, Duration::from_millis(100));
        assert_eq!(session.backoff.max, Duration::from_secs(2));
        assert_eq!(session.backoff.max_attempts.map(NonZeroU32::get), Some(3));
        match session.connection {
            ConnectionMode::Streaming {
                inactivity_millis,
                keepalive_millis,
                send_sync,
            } => {
                assert_eq!(inactivity_millis, Some(20_000));
                assert_eq!(keepalive_millis, Some(5_000));
                assert_eq!(send_sync, Some(false));
            }
            ConnectionMode::Polling { .. } => panic!("expected a streaming connection"),
        }
    }

    #[test]
    fn test_session_options_omit_send_sync_when_it_is_enabled() {
        let config = match crate::config::ClientConfig::builder(address()).build() {
            Ok(config) => config,
            Err(error) => panic!("rejected: {error}"),
        };
        match SessionOptions::from_client_config(config).connection {
            ConnectionMode::Streaming { send_sync, .. } => assert_eq!(send_sync, None),
            ConnectionMode::Polling { .. } => panic!("expected a streaming connection"),
        }
    }

    #[test]
    fn test_options_no_heartbeat_on_a_polling_connection() {
        // `LS_inactivity_millis` is admitted "Only if `LS_polling` is not
        // `true`" [`docs/spec/02-session-lifecycle.md` §8.4].
        let options = SessionOptions::default().with_connection(ConnectionMode::Polling {
            polling_millis: 5000,
            idle_millis: Some(10_000),
        });
        assert_eq!(options.heartbeat_interval(), None);
        assert!(options.is_polling());
    }
}
