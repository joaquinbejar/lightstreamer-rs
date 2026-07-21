//! What the client tells you about the session itself.
//!
//! Everything here answers one question an application holding derived state
//! has to be able to answer: **is what I computed still valid?** TLCP
//! distinguishes an interruption the session survived from one that replaced
//! it, and a client that hid the difference would force every application
//! either to rebuild its state on every hiccup or to be silently wrong after a
//! real one (`docs/adr/0005-recovery-is-visible-in-the-event-stream.md`).
//!
//! The three answers a caller must be able to tell apart are:
//!
//! | You see | Your derived state is |
//! |---|---|
//! | [`SessionEvent::Connected`] with [`Continuity::Preserved`] or [`Continuity::Recovered`] | still valid |
//! | [`SessionEvent::Connected`] with [`Continuity::Replaced`] | invalid — discard and rebuild |
//! | [`SessionEvent::Closed`] | invalid, and nothing more is coming |

use std::time::Duration;

use crate::client::message::MessageOutcome;
use crate::error::{Error, ServerError};
use crate::protocol::response::{Bandwidth, Notification};
use crate::session::{
    BindKind, BoundInfo, RecoveryKind, RecoveryOutcome as WireRecovery, ResubscribedEntry,
    SessionClosed as WireClosed, SubscriptionKey, UnbindReason,
};

/// An opaque handle identifying one subscription for as long as you hold it.
///
/// It survives everything the session does — a rebind, a recovery, and even a
/// replacement of the session by a new one. That is deliberate: the protocol's
/// own subscription numbers are only unique *within* a session and restart at
/// one when a session is replaced [`docs/spec/02-session-lifecycle.md` §4.4],
/// so a caller given those numbers would see its subscriptions change identity
/// for reasons that have nothing to do with them.
///
/// Get one from [`Updates::id`](crate::Updates::id).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SubscriptionId(SubscriptionKey);

impl SubscriptionId {
    /// An opaque number, for logging and for use as a map key.
    ///
    /// Unique within one client. It is **not** the protocol's `LS_subId` and
    /// must not be sent anywhere.
    #[must_use]
    #[inline]
    pub const fn get(self) -> u64 {
        self.0.get()
    }

    /// Wraps the session layer's key.
    pub(crate) const fn new(key: SubscriptionKey) -> Self {
        Self(key)
    }

    /// The session layer's key.
    pub(crate) const fn key(self) -> SubscriptionKey {
        self.0
    }

    /// An id with a chosen value, for [`crate::test_util`].
    #[cfg(feature = "test-util")]
    #[must_use]
    pub(crate) const fn from_raw(id: u64) -> Self {
        Self(SubscriptionKey::from_raw(id))
    }
}

impl std::fmt::Display for SubscriptionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "subscription#{}", self.0.get())
    }
}

/// What a newly bound session means for state you built on the previous one.
///
/// This is the distinction ADR-0005 exists for. Read it as: *may I keep what I
/// computed?*
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Continuity {
    /// The first session of this client. There is nothing to keep or discard.
    New,

    /// The **same** session, rebound over a fresh connection after a clean
    /// handover — the server told the client to reconnect and guaranteed that
    /// nothing was dropped in between
    /// [`docs/spec/02-session-lifecycle.md` §4.4].
    ///
    /// Every subscription is intact, server-side, with all its items and
    /// fields. Keep your state.
    Preserved,

    /// The **same** session, resumed after an interruption the client
    /// recovered from.
    ///
    /// Whether anything was actually missed is not known yet at this point:
    /// the server answers separately, and this client reports the answer as
    /// [`SessionEvent::Recovered`] immediately afterwards. Keep your state,
    /// and read that next event before trusting it completely.
    Recovered {
        /// The point in the session's notification count the client asked to
        /// resume from.
        requested_from: u64,
    },

    /// A **new** session replaced one that was lost.
    ///
    /// Nothing carries over: the notification count restarts at zero and every
    /// subscription is created again from scratch
    /// [`docs/spec/02-session-lifecycle.md` §6.1]. Any state you derived from
    /// the previous session is stale — discard it and rebuild from the
    /// snapshots that follow.
    Replaced {
        /// The identifier of the session this one replaces, when there was one
        /// and the client knew it.
        previous_session_id: Option<String>,
    },
}

impl Continuity {
    /// Whether state derived from the previous session is still valid.
    ///
    /// `false` for [`Continuity::Replaced`] only; `true` for
    /// [`Continuity::New`] as well, where there is no previous state to
    /// invalidate.
    #[must_use]
    #[inline]
    pub const fn is_preserved(&self) -> bool {
        !matches!(self, Self::Replaced { .. })
    }
}

impl From<BindKind> for Continuity {
    fn from(kind: BindKind) -> Self {
        match kind {
            BindKind::Created => Self::New,
            BindKind::Rebound => Self::Preserved,
            BindKind::Recovering {
                requested_progressive,
            } => Self::Recovered {
                requested_from: requested_progressive,
            },
            BindKind::Recreated { previous } => Self::Replaced {
                previous_session_id: previous.map(|id| id.as_str().to_owned()),
            },
        }
    }
}

/// The session is bound to a server and streaming.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Connected {
    /// The server's identifier for this session. Opaque; useful in logs and
    /// for correlating with server-side diagnostics.
    pub session_id: String,
    /// What this bind means for state derived from an earlier one.
    pub continuity: Continuity,
    /// The longest the server will let the connection go quiet before sending
    /// something, even if there is no data
    /// [`docs/spec/02-session-lifecycle.md` §8.1].
    ///
    /// This is the value the server chose, not the one you asked for, and it
    /// is what this client's own liveness check is built from.
    pub keepalive: Duration,
    /// The largest request, in bytes, the server will accept on this session
    /// [`docs/spec/02-session-lifecycle.md` §3.1].
    pub request_limit_bytes: u64,
}

#[cfg(feature = "test-util")]
impl Connected {
    /// Assembles one field by field, for [`crate::test_util`].
    #[must_use]
    pub(crate) const fn from_parts(
        session_id: String,
        continuity: Continuity,
        keepalive: Duration,
        request_limit_bytes: u64,
    ) -> Self {
        Self {
            session_id,
            continuity,
            keepalive,
            request_limit_bytes,
        }
    }
}

impl From<BoundInfo> for Connected {
    fn from(info: BoundInfo) -> Self {
        Self {
            session_id: info.session_id.as_str().to_owned(),
            continuity: info.kind.into(),
            keepalive: info.keep_alive,
            request_limit_bytes: info.request_limit_bytes,
        }
    }
}

/// Where a recovered flow actually resumed, relative to where it was asked to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct Recovery {
    /// The point the client asked to resume from.
    pub requested_from: u64,
    /// The point the server actually resumed from.
    pub resumed_at: u64,
    /// What the difference between the two means.
    pub outcome: RecoveryOutcome,
}

/// The three ways a recovery can turn out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum RecoveryOutcome {
    /// The server resumed exactly where the client asked. Nothing was lost and
    /// nothing was duplicated: continuity is intact.
    Exact,

    /// The server resumed **earlier** than asked, which it is allowed to do,
    /// and re-sent notifications the client already had. This client discarded
    /// them before you saw them, so continuity is still intact
    /// [`docs/spec/02-session-lifecycle.md` §5.2].
    Duplicated {
        /// How many re-delivered notifications were suppressed.
        suppressed: u64,
    },

    /// The server resumed **later** than asked: the notifications in between
    /// are gone and will not arrive.
    ///
    /// State derived from the affected subscriptions may be stale. The
    /// protocol does not say this can happen and does not say what a client
    /// should do about it, so this crate reports it rather than deciding for
    /// you [`docs/spec/02-session-lifecycle.md` §5.4].
    Gap {
        /// How many notifications were skipped.
        missing: u64,
    },
}

impl Recovery {
    /// Whether nothing was lost.
    #[must_use]
    #[inline]
    pub const fn is_lossless(&self) -> bool {
        !matches!(self.outcome, RecoveryOutcome::Gap { .. })
    }

    /// Assembles one field by field, for [`crate::test_util`].
    #[cfg(feature = "test-util")]
    #[must_use]
    pub(crate) const fn from_parts(
        requested_from: u64,
        resumed_at: u64,
        outcome: RecoveryOutcome,
    ) -> Self {
        Self {
            requested_from,
            resumed_at,
            outcome,
        }
    }
}

impl From<WireRecovery> for Recovery {
    fn from(outcome: WireRecovery) -> Self {
        Self {
            requested_from: outcome.requested,
            resumed_at: outcome.resumed_at,
            outcome: match outcome.kind {
                RecoveryKind::Exact => RecoveryOutcome::Exact,
                RecoveryKind::Duplicated { count } => {
                    RecoveryOutcome::Duplicated { suppressed: count }
                }
                RecoveryKind::Gap { missing } => RecoveryOutcome::Gap { missing },
            },
        }
    }
}

/// One subscription re-created on a session that replaced a lost one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct Resubscribed {
    /// Which subscription — the same handle you have been holding all along.
    pub id: SubscriptionId,
    /// Whether the server had confirmed this subscription before the session
    /// was lost. `false` means it was still pending and never became active.
    pub was_active: bool,
    /// Whether this subscription asked for a snapshot, and so restarts from a
    /// complete picture rather than resuming mid-flow.
    ///
    /// If this is `false`, an application that discarded its state because of
    /// a [`Continuity::Replaced`] has no way to rebuild it except by waiting
    /// for every field to tick again. That is a property of the subscription,
    /// not a failure.
    pub snapshot_restarting: bool,
}

#[cfg(feature = "test-util")]
impl Resubscribed {
    /// Assembles one field by field, for [`crate::test_util`].
    #[must_use]
    pub(crate) const fn from_parts(
        id: SubscriptionId,
        was_active: bool,
        snapshot_restarting: bool,
    ) -> Self {
        Self {
            id,
            was_active,
            snapshot_restarting,
        }
    }
}

impl From<ResubscribedEntry> for Resubscribed {
    fn from(entry: ResubscribedEntry) -> Self {
        Self {
            id: SubscriptionId::new(entry.key),
            was_active: entry.previously_active,
            snapshot_restarting: entry.snapshot_requested,
        }
    }
}

/// Why the session stopped streaming.
///
/// A disconnection is not the end: this client reconnects on its own, and
/// [`SessionEvent::Disconnected`] carries how long until the next attempt. It
/// is the end only when [`SessionEvent::Closed`] follows.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum DisconnectReason {
    /// The server handed the connection over cleanly and expects the client to
    /// reconnect. The session itself is untouched and every subscription
    /// survives [`docs/spec/02-session-lifecycle.md` §4.4].
    ///
    /// Routine: it is how TLCP rotates long-lived connections.
    Handover,

    /// The connection failed or was dropped. The session is not closed and a
    /// recovery is attempted [`docs/spec/02-session-lifecycle.md` §2.2].
    ConnectionFailed {
        /// What the transport reported. Diagnostic text; never a credential.
        detail: String,
    },

    /// The connection went silent for longer than the server promised, without
    /// ever failing.
    ///
    /// This is the failure mode a protocol client exists to catch: a socket
    /// that is wedged but never errors. The connection is torn down and
    /// recovered [`docs/spec/02-session-lifecycle.md` §8.1].
    Stalled {
        /// The silence budget that elapsed with no traffic of any kind.
        budget: Duration,
    },

    /// The server refused the session with a code that still allows another
    /// attempt.
    Refused(ServerError),

    /// The server ended the session in order to refresh it, and expects a new
    /// one to be opened immediately — Appendix A code `48`
    /// [`docs/spec/05-error-codes.md` §2].
    ///
    /// Nothing is wrong, but the session that follows is a **new** one: expect
    /// a [`Continuity::Replaced`] next.
    Refreshing(ServerError),
}

impl From<UnbindReason> for DisconnectReason {
    fn from(reason: UnbindReason) -> Self {
        match reason {
            UnbindReason::ForcedByClient { .. }
            | UnbindReason::ContentLengthReached { .. }
            | UnbindReason::PollCycleExpired { .. }
            | UnbindReason::Looped { .. } => Self::Handover,
            UnbindReason::ConnectionFailed { detail } => Self::ConnectionFailed { detail },
            UnbindReason::KeepaliveExpired { budget } => Self::Stalled { budget },
            UnbindReason::Rejected { cause } => Self::Refused(cause.into()),
            UnbindReason::ServerRefresh { cause } => Self::Refreshing(cause.into()),
        }
    }
}

/// Why the session ended for good.
///
/// After this the client is inert: every operation returns
/// [`Error::Disconnected`] and no further event arrives.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ClosedReason {
    /// You asked for it — [`Client::disconnect`](crate::Client::disconnect),
    /// or the client was dropped.
    ByClient,

    /// The server ended the session, or refused to start one, with an
    /// Appendix A code that admits no retry
    /// [`docs/spec/05-error-codes.md` §2].
    ByServer(ServerError),

    /// The connection kept failing and the reconnection budget ran out.
    ReconnectExhausted {
        /// How many consecutive attempts failed.
        attempts: u32,
    },

    /// A failure inside this crate. A bug; please report it.
    Internal {
        /// What failed. Never contains a credential.
        reason: String,
    },
}

impl ClosedReason {
    /// The same reason expressed as an [`Error`], for a caller that wants to
    /// propagate it with `?`.
    ///
    /// # Examples
    ///
    /// ```
    /// use lightstreamer_rs::{ClosedReason, Error};
    ///
    /// let reason = ClosedReason::ReconnectExhausted { attempts: 8 };
    /// assert!(matches!(reason.into_error(), Error::ReconnectExhausted { attempts: 8 }));
    /// ```
    #[must_use]
    pub fn into_error(self) -> Error {
        match self {
            Self::ByClient => Error::Disconnected,
            Self::ByServer(cause) => Error::Session(cause),
            Self::ReconnectExhausted { attempts } => Error::ReconnectExhausted { attempts },
            Self::Internal { reason } => Error::Internal { reason },
        }
    }
}

impl From<WireClosed> for ClosedReason {
    fn from(closed: WireClosed) -> Self {
        match closed {
            WireClosed::ByClient { .. } => Self::ByClient,
            WireClosed::ByServer { cause } => Self::ByServer(cause.into()),
            WireClosed::RetriesExhausted { attempts, .. } => Self::ReconnectExhausted { attempts },
            WireClosed::Internal { reason } => Self::Internal { reason },
        }
    }
}

/// Something the server said about itself or about the session, which changes
/// nothing but may be worth logging.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ServerInfo {
    /// The configured name of the server port the session is bound to.
    /// Diagnostic only [`docs/spec/04-notifications.md` §5.4].
    ServerName(String),

    /// The client's own address, as the server sees it. A change between two
    /// binds means the client moved network
    /// [`docs/spec/04-notifications.md` §5.3].
    ClientIp(String),

    /// The bandwidth limit now in force for the session
    /// [`docs/spec/04-notifications.md` §5.1].
    BandwidthLimit {
        /// The limit in kbps, exactly as the server wrote it, or `None` when
        /// the server declared no limit.
        kbps: Option<String>,
        /// Whether the client is forbidden to change the limit — the server's
        /// `unmanaged` answer.
        managed: bool,
    },

    /// How long the server thinks the session has been bound
    /// [`docs/spec/04-notifications.md` §5.2].
    ///
    /// Comparing it with your own elapsed time detects a client falling behind
    /// the update flow, or a machine that was suspended. This crate reports it
    /// and acts on it in no way.
    Clock {
        /// Seconds elapsed on the server since the session was bound.
        elapsed_seconds: u64,
    },
}

impl ServerInfo {
    /// Translates the session-level notifications that carry no state change.
    ///
    /// Returns `None` for anything else, so a notification that later becomes
    /// meaningful is not silently mislabelled as server info.
    pub(crate) fn from_notification(notification: Notification) -> Option<Self> {
        match notification {
            Notification::ServerName { name } => Some(Self::ServerName(name)),
            Notification::ClientIp { address } => Some(Self::ClientIp(address)),
            Notification::Sync { elapsed_seconds } => Some(Self::Clock { elapsed_seconds }),
            Notification::ConstraintsChanged { bandwidth } => Some(match bandwidth {
                Bandwidth::Limited { kbps } => Self::BandwidthLimit {
                    kbps: Some(kbps),
                    managed: true,
                },
                Bandwidth::Unlimited => Self::BandwidthLimit {
                    kbps: None,
                    managed: true,
                },
                Bandwidth::Unmanaged => Self::BandwidthLimit {
                    kbps: None,
                    managed: false,
                },
            }),
            _ => None,
        }
    }
}

/// Everything the client tells you about the session.
///
/// Delivered by [`SessionEvents`](crate::SessionEvents). The enum is closed
/// but `#[non_exhaustive]`: match it exhaustively with a `_` arm, and a future
/// version adding a variant will not break your build.
///
/// # Examples
///
/// ```no_run
/// use futures_util::StreamExt;
/// use lightstreamer_rs::{Continuity, SessionEvent};
///
/// # async fn run(mut events: lightstreamer_rs::SessionEvents) {
/// while let Some(event) = events.next().await {
///     match event {
///         SessionEvent::Connected(connected) => {
///             if !connected.continuity.is_preserved() {
///                 // A new session replaced the old one: rebuild from scratch.
///             }
///         }
///         SessionEvent::Closed(reason) => {
///             tracing::error!(?reason, "session is over");
///             break;
///         }
///         _ => {}
///     }
/// }
/// # }
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum SessionEvent {
    /// The session is bound to a server and streaming. Read
    /// [`Connected::continuity`] to learn whether your derived state survived.
    Connected(Box<Connected>),

    /// A recovery reported where the flow resumed. Always follows a
    /// [`SessionEvent::Connected`] carrying [`Continuity::Recovered`].
    Recovered(Recovery),

    /// Subscriptions were re-created on a newly bound session.
    ///
    /// Emitted whenever any subscription had to be issued at bind time, which
    /// after a session replacement is all of them. Your
    /// [`Updates`](crate::Updates) streams keep working across this; the event
    /// exists so you know a fresh snapshot is on its way.
    Resubscribed(Vec<Resubscribed>),

    /// The session stopped streaming. A reconnection is already scheduled
    /// unless `retry_in` is `None`, which means the client is about to give up
    /// and a [`SessionEvent::Closed`] follows.
    Disconnected {
        /// Why it stopped.
        reason: DisconnectReason,
        /// How long until the next attempt, or `None` when there will not be
        /// one.
        retry_in: Option<Duration>,
    },

    /// What became of a message you sent with
    /// [`Client::send_message`](crate::Client::send_message)
    /// [`docs/spec/03-requests.md` §12].
    ///
    /// Only ever arrives for a message that carried a progressive; a
    /// fire-and-forget one is reported here only if it never left this client
    /// or the server refused the request outright.
    Message(Box<MessageOutcome>),

    /// The server said something diagnostic.
    ServerInfo(ServerInfo),

    /// The server refused a control request, with a code from **Appendix B**
    /// [`docs/spec/05-error-codes.md` §2]. The session is unaffected.
    ///
    /// A refused **subscription request** is reported on that subscription's
    /// own stream instead, as
    /// [`SubscriptionEvent::Rejected`](crate::SubscriptionEvent::Rejected),
    /// which is where you are looking for it. What lands here is everything
    /// else:
    ///
    /// - a refused **unsubscription** or **reconfiguration** — note that in
    ///   both cases the subscription is **still active**; it is your request
    ///   to change it that failed, not the subscription that died, and its
    ///   stream keeps delivering;
    /// - a refused request that belongs to the session as a whole;
    /// - a refusal the server sent with no identifier at all, which the
    ///   protocol allows when it could not parse the request
    ///   [`docs/spec/03-requests.md` §13.2].
    ///
    /// Every [`ServerError`] carried here is something a server actually said.
    /// A request that failed before reaching one is
    /// [`SessionEvent::RequestNotSent`] instead.
    RequestRejected(ServerError),

    /// A control request never reached the server, so no server refused it.
    ///
    /// This is a **local** failure — the connection was down, or the request
    /// could not be encoded — and it is deliberately not a [`ServerError`].
    /// Giving it a fabricated code would be indistinguishable from a code the
    /// server's Metadata Adapter supplied
    /// (see [`ServerError::is_adapter_defined`]), which is a claim this crate
    /// is in no position to make.
    ///
    /// Subscription requests are exempt: an `add` that never left is reported
    /// on the subscription's own stream as
    /// [`SubscriptionEvent::Deferred`](crate::SubscriptionEvent::Deferred),
    /// because it is still coming.
    RequestNotSent {
        /// Why the request could not be sent.
        reason: String,
    },

    /// A line arrived that this client could not parse.
    ///
    /// Never fatal, and deliberately surfaced rather than swallowed: a server
    /// newer than this crate must not be able to break it, and you should be
    /// able to see that it happened.
    Unrecognized {
        /// The line exactly as it arrived, with its terminator stripped.
        line: String,
    },

    /// Terminal. The session is over and no further event will arrive.
    Closed(ClosedReason),
}

/// The stream of session-level events.
///
/// Handed back by [`Client::connect`](crate::Client::connect), alongside the
/// client itself, because a caller that never reads it should have to decide
/// that on purpose rather than by omission. It implements
/// [`futures_util::Stream`].
///
/// # Dropping this stream
///
/// Dropping it means "I do not want session events". The client keeps running
/// exactly as before — reconnection, recovery and resubscription all still
/// happen — you simply stop being told about them. Nothing blocks and nothing
/// leaks.
///
/// # Backpressure: bounded, and nothing is dropped
///
/// The stream is fed by a **bounded** channel of
/// [`ConnectionOptions::with_session_event_capacity`](crate::ConnectionOptions::with_session_event_capacity)
/// events — 256 by default. While you hold it, the client **blocks** rather
/// than discarding an event when the buffer is full, and that backpressure
/// reaches the socket. A held-but-unread stream will therefore stall the
/// client once it fills.
///
/// The reasoning is the same as for [`Updates`](crate::Updates): these events
/// are what tell an application whether its derived state survived, so
/// silently dropping one would be worse than making a slow consumer visible.
/// If you do not intend to read them, drop the stream — that is the supported
/// way to opt out.
///
/// A stall longer than the session's keepalive interval provokes a
/// reconnection, for the reason set out under
/// [`Updates`](crate::Updates#what-a-long-stall-actually-does). Holding this
/// stream and not reading it is the one shape to avoid.
///
/// # Examples
///
/// ```no_run
/// # use lightstreamer_rs::{Client, ClientConfig, ServerAddress};
/// # async fn run(config: ClientConfig) -> lightstreamer_rs::Result<()> {
/// let (client, events) = Client::connect(config).await?;
/// // Not interested: opt out explicitly, and nothing ever blocks on it.
/// drop(events);
/// # Ok(())
/// # }
/// ```
#[derive(Debug)]
pub struct SessionEvents {
    events: tokio::sync::mpsc::Receiver<SessionEvent>,
}

impl SessionEvents {
    /// Wraps the receiving half the router publishes to.
    pub(crate) const fn new(events: tokio::sync::mpsc::Receiver<SessionEvent>) -> Self {
        Self { events }
    }
}

impl futures_util::Stream for SessionEvents {
    type Item = SessionEvent;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        self.events.poll_recv(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::{ServerCause, SessionId};

    #[test]
    fn test_continuity_distinguishes_the_three_adr_0005_cases() {
        // Continuity preserved.
        assert!(Continuity::from(BindKind::Rebound).is_preserved());
        assert!(
            Continuity::from(BindKind::Recovering {
                requested_progressive: 42
            })
            .is_preserved()
        );
        // Session re-established.
        let replaced = Continuity::from(BindKind::Recreated {
            previous: Some(SessionId::new("S1")),
        });
        assert!(!replaced.is_preserved());
        assert!(matches!(
            replaced,
            Continuity::Replaced { previous_session_id } if previous_session_id.as_deref() == Some("S1")
        ));
        // First session of all.
        assert!(matches!(
            Continuity::from(BindKind::Created),
            Continuity::New
        ));
    }

    #[test]
    fn test_continuity_carries_the_requested_recovery_point() {
        assert!(matches!(
            Continuity::from(BindKind::Recovering {
                requested_progressive: 17
            }),
            Continuity::Recovered { requested_from: 17 }
        ));
    }

    #[test]
    fn test_recovery_reports_a_gap_as_lossy() {
        let exact = Recovery::from(WireRecovery {
            requested: 10,
            resumed_at: 10,
            kind: RecoveryKind::Exact,
        });
        assert!(exact.is_lossless());

        let duplicated = Recovery::from(WireRecovery {
            requested: 10,
            resumed_at: 7,
            kind: RecoveryKind::Duplicated { count: 3 },
        });
        assert!(duplicated.is_lossless());
        assert!(matches!(
            duplicated.outcome,
            RecoveryOutcome::Duplicated { suppressed: 3 }
        ));

        let gap = Recovery::from(WireRecovery {
            requested: 10,
            resumed_at: 14,
            kind: RecoveryKind::Gap { missing: 4 },
        });
        assert!(!gap.is_lossless());
    }

    #[test]
    fn test_disconnect_reason_maps_a_clean_handover_apart_from_a_failure() {
        assert!(matches!(
            DisconnectReason::from(UnbindReason::ContentLengthReached {
                expected_delay: Duration::ZERO
            }),
            DisconnectReason::Handover
        ));
        assert!(matches!(
            DisconnectReason::from(UnbindReason::KeepaliveExpired {
                budget: Duration::from_secs(8)
            }),
            DisconnectReason::Stalled { .. }
        ));
        assert!(matches!(
            DisconnectReason::from(UnbindReason::ServerRefresh {
                cause: ServerCause {
                    code: 48,
                    message: String::new()
                }
            }),
            DisconnectReason::Refreshing(cause) if cause.code() == 48
        ));
    }

    #[test]
    fn test_closed_reason_round_trips_into_an_error() {
        assert!(matches!(
            ClosedReason::ByClient.into_error(),
            Error::Disconnected
        ));
        assert!(matches!(
            ClosedReason::ByServer(ServerError::new(2, "no adapter set")).into_error(),
            Error::Session(cause) if cause.code() == 2
        ));
        assert!(matches!(
            ClosedReason::Internal {
                reason: "boom".to_owned()
            }
            .into_error(),
            Error::Internal { .. }
        ));
    }

    #[test]
    fn test_server_info_translates_the_diagnostic_notifications() {
        assert!(matches!(
            ServerInfo::from_notification(Notification::ServerName {
                name: "Lightstreamer HTTP Server".to_owned()
            }),
            Some(ServerInfo::ServerName(name)) if name.contains("Lightstreamer")
        ));
        assert!(matches!(
            ServerInfo::from_notification(Notification::Sync {
                elapsed_seconds: 12
            }),
            Some(ServerInfo::Clock {
                elapsed_seconds: 12
            })
        ));
        assert!(matches!(
            ServerInfo::from_notification(Notification::ConstraintsChanged {
                bandwidth: Bandwidth::Unmanaged
            }),
            Some(ServerInfo::BandwidthLimit {
                kbps: None,
                managed: false
            })
        ));
        // Anything that is not diagnostic is not mislabelled as diagnostic.
        assert!(
            ServerInfo::from_notification(Notification::Unsubscribed { subscription_id: 1 })
                .is_none()
        );
    }

    #[test]
    fn test_unrecognized_lines_are_surfaced_verbatim() {
        // "Unknown is not fatal": the raw line reaches the caller unchanged.
        let event = SessionEvent::Unrecognized {
            line: "FUTURE,1,2,3".to_owned(),
        };
        assert!(matches!(event, SessionEvent::Unrecognized { line } if line == "FUTURE,1,2,3"));
    }
}
