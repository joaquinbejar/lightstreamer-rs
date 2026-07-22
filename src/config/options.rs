//! Timeouts, limits and the reconnection policy.
//!
//! # On defaults
//!
//! The specification states of every timing it defines that its minimum and
//! maximum are configured on the **server**, and gives no numeric default for
//! any of them; a client must treat the values echoed back at bind time as the
//! only authoritative ones [`docs/spec/02-session-lifecycle.md` §8.7]. Every
//! default in this module is therefore **a choice of this crate**, documented
//! as such, and every one is overridable. None is presented as a protocol
//! requirement.
//!
//! # Where validation happens
//!
//! [`ServerAddress`](crate::ServerAddress) and
//! [`AdapterSet`](crate::AdapterSet) check themselves at construction, because
//! each is one value with one rule. The knobs in this module are checked
//! together, when [`ClientConfig::builder`](crate::ClientConfig::builder) is
//! built: several of them constrain each other (a retry ceiling below its
//! first delay, for instance) and checking them one at a time could not catch
//! that. Until then a [`ConnectionOptions`] is inert — it opens nothing and
//! sends nothing — so an unchecked one cannot reach a server.

use std::num::{NonZeroU32, NonZeroU64, NonZeroUsize};
use std::time::Duration;

/// Default limit on how long a session-opening request may go unanswered.
///
/// A choice of this crate; the specification defines no such limit.
const DEFAULT_OPEN_TIMEOUT: Duration = Duration::from_secs(10);

/// Default grace added to the server's own keepalive interval before a silent
/// stream is declared dead.
///
/// The specification requires a client-side "configurable timeout" on top of
/// the keepalive interval but names no value
/// [`docs/spec/02-session-lifecycle.md` §8.1]. Three seconds is this crate's
/// choice.
const DEFAULT_KEEPALIVE_SLACK: Duration = Duration::from_secs(3);

/// Default delay before the first reconnection attempt. A choice of this crate.
const DEFAULT_RETRY_INITIAL: Duration = Duration::from_millis(500);

/// Default ceiling on the reconnection delay. A choice of this crate.
const DEFAULT_RETRY_MAX: Duration = Duration::from_secs(30);

/// Default cap on consecutive failed reconnection attempts. A choice of this
/// crate.
const DEFAULT_RETRY_ATTEMPTS: u32 = 8;

/// Default capacity of the session-event stream.
const DEFAULT_SESSION_EVENT_CAPACITY: usize = 256;

/// Default capacity of each subscription's update stream.
const DEFAULT_UPDATE_CAPACITY: usize = 1024;

/// Which transport carries the session.
///
/// TLCP defines two: a WebSocket, and HTTP — the latter in a streaming and a
/// long-polling flavour [`docs/spec/01-foundations.md` §6]. All three are
/// implemented; the enum is `#[non_exhaustive]` so that a fourth could be added
/// later as a minor version bump rather than a breaking one.
///
/// Forcing a transport is offered because a caller sometimes knows something
/// this crate cannot discover — a proxy that mangles WebSocket upgrades, say,
/// for which one of the HTTP variants is the way through.
///
/// The two HTTP variants differ in how a stream connection ends: streaming
/// holds one response open until its content length is reached and then rebinds
/// (transition T3), whereas long polling returns each cycle's notifications and
/// rebinds every cycle (transition T4)
/// [`docs/spec/02-session-lifecycle.md` §2.2, §7]. WebSocket does neither.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
#[repr(u8)]
#[non_exhaustive]
pub enum Transport {
    /// A single WebSocket carrying both the stream and the control requests
    /// [`docs/spec/01-foundations.md` §6.2].
    #[default]
    WebSocket,
    /// HTTP streaming: one long-lived response body of near-infinite length,
    /// with each control request on its own connection
    /// [`docs/spec/01-foundations.md` §3; §6.1].
    HttpStreaming,
    /// HTTP long polling: each connection returns the notifications ready now
    /// and closes, and the session rebinds each cycle
    /// [`docs/spec/02-session-lifecycle.md` §7].
    HttpPolling,
}

impl Transport {
    /// A short name, for logs and error messages.
    #[must_use]
    #[inline]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::WebSocket => "websocket",
            Self::HttpStreaming => "http-streaming",
            Self::HttpPolling => "http-polling",
        }
    }
}

/// How reconnection attempts are spaced and how many are allowed.
///
/// When a stream connection breaks, this crate reconnects on its own — that is
/// the whole point of a protocol client. What it will not do is hide the
/// consequences: every attempt and every outcome is reported on the session
/// event stream, and when this policy's budget runs out the session is
/// reported definitively lost rather than retried forever in silence
/// (`docs/adr/0005-recovery-is-visible-in-the-event-stream.md`).
///
/// Delays follow **full jitter**: the base delay doubles after each failure up
/// to [`RetryPolicy::max_delay`], and the delay actually waited is drawn
/// uniformly from zero to that base. Jitter is this crate's choice, not a
/// protocol rule; it exists so that a fleet of clients knocked off a server at
/// the same instant does not return in lockstep.
///
/// A successful bind resets the schedule, so a session that reconnects cleanly
/// never carries delay over from an unrelated earlier failure.
///
/// # Examples
///
/// ```
/// use std::time::Duration;
/// use lightstreamer_rs::RetryPolicy;
///
/// // Give up after three failures instead of the default eight.
/// let policy = RetryPolicy::default().with_max_attempts(std::num::NonZeroU32::new(3));
/// assert_eq!(policy.max_attempts().map(|n| n.get()), Some(3));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryPolicy {
    initial_delay: Duration,
    max_delay: Duration,
    max_attempts: Option<NonZeroU32>,
}

impl Default for RetryPolicy {
    /// 500 ms growing to 30 s, giving up after 8 consecutive failures — which
    /// spans roughly two minutes of retrying. Every number is a choice of this
    /// crate.
    fn default() -> Self {
        Self {
            initial_delay: DEFAULT_RETRY_INITIAL,
            max_delay: DEFAULT_RETRY_MAX,
            max_attempts: NonZeroU32::new(DEFAULT_RETRY_ATTEMPTS),
        }
    }
}

impl RetryPolicy {
    /// Sets the delay before the first reconnection attempt.
    #[must_use = "builders do nothing unless the result is used"]
    pub const fn with_initial_delay(mut self, delay: Duration) -> Self {
        self.initial_delay = delay;
        self
    }

    /// Sets the ceiling the delay grows to and does not exceed.
    #[must_use = "builders do nothing unless the result is used"]
    pub const fn with_max_delay(mut self, delay: Duration) -> Self {
        self.max_delay = delay;
        self
    }

    /// Sets how many consecutive failures are allowed before the session is
    /// reported definitively lost.
    ///
    /// `None` retries forever. That is permitted, but then nothing ever tells
    /// the caller "this is not coming back" except the individual attempt
    /// events, so choose it deliberately.
    #[must_use = "builders do nothing unless the result is used"]
    pub const fn with_max_attempts(mut self, attempts: Option<NonZeroU32>) -> Self {
        self.max_attempts = attempts;
        self
    }

    /// The delay before the first reconnection attempt.
    #[must_use]
    #[inline]
    pub const fn initial_delay(&self) -> Duration {
        self.initial_delay
    }

    /// The ceiling on the reconnection delay.
    #[must_use]
    #[inline]
    pub const fn max_delay(&self) -> Duration {
        self.max_delay
    }

    /// How many consecutive failures are allowed, or `None` for no limit.
    #[must_use]
    #[inline]
    pub const fn max_attempts(&self) -> Option<NonZeroU32> {
        self.max_attempts
    }
}

/// Timeouts, buffer sizes and the reconnection policy for one client.
///
/// Every field has a default that works against a normally configured server;
/// change only what you have a reason to change. See the module documentation
/// for why these are checked when the [`ClientConfig`](crate::ClientConfig) is
/// built rather than one at a time.
///
/// # Examples
///
/// ```
/// use std::time::Duration;
/// use lightstreamer_rs::ConnectionOptions;
///
/// let options = ConnectionOptions::default()
///     .with_open_timeout(Duration::from_secs(5))
///     .with_send_sync(false);
/// assert_eq!(options.open_timeout(), Duration::from_secs(5));
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectionOptions {
    open_timeout: Duration,
    keepalive_slack: Duration,
    keepalive_hint: Option<Duration>,
    inactivity_commitment: Option<Duration>,
    send_sync: bool,
    content_length: Option<NonZeroU64>,
    retry: RetryPolicy,
    session_event_capacity: NonZeroUsize,
    update_capacity: NonZeroUsize,
}

impl Default for ConnectionOptions {
    fn default() -> Self {
        Self {
            open_timeout: DEFAULT_OPEN_TIMEOUT,
            keepalive_slack: DEFAULT_KEEPALIVE_SLACK,
            keepalive_hint: None,
            inactivity_commitment: None,
            send_sync: true,
            content_length: None,
            retry: RetryPolicy::default(),
            session_event_capacity: NonZeroUsize::new(DEFAULT_SESSION_EVENT_CAPACITY)
                .unwrap_or(NonZeroUsize::MIN),
            update_capacity: NonZeroUsize::new(DEFAULT_UPDATE_CAPACITY)
                .unwrap_or(NonZeroUsize::MIN),
        }
    }
}

impl ConnectionOptions {
    /// How long a session-opening request may go unanswered before the attempt
    /// is abandoned and retried.
    ///
    /// This is a limit of this crate: the protocol defines none, and without
    /// one a server that accepts a connection and then says nothing would wedge
    /// the client forever. Default: 10 s.
    #[must_use = "builders do nothing unless the result is used"]
    pub const fn with_open_timeout(mut self, timeout: Duration) -> Self {
        self.open_timeout = timeout;
        self
    }

    /// Extra grace beyond the server's own keepalive interval before a silent
    /// stream is treated as dead and recovered.
    ///
    /// The server promises to send *something* — a keepalive probe if nothing
    /// else — at least this often, and tells the client the interval when the
    /// session binds [`docs/spec/02-session-lifecycle.md` §8.1]. This value is
    /// added on top of that negotiated interval to absorb one late probe.
    /// Default: 3 s.
    ///
    /// The negotiated interval always wins over
    /// [`ConnectionOptions::with_keepalive_hint`]; this slack is the only part
    /// of the liveness budget the caller controls outright.
    #[must_use = "builders do nothing unless the result is used"]
    pub const fn with_keepalive_slack(mut self, slack: Duration) -> Self {
        self.keepalive_slack = slack;
        self
    }

    /// Asks the server for a particular keepalive interval.
    ///
    /// A **hint**, not a setting: the server clamps it to its own configured
    /// minimum and maximum and reports the interval it actually adopted, which
    /// is what this client then enforces
    /// [`docs/spec/02-session-lifecycle.md` §8.1]. Leave it unset to let the
    /// server choose, which is the default.
    #[must_use = "builders do nothing unless the result is used"]
    pub const fn with_keepalive_hint(mut self, interval: Option<Duration>) -> Self {
        self.keepalive_hint = interval;
        self
    }

    /// Commits to sending the server something at least this often.
    ///
    /// If the server hears nothing from the client for longer than this it may
    /// conclude the client is stuck and act on it
    /// [`docs/spec/03-requests.md` §2.1]. Setting it makes this crate send a
    /// heartbeat whenever nothing else has been sent, at half the committed
    /// interval — half being this crate's choice, so that one lost heartbeat
    /// does not break the commitment.
    ///
    /// Unset by default: without a commitment there is nothing to keep, and
    /// the server applies no such check.
    #[must_use = "builders do nothing unless the result is used"]
    pub const fn with_inactivity_commitment(mut self, interval: Option<Duration>) -> Self {
        self.inactivity_commitment = interval;
        self
    }

    /// Whether to let the server send its periodic clock-synchronisation
    /// notifications.
    ///
    /// They carry the seconds elapsed on the server since the session began,
    /// which lets a client detect that its own machine has been suspended
    /// [`docs/spec/03-requests.md` §2.1]. This crate surfaces them on the
    /// session event stream and does not act on them. Default: enabled.
    #[must_use = "builders do nothing unless the result is used"]
    pub const fn with_send_sync(mut self, enabled: bool) -> Self {
        self.send_sync = enabled;
        self
    }

    /// Asks the server to end each stream connection after this many bytes,
    /// forcing a clean rebind.
    ///
    /// Chiefly a defence against intermediaries that buffer an endless
    /// response; the server raises a value it considers too low
    /// [`docs/spec/03-requests.md` §2.1]. Leave it unset to let the server
    /// choose, which is the default. A rebind triggered this way preserves the
    /// session and every subscription.
    #[must_use = "builders do nothing unless the result is used"]
    pub const fn with_content_length(mut self, bytes: Option<NonZeroU64>) -> Self {
        self.content_length = bytes;
        self
    }

    /// Sets the reconnection policy.
    #[must_use = "builders do nothing unless the result is used"]
    pub const fn with_retry(mut self, retry: RetryPolicy) -> Self {
        self.retry = retry;
        self
    }

    /// Capacity of the client's session event stream.
    ///
    /// The stream is bounded and **never drops**: see
    /// [`SessionEvents`](crate::SessionEvents) for what that means for a
    /// consumer that falls behind. Default: 256 events.
    #[must_use = "builders do nothing unless the result is used"]
    pub const fn with_session_event_capacity(mut self, capacity: NonZeroUsize) -> Self {
        self.session_event_capacity = capacity;
        self
    }

    /// Capacity of each subscription's update stream.
    ///
    /// Bounded, and **never drops**: see [`Updates`](crate::Updates). Default:
    /// 1024 events per subscription.
    #[must_use = "builders do nothing unless the result is used"]
    pub const fn with_update_capacity(mut self, capacity: NonZeroUsize) -> Self {
        self.update_capacity = capacity;
        self
    }

    /// How long a session-opening request may go unanswered.
    #[must_use]
    #[inline]
    pub const fn open_timeout(&self) -> Duration {
        self.open_timeout
    }

    /// Grace beyond the negotiated keepalive interval.
    #[must_use]
    #[inline]
    pub const fn keepalive_slack(&self) -> Duration {
        self.keepalive_slack
    }

    /// The keepalive interval asked of the server, if any.
    #[must_use]
    #[inline]
    pub const fn keepalive_hint(&self) -> Option<Duration> {
        self.keepalive_hint
    }

    /// The inactivity commitment made to the server, if any.
    #[must_use]
    #[inline]
    pub const fn inactivity_commitment(&self) -> Option<Duration> {
        self.inactivity_commitment
    }

    /// Whether clock-synchronisation notifications are enabled.
    #[must_use]
    #[inline]
    pub const fn send_sync(&self) -> bool {
        self.send_sync
    }

    /// The requested stream-connection byte budget, if any.
    #[must_use]
    #[inline]
    pub const fn content_length(&self) -> Option<NonZeroU64> {
        self.content_length
    }

    /// The reconnection policy.
    #[must_use]
    #[inline]
    pub const fn retry(&self) -> RetryPolicy {
        self.retry
    }

    /// Capacity of the session event stream.
    #[must_use]
    #[inline]
    pub const fn session_event_capacity(&self) -> NonZeroUsize {
        self.session_event_capacity
    }

    /// Capacity of each subscription's update stream.
    #[must_use]
    #[inline]
    pub const fn update_capacity(&self) -> NonZeroUsize {
        self.update_capacity
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_options_defaults_are_the_documented_ones() {
        let options = ConnectionOptions::default();
        assert_eq!(options.open_timeout(), Duration::from_secs(10));
        assert_eq!(options.keepalive_slack(), Duration::from_secs(3));
        assert_eq!(options.keepalive_hint(), None);
        assert_eq!(options.inactivity_commitment(), None);
        assert!(options.send_sync());
        assert_eq!(options.content_length(), None);
    }

    #[test]
    fn test_retry_defaults_are_the_documented_ones() {
        let retry = RetryPolicy::default();
        assert_eq!(retry.initial_delay(), Duration::from_millis(500));
        assert_eq!(retry.max_delay(), Duration::from_secs(30));
        assert_eq!(retry.max_attempts().map(NonZeroU32::get), Some(8));
    }

    #[test]
    fn test_retry_max_attempts_can_be_removed() {
        let retry = RetryPolicy::default().with_max_attempts(None);
        assert_eq!(retry.max_attempts(), None);
    }

    #[test]
    fn test_options_setters_round_trip() {
        let options = ConnectionOptions::default()
            .with_open_timeout(Duration::from_secs(4))
            .with_keepalive_slack(Duration::from_secs(1))
            .with_keepalive_hint(Some(Duration::from_secs(5)))
            .with_inactivity_commitment(Some(Duration::from_secs(20)))
            .with_send_sync(false)
            .with_content_length(NonZeroU64::new(1_000_000));
        assert_eq!(options.open_timeout(), Duration::from_secs(4));
        assert_eq!(options.keepalive_slack(), Duration::from_secs(1));
        assert_eq!(options.keepalive_hint(), Some(Duration::from_secs(5)));
        assert_eq!(
            options.inactivity_commitment(),
            Some(Duration::from_secs(20))
        );
        assert!(!options.send_sync());
        assert_eq!(
            options.content_length().map(NonZeroU64::get),
            Some(1_000_000)
        );
    }

    #[test]
    fn test_transport_default_is_websocket() {
        assert_eq!(Transport::default(), Transport::WebSocket);
        assert_eq!(Transport::default().as_str(), "websocket");
    }

    #[test]
    fn test_transport_variants_name_themselves() {
        assert_eq!(Transport::HttpStreaming.as_str(), "http-streaming");
        assert_eq!(Transport::HttpPolling.as_str(), "http-polling");
    }
}
