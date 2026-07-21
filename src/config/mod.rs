//! Everything a client needs to be told before it can open a session.
//!
//! Configuration arrives through typed, checked values — never through the
//! environment. This is a library: reading `LS_PASSWORD` out of the process
//! environment behind the caller's back would make the crate's behaviour
//! depend on something the caller did not write down, and would put a secret
//! somewhere this crate has no business looking. Credentials are supplied by
//! the caller, are never logged, and never appear in an error.
//!
//! # Shape of the API
//!
//! ```
//! use lightstreamer_rs::{AdapterSet, ClientConfig, Credentials, ServerAddress};
//!
//! let config = ClientConfig::builder(ServerAddress::try_new("https://push.lightstreamer.com")?)
//!     .with_adapter_set(AdapterSet::try_new("DEMO")?)
//!     .with_credentials(Credentials::anonymous())
//!     .build()?;
//!
//! assert_eq!(config.address().as_str(), "https://push.lightstreamer.com");
//! # Ok::<(), lightstreamer_rs::ConfigError>(())
//! ```

mod address;
mod credentials;
mod options;

use std::time::Duration;

pub use address::ServerAddress;
pub use credentials::{AdapterSet, Credentials};
pub use options::{ConnectionOptions, RetryPolicy, Transport};

/// The longest any timing in this crate may be set to: 365 days.
///
/// A choice of this crate, and an upper bound rather than a recommendation —
/// every sensible value is orders of magnitude below it. It exists because a
/// duration is added to an [`Instant`](std::time::Instant) to make a deadline,
/// and that addition is not total: past the runtime timer's own range the
/// arithmetic overflows, and an overflowed deadline is an expired one. A
/// timeout of [`Duration::MAX`] would then mean "give up at once" — the exact
/// opposite of what it says. Refusing the value at configuration time is the
/// only place that inversion can be caught while it is still a caller's
/// mistake rather than a client that retries in a loop.
pub const MAX_TIMING: Duration = Duration::from_secs(365 * 24 * 60 * 60);

/// A configuration value that cannot be used as given.
///
/// Every variant names the one thing that is wrong and, where it helps, the
/// offending value. No variant ever carries a password: a credential that
/// reached an error message would end up in whatever log the caller writes the
/// error to.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ConfigError {
    /// The server address was empty or only whitespace.
    #[error("server address is empty")]
    EmptyServerAddress,

    /// The server address has no `scheme://` prefix.
    ///
    /// The scheme is required rather than guessed, because defaulting to TLS
    /// or to plain text would each be wrong half the time and neither would be
    /// visible to the caller.
    #[error(
        "server address `{address}` has no scheme: expected ws://, wss://, http:// or https://"
    )]
    MissingScheme {
        /// The address as supplied.
        address: String,
    },

    /// The server address names a scheme this crate cannot speak.
    #[error("unsupported scheme `{scheme}`: expected ws, wss, http or https")]
    UnsupportedScheme {
        /// The scheme as supplied, lowercased.
        scheme: String,
    },

    /// The server address has a scheme but no host after it.
    #[error("server address `{address}` has no host")]
    MissingHost {
        /// The address as supplied.
        address: String,
    },

    /// The server address carries a `user:password@` prefix.
    ///
    /// Refused rather than stripped, and deliberately reported without
    /// quoting the address: an address that reached an error message would
    /// take the credential in it into whatever log the error is written to.
    /// Supply credentials with [`Credentials`] instead, where this crate
    /// knows to redact them.
    #[error("server address must not contain credentials; use Credentials instead")]
    AddressHasUserinfo,

    /// The host part of the server address is not a host.
    #[error(
        "`{host}` is not a host: an IPv6 literal must be bracketed, and no host may contain whitespace"
    )]
    InvalidHost {
        /// The host as supplied.
        host: String,
    },

    /// The port part of the server address is not a port.
    #[error("`{port}` is not a port: expected a number from 1 to 65535")]
    InvalidPort {
        /// The port as supplied.
        port: String,
    },

    /// What follows the host in the server address is not a usable base path.
    ///
    /// TLCP fixes the endpoint path [`docs/spec/01-foundations.md` §6.2.1], so
    /// a query, a fragment or a `..` segment either means something this crate
    /// would silently drop or aims the connection at a different resource.
    #[error("`{path}` is not a base path: no query, fragment or `..` segment is allowed")]
    InvalidAddressPath {
        /// The path as supplied.
        path: String,
    },

    /// The Adapter Set name was empty or only whitespace.
    ///
    /// To request the server's `DEFAULT` Adapter Set, leave the name unset
    /// instead: naming the empty string is a different request, and not one a
    /// caller means [`docs/spec/03-requests.md` §2.1].
    #[error("adapter set name is empty; leave it unset to use the server's DEFAULT")]
    EmptyAdapterSet,

    /// The Adapter Set name contains an ASCII control character.
    #[error("adapter set name contains a control character")]
    AdapterSetControlCharacter,

    /// A subscription named no items.
    #[error("subscription names no items")]
    EmptyItemGroup,

    /// An item group contains an ASCII control character.
    #[error("item group contains a control character")]
    ItemGroupControlCharacter,

    /// An item name in a list contains whitespace, which would split it into
    /// two items once the list is joined.
    #[error("item name `{name}` contains whitespace; item names are separated by spaces")]
    ItemNameHasWhitespace {
        /// The offending name.
        name: String,
    },

    /// A subscription named no fields.
    #[error("subscription names no fields")]
    EmptyFieldSchema,

    /// A field schema contains an ASCII control character.
    #[error("field schema contains a control character")]
    FieldSchemaControlCharacter,

    /// A field name in a list contains whitespace, which would split it into
    /// two fields once the list is joined.
    #[error("field name `{name}` contains whitespace; field names are separated by spaces")]
    FieldNameHasWhitespace {
        /// The offending name.
        name: String,
    },

    /// A message sequence name is not a legal identifier.
    #[error("`{name}` is not a sequence name: expected letters, digits and underscores")]
    InvalidSequenceName {
        /// The name as supplied.
        name: String,
    },

    /// A frequency limit is not a decimal number with a dot separator.
    #[error("`{value}` is not a frequency: expected a decimal number such as `2` or `0.5`")]
    InvalidFrequency {
        /// The text as supplied.
        value: String,
    },

    /// A snapshot length was asked for in a mode that does not admit one.
    ///
    /// `LS_snapshot` as a number of events is "admitted only if `LS_mode` is
    /// `DISTINCT`" [`docs/spec/03-requests.md` §6.1].
    #[error("a snapshot length is admitted only with SubscriptionMode::Distinct, not {mode}")]
    SnapshotLengthNeedsDistinct {
        /// The mode the subscription was built with.
        mode: &'static str,
    },

    /// A snapshot was asked for in [`SubscriptionMode::Raw`](crate::SubscriptionMode::Raw),
    /// which keeps no item state and so has none to send
    /// [`docs/spec/03-requests.md` §6.1].
    #[error("a snapshot is not available with SubscriptionMode::Raw")]
    SnapshotNeedsAStatefulMode,

    /// A maximum frequency was asked for in
    /// [`SubscriptionMode::Raw`](crate::SubscriptionMode::Raw), which the
    /// server does not filter [`docs/spec/03-requests.md` §6.1].
    #[error("a maximum frequency is not applied with SubscriptionMode::Raw")]
    FrequencyNeedsAFilteredMode,

    /// A buffer size was asked for in a mode the server does not buffer.
    ///
    /// `LS_requested_buffer_size` is considered "only if `LS_mode` is `MERGE`
    /// or `DISTINCT`" [`docs/spec/03-requests.md` §6.1].
    #[error("a buffer size applies only to SubscriptionMode::Merge or ::Distinct, not {mode}")]
    BufferSizeNeedsMergeOrDistinct {
        /// The mode the subscription was built with.
        mode: &'static str,
    },

    /// A buffer size was asked for alongside an unfiltered frequency, which
    /// the server ignores [`docs/spec/03-requests.md` §6.1].
    #[error("a buffer size is ignored with MaxFrequency::Unfiltered; set one or the other")]
    BufferSizeWithUnfilteredFrequency,

    /// A [`Message::fire_and_forget`](crate::Message::fire_and_forget) message
    /// was placed in a sequence.
    ///
    /// The two are inseparable in the protocol: `LS_msg_prog` is "mandatory
    /// whenever `LS_sequence` is specified" and a fire-and-forget message is
    /// precisely one with no progressive [`docs/spec/03-requests.md` §12.1].
    #[error("a fire-and-forget message cannot be placed in a sequence: it carries no progressive")]
    SequencedMessageNeedsAProgressive,

    /// A maximum wait was set on a message that is in no sequence, where the
    /// server ignores it [`docs/spec/03-requests.md` §12.1].
    #[error("a maximum wait applies only to a message in a sequence")]
    MaxWaitNeedsASequence,

    /// A duration that must be positive was zero.
    #[error("`{field}` must be greater than zero")]
    ZeroDuration {
        /// Which knob, by the name of its builder method.
        field: &'static str,
    },

    /// A duration is longer than this crate's timers are defined for.
    ///
    /// Either it does not fit the whole number of milliseconds the wire
    /// carries, or it exceeds [`MAX_TIMING`]. Both are refused here rather
    /// than clamped: a deadline that overflowed would become an *immediate*
    /// one, inverting the very setting that produced it.
    #[error("`{field}` is longer than the supported maximum of {MAX_TIMING:?}")]
    DurationTooLarge {
        /// Which knob, by the name of its builder method.
        field: &'static str,
    },

    /// The reconnection ceiling is below the first reconnection delay, so the
    /// schedule could never be honoured.
    #[error("retry max_delay ({max:?}) is below initial_delay ({initial:?})")]
    RetryCeilingBelowInitial {
        /// The configured first delay.
        initial: Duration,
        /// The configured ceiling.
        max: Duration,
    },
}

/// Everything needed to open a session, checked and ready to use.
///
/// Build one with [`ClientConfig::builder`]; the fields are private so a
/// configuration that failed a check cannot exist. Hand it to
/// [`Client::connect`](crate::Client::connect).
///
/// # Examples
///
/// ```
/// use std::time::Duration;
/// use lightstreamer_rs::{
///     AdapterSet, ClientConfig, ConnectionOptions, Credentials, ServerAddress,
/// };
///
/// let config = ClientConfig::builder(ServerAddress::try_new("wss://push.example.com")?)
///     .with_adapter_set(AdapterSet::try_new("MY_ADAPTERS")?)
///     .with_credentials(Credentials::new("alice", "hunter2"))
///     .with_options(ConnectionOptions::default().with_open_timeout(Duration::from_secs(5)))
///     .build()?;
///
/// assert_eq!(config.adapter_set().map(AdapterSet::as_str), Some("MY_ADAPTERS"));
/// # Ok::<(), lightstreamer_rs::ConfigError>(())
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientConfig {
    address: ServerAddress,
    adapter_set: Option<AdapterSet>,
    credentials: Credentials,
    transport: Transport,
    options: ConnectionOptions,
}

impl ClientConfig {
    /// Starts building a configuration for a server.
    ///
    /// The address is the only value with no sensible default, so it is taken
    /// here rather than left to be forgotten.
    #[must_use = "builders do nothing unless .build() is called"]
    pub fn builder(address: ServerAddress) -> ClientConfigBuilder {
        ClientConfigBuilder {
            address,
            adapter_set: None,
            credentials: Credentials::anonymous(),
            transport: Transport::default(),
            options: ConnectionOptions::default(),
        }
    }

    /// The server this configuration points at.
    #[must_use]
    #[inline]
    pub const fn address(&self) -> &ServerAddress {
        &self.address
    }

    /// The Adapter Set that will serve the session, or `None` to let the
    /// server use its `DEFAULT`.
    #[must_use]
    #[inline]
    pub const fn adapter_set(&self) -> Option<&AdapterSet> {
        self.adapter_set.as_ref()
    }

    /// The credentials the session will be created with.
    #[must_use]
    #[inline]
    pub const fn credentials(&self) -> &Credentials {
        &self.credentials
    }

    /// The transport that will carry the session.
    #[must_use]
    #[inline]
    pub const fn transport(&self) -> Transport {
        self.transport
    }

    /// The timeouts, limits and reconnection policy.
    #[must_use]
    #[inline]
    pub const fn options(&self) -> &ConnectionOptions {
        &self.options
    }

    /// Takes the configuration apart for the layer that will run it.
    ///
    /// This module is a **leaf**: it validates what the caller wrote and knows
    /// nothing about the state machine, the wire, or the transports — it cannot
    /// even name them. The translation from this vocabulary into theirs
    /// therefore lives on the other side of the boundary, in
    /// `SessionOptions::from_client_config`, and this is the one accessor it
    /// needs.
    pub(crate) fn into_parts(self) -> (Option<AdapterSet>, Credentials, ConnectionOptions) {
        (self.adapter_set, self.credentials, self.options)
    }
}

/// Assembles a [`ClientConfig`], checking it once at the end.
///
/// The checks that involve more than one value — a reconnection ceiling below
/// its own first delay, for instance — cannot be made one setter at a time, so
/// [`ClientConfigBuilder::build`] is where all of them happen and where the
/// typed error comes from.
#[derive(Debug, Clone)]
#[must_use = "builders do nothing unless .build() is called"]
pub struct ClientConfigBuilder {
    address: ServerAddress,
    adapter_set: Option<AdapterSet>,
    credentials: Credentials,
    transport: Transport,
    options: ConnectionOptions,
}

impl ClientConfigBuilder {
    /// Names the Adapter Set that will serve the session.
    ///
    /// Left unset, the server uses an Adapter Set named `DEFAULT`
    /// [`docs/spec/03-requests.md` §2.1].
    #[must_use = "builders do nothing unless .build() is called"]
    pub fn with_adapter_set(mut self, adapter_set: AdapterSet) -> Self {
        self.adapter_set = Some(adapter_set);
        self
    }

    /// Supplies the credentials the session is created with.
    ///
    /// Defaults to [`Credentials::anonymous`].
    #[must_use = "builders do nothing unless .build() is called"]
    pub fn with_credentials(mut self, credentials: Credentials) -> Self {
        self.credentials = credentials;
        self
    }

    /// Forces a particular transport.
    ///
    /// Defaults to [`Transport::WebSocket`], which is the only one this
    /// release implements.
    #[must_use = "builders do nothing unless .build() is called"]
    pub const fn with_transport(mut self, transport: Transport) -> Self {
        self.transport = transport;
        self
    }

    /// Sets the timeouts, limits and reconnection policy.
    #[must_use = "builders do nothing unless .build() is called"]
    pub fn with_options(mut self, options: ConnectionOptions) -> Self {
        self.options = options;
        self
    }

    /// Checks everything and produces the configuration.
    ///
    /// # Errors
    ///
    /// - [`ConfigError::ZeroDuration`] if `open_timeout`, or a keepalive hint
    ///   or inactivity commitment that was set, is zero, or if the first retry
    ///   delay is zero. A zero `open_timeout` would abandon every attempt
    ///   before it started; a zero retry delay would reconnect in a hot loop.
    /// - [`ConfigError::DurationTooLarge`] if any duration exceeds
    ///   [`MAX_TIMING`], or if a value that travels as a whole number of
    ///   milliseconds does not fit one. Every timing is checked, including the
    ///   keepalive slack and both retry delays: an overflowed deadline is an
    ///   expired one, so a duration too large to add to an instant would
    ///   silently become the opposite of what it says.
    /// - [`ConfigError::RetryCeilingBelowInitial`] if the reconnection ceiling
    ///   is below the first delay.
    ///
    /// A zero keepalive *slack* is accepted: it means "allow the server not
    /// one millisecond past the interval it promised", which is aggressive but
    /// coherent.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::Duration;
    /// use lightstreamer_rs::{
    ///     ClientConfig, ConfigError, ConnectionOptions, ServerAddress,
    /// };
    ///
    /// let rejected = ClientConfig::builder(ServerAddress::try_new("wss://push.example.com")?)
    ///     .with_options(ConnectionOptions::default().with_open_timeout(Duration::ZERO))
    ///     .build();
    ///
    /// assert!(matches!(rejected, Err(ConfigError::ZeroDuration { field: "open_timeout" })));
    /// # Ok::<(), ConfigError>(())
    /// ```
    pub fn build(self) -> Result<ClientConfig, ConfigError> {
        require_positive("open_timeout", self.options.open_timeout())?;
        require_within_maximum("open_timeout", self.options.open_timeout())?;
        require_within_maximum("keepalive_slack", self.options.keepalive_slack())?;

        if let Some(hint) = self.options.keepalive_hint() {
            require_positive("keepalive_hint", hint)?;
            require_within_maximum("keepalive_hint", hint)?;
            require_millis("keepalive_hint", hint)?;
        }
        if let Some(commitment) = self.options.inactivity_commitment() {
            require_positive("inactivity_commitment", commitment)?;
            require_within_maximum("inactivity_commitment", commitment)?;
            require_millis("inactivity_commitment", commitment)?;
        }

        let retry = self.options.retry();
        require_positive("retry.initial_delay", retry.initial_delay())?;
        require_within_maximum("retry.initial_delay", retry.initial_delay())?;
        require_within_maximum("retry.max_delay", retry.max_delay())?;
        if retry.max_delay() < retry.initial_delay() {
            return Err(ConfigError::RetryCeilingBelowInitial {
                initial: retry.initial_delay(),
                max: retry.max_delay(),
            });
        }

        Ok(ClientConfig {
            address: self.address,
            adapter_set: self.adapter_set,
            credentials: self.credentials,
            transport: self.transport,
            options: self.options,
        })
    }
}

/// Rejects a zero duration where a positive one is required.
#[cold]
#[inline(never)]
fn zero_duration(field: &'static str) -> ConfigError {
    ConfigError::ZeroDuration { field }
}

/// Checks that a duration is greater than zero.
fn require_positive(field: &'static str, value: Duration) -> Result<(), ConfigError> {
    if value.is_zero() {
        return Err(zero_duration(field));
    }
    Ok(())
}

/// Checks that a duration is one this crate's timers are defined for.
fn require_within_maximum(field: &'static str, value: Duration) -> Result<(), ConfigError> {
    if value > MAX_TIMING {
        return Err(ConfigError::DurationTooLarge { field });
    }
    Ok(())
}

/// Checks that a duration fits the whole number of milliseconds the wire
/// carries.
fn require_millis(field: &'static str, value: Duration) -> Result<(), ConfigError> {
    u64::try_from(value.as_millis())
        .map(|_| ())
        .map_err(|_| ConfigError::DurationTooLarge { field })
}

#[cfg(test)]
mod tests {

    use super::*;

    fn address() -> ServerAddress {
        match ServerAddress::try_new("wss://push.example.com") {
            Ok(address) => address,
            Err(error) => unreachable!("the fixture address is valid: {error}"),
        }
    }

    #[test]
    fn test_config_builds_with_only_an_address() {
        match ClientConfig::builder(address()).build() {
            Ok(config) => {
                assert_eq!(config.adapter_set(), None);
                assert_eq!(config.transport(), Transport::WebSocket);
                assert_eq!(config.credentials().user(), None);
            }
            Err(error) => panic!("a minimal configuration was rejected: {error}"),
        }
    }

    #[test]
    fn test_config_keeps_what_it_was_given() {
        let adapters = match AdapterSet::try_new("DEMO") {
            Ok(set) => set,
            Err(error) => unreachable!("the fixture adapter set is valid: {error}"),
        };
        let built = ClientConfig::builder(address())
            .with_adapter_set(adapters)
            .with_credentials(Credentials::new("alice", "hunter2"))
            .with_options(ConnectionOptions::default().with_send_sync(false))
            .build();

        match built {
            Ok(config) => {
                assert_eq!(config.adapter_set().map(AdapterSet::as_str), Some("DEMO"));
                assert_eq!(config.credentials().user(), Some("alice"));
                assert!(config.credentials().has_password());
                assert!(!config.options().send_sync());
            }
            Err(error) => panic!("rejected: {error}"),
        }
    }

    #[test]
    fn test_config_rejects_a_zero_open_timeout() {
        let built = ClientConfig::builder(address())
            .with_options(ConnectionOptions::default().with_open_timeout(Duration::ZERO))
            .build();
        assert!(matches!(
            built,
            Err(ConfigError::ZeroDuration {
                field: "open_timeout"
            })
        ));
    }

    #[test]
    fn test_config_rejects_a_zero_keepalive_hint() {
        let built = ClientConfig::builder(address())
            .with_options(ConnectionOptions::default().with_keepalive_hint(Some(Duration::ZERO)))
            .build();
        assert!(matches!(
            built,
            Err(ConfigError::ZeroDuration {
                field: "keepalive_hint"
            })
        ));
    }

    #[test]
    fn test_config_rejects_a_zero_inactivity_commitment() {
        let built = ClientConfig::builder(address())
            .with_options(
                ConnectionOptions::default().with_inactivity_commitment(Some(Duration::ZERO)),
            )
            .build();
        assert!(matches!(
            built,
            Err(ConfigError::ZeroDuration {
                field: "inactivity_commitment"
            })
        ));
    }

    #[test]
    fn test_config_rejects_a_duration_that_cannot_be_expressed_in_millis() {
        let built = ClientConfig::builder(address())
            .with_options(ConnectionOptions::default().with_keepalive_hint(Some(Duration::MAX)))
            .build();
        assert!(matches!(
            built,
            Err(ConfigError::DurationTooLarge {
                field: "keepalive_hint"
            })
        ));
    }

    #[test]
    fn test_config_rejects_a_zero_first_retry_delay() {
        let retry = RetryPolicy::default().with_initial_delay(Duration::ZERO);
        let built = ClientConfig::builder(address())
            .with_options(ConnectionOptions::default().with_retry(retry))
            .build();
        assert!(matches!(
            built,
            Err(ConfigError::ZeroDuration {
                field: "retry.initial_delay"
            })
        ));
    }

    #[test]
    fn test_config_rejects_a_retry_ceiling_below_its_first_delay() {
        let retry = RetryPolicy::default()
            .with_initial_delay(Duration::from_secs(10))
            .with_max_delay(Duration::from_secs(1));
        let built = ClientConfig::builder(address())
            .with_options(ConnectionOptions::default().with_retry(retry))
            .build();
        assert!(matches!(
            built,
            Err(ConfigError::RetryCeilingBelowInitial { .. })
        ));
    }

    #[test]
    fn test_config_accepts_an_equal_retry_ceiling_and_first_delay() {
        let retry = RetryPolicy::default()
            .with_initial_delay(Duration::from_secs(2))
            .with_max_delay(Duration::from_secs(2));
        let built = ClientConfig::builder(address())
            .with_options(ConnectionOptions::default().with_retry(retry))
            .build();
        assert!(built.is_ok());
    }

    // -----------------------------------------------------------------------
    // A-08: no timing may be large enough to overflow a deadline
    // -----------------------------------------------------------------------

    /// Applies one duration to every timing knob in turn.
    fn with_each_timing(value: Duration) -> Vec<(&'static str, Result<ClientConfig, ConfigError>)> {
        let retry_initial = RetryPolicy::default()
            .with_initial_delay(value)
            .with_max_delay(value);
        // The ceiling is checked against the first delay too, so that one is
        // pinned at the smallest legal value while the ceiling varies.
        let retry_max = RetryPolicy::default()
            .with_initial_delay(Duration::from_nanos(1))
            .with_max_delay(value);
        vec![
            (
                "open_timeout",
                ClientConfig::builder(address())
                    .with_options(ConnectionOptions::default().with_open_timeout(value))
                    .build(),
            ),
            (
                "keepalive_slack",
                ClientConfig::builder(address())
                    .with_options(ConnectionOptions::default().with_keepalive_slack(value))
                    .build(),
            ),
            (
                "keepalive_hint",
                ClientConfig::builder(address())
                    .with_options(ConnectionOptions::default().with_keepalive_hint(Some(value)))
                    .build(),
            ),
            (
                "inactivity_commitment",
                ClientConfig::builder(address())
                    .with_options(
                        ConnectionOptions::default().with_inactivity_commitment(Some(value)),
                    )
                    .build(),
            ),
            (
                "retry.initial_delay",
                ClientConfig::builder(address())
                    .with_options(ConnectionOptions::default().with_retry(retry_initial))
                    .build(),
            ),
            (
                "retry.max_delay",
                ClientConfig::builder(address())
                    .with_options(ConnectionOptions::default().with_retry(retry_max))
                    .build(),
            ),
        ]
    }

    #[test]
    fn test_config_accepts_every_timing_at_the_supported_maximum() {
        for (field, built) in with_each_timing(MAX_TIMING) {
            assert!(built.is_ok(), "{field} was rejected at the maximum");
        }
    }

    #[test]
    fn test_config_rejects_every_timing_one_step_past_the_maximum() {
        let over = match MAX_TIMING.checked_add(Duration::from_nanos(1)) {
            Some(over) => over,
            None => Duration::MAX,
        };
        for (field, built) in with_each_timing(over) {
            assert!(
                matches!(built, Err(ConfigError::DurationTooLarge { field: named }) if named == field),
                "{field} was accepted one step past the maximum"
            );
        }
    }

    #[test]
    fn test_config_rejects_every_timing_at_duration_max() {
        // The value that would otherwise overflow the deadline arithmetic and
        // become an *immediate* one.
        for (field, built) in with_each_timing(Duration::MAX) {
            assert!(
                matches!(built, Err(ConfigError::DurationTooLarge { field: named }) if named == field),
                "{field} accepted Duration::MAX"
            );
        }
    }

    #[test]
    fn test_config_accepts_every_timing_at_its_smallest_usable_value() {
        // One nanosecond is aggressive but coherent everywhere it is allowed;
        // zero is separately refused where it would invert behaviour.
        for (field, built) in with_each_timing(Duration::from_nanos(1)) {
            assert!(built.is_ok(), "{field} was rejected at one nanosecond");
        }
    }

    #[test]
    fn test_config_accepts_a_zero_keepalive_slack() {
        let built = ClientConfig::builder(address())
            .with_options(ConnectionOptions::default().with_keepalive_slack(Duration::ZERO))
            .build();
        assert!(built.is_ok());
    }

    #[test]
    fn test_config_accepts_unlimited_retries() {
        let retry = RetryPolicy::default().with_max_attempts(None);
        let built = ClientConfig::builder(address())
            .with_options(ConnectionOptions::default().with_retry(retry))
            .build();
        assert!(built.is_ok());
    }
}
