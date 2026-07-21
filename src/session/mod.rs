//! The session lifecycle state machine: create → bind → loop/rebind →
//! recovery, and the terminal paths out of all of them.
//!
//! This is the only layer that combines the two halves of the client: it
//! drives a [`Transport`], feeds every line it yields through
//! [`crate::protocol::response::parse_line`], and keeps the session alive.
//! Everything protocol-visible here is derived from
//! `docs/spec/02-session-lifecycle.md`, whose §2.2 transition table (T1–T9) is
//! implemented literally and cited transition by transition.
//!
//! # The states
//!
//! The specification defines exactly two session states — **Bound: Session is
//! Streaming** and **Unbound: Session is Buffering** — plus the unnamed
//! pre-session and destroyed endpoints of its state diagram
//! [`docs/spec/02-session-lifecycle.md` §2.1, §2.3]. This module keeps that
//! vocabulary. [`StreamState`] refines *Bound* into the moment before `CONOK`
//! has arrived and the moment after, because a client must time out an
//! unanswered handshake and the spec's two states give it nowhere to say so;
//! the unbound state is not a variant here at all but the interval between two
//! turns of the driver's loop, which is where it naturally lives.
//!
//! # What the caller is told, and why
//!
//! `docs/adr/0005-recovery-is-visible-in-the-event-stream.md` is binding: an
//! application holding derived state must be able to tell an interruption that
//! **preserved continuity** from one that **replaced the session**, and both
//! from a session that is **definitively gone**. That is why [`BindKind`],
//! [`RecoveryOutcome`] and [`SessionClosed`] are separate types with separate
//! variants rather than a single "reconnected" signal: the distinction exists
//! in the protocol, and hiding it would force every application either to
//! rebuild state on every hiccup or to be silently wrong after a real one.
//!
//! # Transport independence
//!
//! Every transport-dependent decision here reads a declared
//! [`TransportProperties`] flag. There is no match on a concrete transport type
//! in this module, and adding a fourth transport requires no change to it
//! (`docs/adr/0007-transport-port-shape.md`).
//!
//! # Concurrency and shutdown
//!
//! [`SessionDriver::run`] is the whole runtime of this layer: one future, no
//! spawned tasks, no detached work. Whoever spawns it owns its lifetime, and
//! every exit path — terminal notification, exhausted retries, caller
//! shutdown, or a dropped [`SessionHandle`] — closes the transport before
//! returning. The driver requires [`Transport::next_line`] to be cancel-safe,
//! since it is polled inside a `select!` alongside the command channel and the
//! liveness timer; a line must not be lost when that future is dropped.
//!
//! Both channels are bounded. The event channel applies backpressure rather
//! than dropping, because a dropped data notification would desynchronise the
//! recovery progressive that makes recovery correct
//! [`docs/spec/02-session-lifecycle.md` §5.2].
//!
//! # The stop lane
//!
//! Backpressure has one exception, and it is deliberate: **an ordered stop is
//! never subject to it**. A caller that stops reading its event stream blocks
//! the driver, which is the documented contract — but it must not thereby make
//! the client unstoppable, because that would leak a task and a socket for the
//! life of the process. [`SessionHandle::stop`] therefore raises a signal that
//! every blocking wait in this module races against: a full event channel, a
//! reconnection delay, and the establishment of a stream connection all give it
//! up at once. Any event that had not yet been delivered when a stop is ordered
//! is discarded, since the stream it was going to is about to end anyway.

// Several of this module's `pub(crate)` operations — `reconfigure`,
// `force_rebind`, `shutdown` and some builder setters — are exercised by this
// file's own tests but not yet reached from the public façade, which exposes a
// deliberately smaller surface than the state machine supports. They are
// covered, not unwritten.
#![allow(dead_code)]

pub(crate) mod backoff;
pub(crate) mod liveness;
pub(crate) mod options;

use std::collections::{BTreeMap, HashMap};
use std::num::NonZeroU32;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use tokio::sync::{mpsc, watch};
use tokio::time::Instant;

use crate::config::ClientConfig;
pub(crate) use crate::protocol::ProtocolError;
use crate::protocol::request::{
    BindSession, CreateSession, DestroyCause, DestroySession, ForceRebind, Heartbeat,
    MaxFrequencyLimit, MessageSend, ReconfigureSubscription, RequestId, Subscribe, TlcpRequest,
    TransportKind, Unsubscribe,
};
use crate::protocol::response::{Notification, parse_line};
use crate::session::backoff::Backoff;
use crate::session::liveness::{Liveness, LivenessAction};
use crate::session::options::SessionOptions;
use crate::subscription::manager::SubscriptionManager;
use crate::transport::ws::WsTransport;
use crate::transport::{AnyTransport, TransportError};

// ---------------------------------------------------------------------------
// The vocabulary this layer publishes upward
// ---------------------------------------------------------------------------

// The façade above talks to this module and to nothing below it: the documented
// dependency graph is `client -> session -> {protocol, transport port}`, with no
// edge from `client` to `protocol` or `transport`. Some of what the façade needs
// is nonetheless defined in those layers — a subscription mode is a wire value,
// a decoded update is subscription state — so this block is the curated list of
// what a caller of this module may name.
//
// It is a narrowing, not a formality. Everything not re-exported here — the
// parser, the request encoders, `Notification`, `ProtocolError`, the transports
// — is unreachable from `client`, and `tests/architecture.rs` fails if that
// stops being true. Adding a name here is a deliberate widening of the seam.
pub(crate) use crate::protocol::request::{
    DecimalNumber, RequestedBufferSize, RequestedMaxFrequency, SequenceName, Snapshot,
    SubscriptionMode,
};
pub(crate) use crate::protocol::response::{
    Bandwidth, FilteringMode, MaxFrequency, MessageSequence,
};
pub(crate) use crate::subscription::manager::{
    CommandFields, SubscriptionError, SubscriptionEvent,
};
// `connect` is generic over the port, so the port is part of this layer's own
// signature: a test that drives a whole client over a scripted transport must
// be able to name it. What stays unreachable from above is every *adapter*.
pub(crate) use crate::transport::{EncodedRequest, StreamOpen, Transport, TransportProperties};

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// A failure of the session layer itself, as opposed to a failure the server
/// reported (which is carried in [`ServerCause`], with its code preserved).
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub(crate) enum SessionError {
    /// The options and the transport disagree about something that must match.
    #[error("invalid session configuration: {reason}")]
    Configuration {
        /// What is inconsistent, in terms the caller can act on. Never
        /// contains a credential.
        reason: String,
    },

    /// A request could not be encoded.
    #[error(transparent)]
    Protocol(#[from] ProtocolError),

    /// The driver has stopped, so no further command can be delivered.
    #[error("the session driver has stopped")]
    Stopped,

    /// A monotonic identifier space ran out. Identifiers are never reused
    /// while a response to the old one could still arrive, so exhaustion is
    /// reported rather than wrapped around
    /// [`docs/spec/02-session-lifecycle.md` §4.4].
    #[error("the {what} identifier space is exhausted")]
    Exhausted {
        /// Which space ran out.
        what: &'static str,
    },
}

// ---------------------------------------------------------------------------
// Identity types
// ---------------------------------------------------------------------------

/// A server-assigned session identifier
/// [`docs/spec/02-session-lifecycle.md` §3.1].
///
/// It is opaque: the client echoes it and compares it, never parses it. A
/// rebind or a recovery returns the *same* identifier, which is exactly how
/// this module tells a resumed session from a replaced one — the Hands On
/// transcripts show the identical id across a rebind and a recovery, and a
/// different one after a destroy [`docs/spec/02-session-lifecycle.md` §10, F7,
/// F8, F9].
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct SessionId(String);

impl SessionId {
    /// Wraps an identifier as the server sent it.
    #[must_use]
    #[inline]
    pub(crate) fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// The identifier as it goes back on the wire.
    #[must_use]
    #[inline]
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for SessionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// A caller-facing handle on one subscription, stable for as long as the
/// caller keeps it.
///
/// It is deliberately **not** the protocol's `LS_subId`. That number "must be a
/// progressive integer number starting with 1, and must be unique within the
/// session" [`docs/spec/02-session-lifecycle.md` §4.4], so it necessarily
/// restarts at 1 when a lost session is replaced — the spec does not even say
/// whether a client should try to preserve its old numbering
/// [ibid. §9.5, ambiguity A17]. Giving the caller a key that outlives the
/// session, and renumbering the wire ids underneath it, makes that a
/// non-question: no subscription changes identity because the session did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct SubscriptionKey(u64);

impl SubscriptionKey {
    /// The key's opaque numeric value, for logging and for a façade that needs
    /// a map key.
    #[must_use]
    #[inline]
    pub(crate) const fn get(self) -> u64 {
        self.0
    }

    /// A key with a chosen value, for [`crate::test_util`] to hand a consumer
    /// a `SubscriptionId` its tests can name. No session ever allocates one
    /// this way.
    #[cfg(feature = "test-util")]
    #[must_use]
    pub(crate) const fn from_raw(id: u64) -> Self {
        Self(id)
    }
}

/// What the caller asked to subscribe to.
///
/// This is the *desired* subscription, held for the lifetime of the session
/// driver. The wire request it turns into ([`Subscribe`]) additionally needs a
/// session id, a request id and an `LS_subId`, all of which belong to one
/// particular session and are re-derived every time the subscription is issued
/// [`docs/spec/03-requests.md` §6.1].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SubscriptionSpec {
    /// `LS_group` — the item-group name, interpreted by the Metadata Adapter.
    pub(crate) group: String,
    /// `LS_schema` — the field-schema name.
    pub(crate) schema: String,
    /// `LS_mode` — the subscription mode of every item in the group.
    pub(crate) mode: SubscriptionMode,
    /// `LS_data_adapter` — which Data Adapter provides the items.
    pub(crate) data_adapter: Option<String>,
    /// `LS_selector` — a Metadata-Adapter-interpreted selector.
    pub(crate) selector: Option<String>,
    /// `LS_requested_buffer_size` — per-item buffer dimension.
    pub(crate) requested_buffer_size: Option<RequestedBufferSize>,
    /// `LS_requested_max_frequency` — per-item update frequency ceiling.
    pub(crate) requested_max_frequency: Option<RequestedMaxFrequency>,
    /// `LS_snapshot` — whether, and how much, snapshot to request.
    ///
    /// This is the field that decides whether a re-established subscription
    /// restarts from a snapshot, which the caller is told about in
    /// [`ResubscribedEntry::snapshot_requested`]
    /// (`docs/adr/0005-recovery-is-visible-in-the-event-stream.md`).
    pub(crate) snapshot: Option<Snapshot>,

    /// The caller's own names for the items, in order, or empty when the
    /// subscription named a server-side group.
    ///
    /// Not a request parameter: TLCP transmits item counts and never names
    /// [`docs/spec/04-notifications.md` §3.1], so these exist only to label the
    /// updates this session hands upward. They travel with the spec because the
    /// [`SubscriptionManager`] that needs them is rebuilt from it whenever a
    /// replacement session re-establishes the subscription.
    pub(crate) declared_items: Vec<String>,
    /// The caller's own names for the fields — see
    /// [`declared_items`](Self::declared_items).
    pub(crate) declared_fields: Vec<String>,
}

impl SubscriptionSpec {
    /// A minimal subscription: a group, a schema and a mode.
    #[must_use]
    pub(crate) fn new(
        group: impl Into<String>,
        schema: impl Into<String>,
        mode: SubscriptionMode,
    ) -> Self {
        Self {
            group: group.into(),
            schema: schema.into(),
            mode,
            data_adapter: None,
            selector: None,
            requested_buffer_size: None,
            requested_max_frequency: None,
            snapshot: None,
            declared_items: Vec::new(),
            declared_fields: Vec::new(),
        }
    }

    /// Whether the caller asked for a snapshot.
    #[must_use]
    #[inline]
    fn wants_snapshot(&self) -> bool {
        matches!(self.snapshot, Some(Snapshot::On | Snapshot::Length(_)))
    }

    /// A fresh [`SubscriptionManager`] for this subscription.
    ///
    /// Built from the spec rather than handed down from the caller, so that a
    /// subscription re-established on a replacement session gets a state
    /// machine with no memory of the old one — the server restarts its flow, so
    /// the baseline must restart with it
    /// [`docs/spec/02-session-lifecycle.md` §9.5].
    #[must_use]
    fn new_manager(&self) -> SubscriptionManager {
        SubscriptionManager::new(
            self.mode,
            self.wants_snapshot(),
            self.declared_items.clone(),
            self.declared_fields.clone(),
        )
    }
}

/// A message the caller wants delivered to the Metadata Adapter
/// [`docs/spec/03-requests.md` §12].
///
/// As with [`SubscriptionSpec`], this is what the caller asked for; the
/// session id and the `LS_reqId` belong to one particular connection and are
/// filled in when the request is issued.
///
/// # The progressive is the caller's, not this client's
///
/// `LS_msg_prog` is "the progressive number of this message, starting at 1",
/// it orders messages within a sequence, and it is what identifies the message
/// in `MSGDONE`/`MSGFAIL` [`docs/spec/03-requests.md` §12.1]. This layer does
/// **not** allocate it. The server tracks the numbering per session — its
/// duplicate and skipped-progressive codes (`32`, `33`, `38`, `39`
/// [`docs/spec/05-error-codes.md` §2]) are session-scoped — so a client-side
/// counter would silently restart whenever a session was replaced, at exactly
/// the moment the caller most needs to know what happened to a message. The
/// caller owns the sequence numbering, and this layer never renumbers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OutgoingMessage {
    /// `LS_message` — the payload. Its meaning is entirely the Metadata
    /// Adapter's [`docs/spec/03-requests.md` §12.1].
    pub(crate) message: String,
    /// `LS_sequence` — messages sharing a sequence are processed in
    /// `LS_msg_prog` order. `None` lets the server process the message at once,
    /// possibly concurrently with others.
    pub(crate) sequence: Option<SequenceName>,
    /// `LS_msg_prog` — this message's position in its sequence, and its
    /// identity in `MSGDONE`/`MSGFAIL`. Mandatory whenever a sequence is given
    /// or an outcome is requested; [`MessageSend`] rejects the combination that
    /// omits it, so a missing one surfaces as
    /// [`MessageResult::NotSent`] rather than as a server error.
    pub(crate) msg_prog: Option<NonZeroU32>,
    /// `LS_max_wait` — how long the server may wait for earlier messages of the
    /// same sequence before processing this one. Only meaningful alongside a
    /// sequence.
    pub(crate) max_wait_millis: Option<u64>,
    /// `LS_outcome` — `Some(false)` suppresses the `MSGDONE`/`MSGFAIL`
    /// notification, making the send fire-and-forget. `None` leaves the
    /// server's default of `true` [`docs/spec/03-requests.md` §12.1].
    pub(crate) outcome: Option<bool>,
}

impl OutgoingMessage {
    /// A fire-and-forget message with no sequence and no progressive.
    ///
    /// `LS_outcome` is set to `false` because `LS_msg_prog` "is mandatory
    /// whenever `LS_sequence` is specified or `LS_outcome` is set to `true`",
    /// and `LS_outcome` defaults to `true`
    /// [`docs/spec/03-requests.md` §12.1] — so a message with no progressive
    /// can only be sent by explicitly declining the outcome.
    #[must_use]
    pub(crate) fn fire_and_forget(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            sequence: None,
            msg_prog: None,
            max_wait_millis: None,
            outcome: Some(false),
        }
    }

    /// A message identified by a progressive, whose outcome will be reported.
    #[must_use]
    pub(crate) fn numbered(message: impl Into<String>, msg_prog: NonZeroU32) -> Self {
        Self {
            message: message.into(),
            sequence: None,
            msg_prog: Some(msg_prog),
            max_wait_millis: None,
            outcome: None,
        }
    }

    /// Places this message in an ordered sequence.
    #[must_use = "builders do nothing unless the result is used"]
    pub(crate) fn in_sequence(mut self, sequence: SequenceName) -> Self {
        self.sequence = Some(sequence);
        self
    }

    /// How this message identifies itself in `MSGDONE`/`MSGFAIL`.
    #[must_use]
    fn identity(&self) -> (MessageSequence, Option<u64>) {
        let sequence = match &self.sequence {
            Some(name) => MessageSequence::Named(name.as_str().to_owned()),
            // The server echoes the literal `*` when the send named no
            // sequence [`docs/spec/04-notifications.md` §4.1].
            None => MessageSequence::Unspecified,
        };
        (sequence, self.msg_prog.map(|prog| u64::from(prog.get())))
    }
}

// ---------------------------------------------------------------------------
// Outcome types
// ---------------------------------------------------------------------------

/// A cause code and message exactly as the server supplied them.
///
/// The code is preserved as a number and never folded into the message, so a
/// caller can branch on it [`docs/spec/05-error-codes.md` §1]. Codes `<= 0`
/// are Metadata-Adapter-defined and application-specific [ibid. §2].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ServerCause {
    /// The `<error-code>` or `<cause-code>` from Appendix A.
    pub(crate) code: i64,
    /// The accompanying human-readable text, which may be empty.
    pub(crate) message: String,
}

/// How a session came to be bound, and therefore what the caller may keep.
///
/// This is the ADR-0005 distinction at its sharpest: [`BindKind::Rebound`] and
/// [`BindKind::Recovering`] leave derived state valid, [`BindKind::Recreated`]
/// invalidates it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BindKind {
    /// T1 — the first session of this driver was created
    /// [`docs/spec/02-session-lifecycle.md` §2.2, T1].
    Created,

    /// T1 again, but after an earlier session was lost: a **new** session with
    /// a new identifier, its progressive restarted at zero and its
    /// subscriptions re-executed from scratch
    /// [`docs/spec/02-session-lifecycle.md` §6.1, §9.5]. Derived state built
    /// on the previous session must be discarded.
    Recreated {
        /// The identifier of the session this one replaces, when there was one.
        previous: Option<SessionId>,
    },

    /// T6 after a clean `LOOP` — the same session rebound with no recovery
    /// request. The server "restarts sending real-time notifications from where
    /// it stopped. In particular, all subscriptions are preserved, with all
    /// their items and fields" [`docs/spec/02-session-lifecycle.md` §4.4].
    Rebound,

    /// T6 with `LS_recovery_from` — the same session, resuming from a stated
    /// progressive. Whether the resumption was exact is reported separately by
    /// [`SessionEvent::Recovered`], because `PROG` arrives after `CONOK`
    /// [`docs/spec/02-session-lifecycle.md` §5.4].
    Recovering {
        /// The progressive the client asked to resume from.
        requested_progressive: u64,
    },
}

/// Where a recovered flow actually resumed, relative to where it was asked to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RecoveryOutcome {
    /// What `LS_recovery_from` carried.
    pub(crate) requested: u64,
    /// What `PROG` answered.
    pub(crate) resumed_at: u64,
    /// The relationship between the two.
    pub(crate) kind: RecoveryKind,
}

/// The three ways a `PROG` can relate to the requested progressive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RecoveryKind {
    /// The server resumed exactly where the client asked. Nothing was lost and
    /// nothing is duplicated.
    Exact,

    /// The server resumed **earlier** than asked, which it is explicitly
    /// allowed to do. The duplicated notifications are discarded by this client
    /// before the caller sees them, as the spec's fourth recovery precondition
    /// requires: "the first data notifications received are discarded, until
    /// the desired point is reached"
    /// [`docs/spec/02-session-lifecycle.md` §5.2].
    Duplicated {
        /// How many notifications this client suppressed.
        count: u64,
    },

    /// The server resumed **later** than asked: the notifications in between
    /// are gone.
    ///
    /// SPEC-AMBIGUITY (A7): `PROG` "can be lower" than requested, but the spec
    /// "never states whether it can be *higher* than requested, nor what a
    /// client must do if it is"
    /// [`docs/spec/02-session-lifecycle.md` §5.4]. Reporting the gap is the
    /// defensive choice — silently continuing would tell an application that
    /// continuity was preserved when it was not, which
    /// `docs/adr/0005-recovery-is-visible-in-the-event-stream.md` calls out as
    /// worse than saying nothing.
    Gap {
        /// How many notifications were skipped by the server.
        missing: u64,
    },
}

/// Why the session stopped being bound to a stream connection.
///
/// One variant per edge leaving *Bound* in the specification's own state
/// diagram [`docs/spec/02-session-lifecycle.md` §2.2, §2.3], plus the two
/// cases the diagram has no edge for: a stalled-but-not-broken connection
/// (§8.1) and a rejected bind (§6.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum UnbindReason {
    /// T2 — `Force-Unbound by Client`: the client sent `LS_op=force_rebind`
    /// and the server answered with `LOOP`
    /// [`docs/spec/02-session-lifecycle.md` §2.2, T2].
    ForcedByClient {
        /// The delay `LOOP` asked for before rebinding.
        expected_delay: Duration,
    },

    /// T3 — `Content-Length Reached`: the HTTP stream connection exhausted its
    /// byte budget [`docs/spec/02-session-lifecycle.md` §2.2, T3].
    ContentLengthReached {
        /// The delay `LOOP` asked for before rebinding.
        expected_delay: Duration,
    },

    /// T4 — `Poll Cycle Expired`: the polling cycle completed
    /// [`docs/spec/02-session-lifecycle.md` §2.2, T4].
    PollCycleExpired {
        /// The delay `LOOP` asked for before rebinding. On a synchronous
        /// polling session this may be non-zero
        /// [`docs/spec/02-session-lifecycle.md` §4.2].
        expected_delay: Duration,
    },

    /// A `LOOP` arrived on a transport whose declared properties identify none
    /// of T2, T3 or T4. Rebinding is the same either way; only the label
    /// differs.
    Looped {
        /// The delay `LOOP` asked for before rebinding.
        expected_delay: Duration,
    },

    /// T5 — `Connection Failed`: "a stream connection is dropped by any cause"
    /// and no notification is received. The session is *not* closed and a
    /// rebind — specifically, a recovery — is still allowed
    /// [`docs/spec/02-session-lifecycle.md` §2.2, T5].
    ConnectionFailed {
        /// What the transport reported. Never contains a credential.
        detail: String,
    },

    /// The stream went quiet past its keepalive budget without ever failing.
    ///
    /// Not an edge in the p.8 diagram, but the spec's own prescription: "if
    /// after the expected interval plus a configurable timeout no `PROBE` has
    /// been received, the connection is closed and reopened"
    /// [`docs/spec/02-session-lifecycle.md` §8.1], and "if a connection becomes
    /// mute, the client can issue a recovery request" [ibid. §5.1].
    KeepaliveExpired {
        /// The budget that elapsed with no traffic at all.
        budget: Duration,
    },

    /// A `create_session` or `bind_session` was answered with a `CONERR` whose
    /// code permits another attempt [`docs/spec/02-session-lifecycle.md` §6.2].
    Rejected {
        /// The server's cause, code preserved.
        cause: ServerCause,
    },

    /// The server ended the session with a code that tells the client to open a
    /// new one at once — code `48`, "the client should recover by opening a new
    /// session immediately" [`docs/spec/02-session-lifecycle.md` §6.2].
    ServerRefresh {
        /// The server's cause, code preserved.
        cause: ServerCause,
    },
}

impl UnbindReason {
    /// Whether this unbind followed a clean `LOOP`, after which a plain rebind
    /// is safe: "the Server will assume that all updates sent in the previous
    /// connection (up to the `LOOP` notification) have been received"
    /// [`docs/spec/02-session-lifecycle.md` §4.4].
    #[must_use]
    const fn is_clean_loop(&self) -> bool {
        matches!(
            self,
            Self::ForcedByClient { .. }
                | Self::ContentLengthReached { .. }
                | Self::PollCycleExpired { .. }
                | Self::Looped { .. }
        )
    }

    /// The delay the server asked for before rebinding, if it named one.
    #[must_use]
    const fn expected_delay(&self) -> Option<Duration> {
        match self {
            Self::ForcedByClient { expected_delay }
            | Self::ContentLengthReached { expected_delay }
            | Self::PollCycleExpired { expected_delay }
            | Self::Looped { expected_delay } => Some(*expected_delay),
            _ => None,
        }
    }
}

/// Why the session ended, terminally. The driver has stopped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SessionClosed {
    /// T7 or T8 — the client ended it
    /// [`docs/spec/02-session-lifecycle.md` §2.2, T7/T8].
    ByClient {
        /// Whether the server confirmed the destroy with an `END`.
        ///
        /// `false` when the caller merely shut the driver down, or when the
        /// destroy could not be confirmed. Dropping the connection without a
        /// destroy leaves the session buffering server-side until its timeout
        /// [`docs/spec/02-session-lifecycle.md` §6.3], which is the reason the
        /// destroy path exists at all.
        destroy_confirmed: bool,
        /// The cause the server reported in `END`, when it sent one. The
        /// default for a client destroy is code `31`
        /// [`docs/spec/02-session-lifecycle.md` §10, F5].
        cause: Option<ServerCause>,
    },

    /// The server closed the session, or refused to bind it, with a code that
    /// admits no retry [`docs/spec/02-session-lifecycle.md` §6.2].
    ByServer {
        /// The server's cause, code preserved.
        cause: ServerCause,
    },

    /// The reconnect budget was spent without ever binding again. This is T9 in
    /// practice: the session is treated as definitively lost
    /// [`docs/spec/02-session-lifecycle.md` §2.2, T9].
    RetriesExhausted {
        /// How many consecutive attempts failed.
        attempts: u32,
        /// The last thing that went wrong, when there was a last thing.
        last: Option<UnbindReason>,
    },

    /// The driver could not even build the request it needed to send. A bug or
    /// an impossible configuration, never a server condition.
    Internal {
        /// What failed. Never contains a credential.
        reason: String,
    },
}

// ---------------------------------------------------------------------------
// Events
// ---------------------------------------------------------------------------

/// What the caller learns about one bind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BoundInfo {
    /// The session that is now bound.
    pub(crate) session_id: SessionId,
    /// How it came to be bound, and therefore what derived state survives.
    pub(crate) kind: BindKind,
    /// `CONOK` argument 3: the longest inactivity the server guarantees
    /// [`docs/spec/02-session-lifecycle.md` §8.1]. This, not any configured
    /// value, is what the client's liveness budget is built from.
    pub(crate) keep_alive: Duration,
    /// `CONOK` argument 2: the maximum request length the server accepts, in
    /// bytes [`docs/spec/02-session-lifecycle.md` §3.1].
    pub(crate) request_limit_bytes: u64,
    /// `CONOK` argument 4: the control link, or `None` when the server sent the
    /// literal `*` [`docs/spec/02-session-lifecycle.md` §3.1].
    pub(crate) control_link: Option<String>,
}

/// One subscription re-issued on a newly bound session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResubscribedEntry {
    /// The caller's stable key, unchanged across the re-establishment.
    pub(crate) key: SubscriptionKey,
    /// The `LS_subId` allocated for it in the new session.
    pub(crate) subscription_id: NonZeroU32,
    /// Whether this subscription had already been confirmed by the server
    /// before the session was replaced. `false` means it was still pending.
    pub(crate) previously_active: bool,
    /// Whether the subscription asks for a snapshot, and will therefore restart
    /// from one rather than resume
    /// (`docs/adr/0005-recovery-is-visible-in-the-event-stream.md`).
    pub(crate) snapshot_requested: bool,
}

/// Which subscription operation a control request was performing.
///
/// The distinction is not cosmetic: a refused `add` means the subscription
/// does not exist, while a refused `delete` means it very much still does.
/// A caller told only "rejected" would get one of the two backwards.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SubscriptionOperation {
    /// `LS_op=add` — the subscription was never established. It has been
    /// dropped from the desired set and will **not** be re-issued on a later
    /// session.
    Subscribe,
    /// `LS_op=delete` — the subscription is **still active**; the caller's
    /// request to remove it did not take effect.
    Unsubscribe,
    /// `LS_op=reconf` — the subscription is unchanged and still active; only
    /// the frequency change failed.
    Reconfigure,
}

/// What a control request was acting on.
///
/// Control responses correlate by `LS_reqId`, which is this layer's private
/// sequence — the caller never sees one and so could never map a response back
/// to the thing it asked for. Naming the target closes that gap: a rejection
/// that cannot be routed to the subscription it killed is a silently lost
/// subscription, whatever the event stream says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ControlTarget {
    /// The request concerned the session as a whole — `force_rebind`,
    /// `destroy`, `msg` — or the response could not be correlated to any
    /// request at all.
    Session,
    /// The request concerned one subscription.
    Subscription {
        /// Which one, in the caller's own stable terms.
        key: SubscriptionKey,
        /// What was being attempted, and therefore what the failure means for
        /// the subscription's state.
        operation: SubscriptionOperation,
    },
}

/// How a control request ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ControlOutcome {
    /// `REQOK` — the server accepted the request
    /// [`docs/spec/03-requests.md` §13.1].
    Accepted,
    /// `REQERR` or `ERROR` — the server refused it, with an Appendix B code
    /// [`docs/spec/05-error-codes.md` §1].
    Rejected {
        /// The server's cause, code preserved.
        cause: ServerCause,
    },
    /// The request never reached the server: the transport refused it or the
    /// encoder rejected the parameters. No server code exists for this.
    NotSent {
        /// What went wrong. Never contains a credential.
        reason: String,
    },
}

/// What became of one message sent with `msg`
/// [`docs/spec/03-requests.md` §12].
///
/// A message can fail in three places, and the caller needs to tell them
/// apart: it may never leave this client, the server may refuse to accept it,
/// or the Metadata Adapter may fail while processing it. Only the third is a
/// `MSGFAIL`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MessageResult {
    /// `MSGDONE` — the Metadata Adapter's `notifyUserMessage` returned without
    /// raising [`docs/spec/04-notifications.md` §4.1].
    Done {
        /// The adapter's response text. Empty when it supplied none.
        response: String,
    },

    /// `MSGFAIL` — the message was not delivered, or its processing failed
    /// [`docs/spec/04-notifications.md` §4.2].
    Failed {
        /// The server's cause, code preserved. Appendix B codes `32`, `33`,
        /// `38` and `39` concern the sequence numbering specifically
        /// [`docs/spec/05-error-codes.md` §2].
        cause: ServerCause,
    },

    /// `REQERR` or `ERROR` — the server refused the request outright, so no
    /// `MSGDONE`/`MSGFAIL` will ever follow
    /// [`docs/spec/03-requests.md` §13.2].
    Refused {
        /// The server's cause, code preserved.
        cause: ServerCause,
    },

    /// The message never reached the server: nothing was bound to send it on,
    /// the transport refused it, or its parameters could not be encoded.
    ///
    /// **Messages are not buffered.** A subscription is a statement of desired
    /// state and is safely re-issued on whatever session comes next; a message
    /// is a one-shot side effect whose ordering and de-duplication the server
    /// tracks *per session* [`docs/spec/03-requests.md` §12.1;
    /// `docs/spec/05-error-codes.md` §2, codes 32/33/38/39]. Re-issuing one
    /// across a session replacement would restart the numbering the server
    /// deduplicates against, so this client fails the send instead of
    /// promising an ordering it cannot keep. Retrying is the caller's
    /// decision, which is the only place the decision can be made correctly.
    NotSent {
        /// What stopped it. Never contains a credential.
        reason: String,
    },
}

/// What one subscription notification meant, or the typed reason it could not
/// be reconciled with what the server already said.
///
/// A line this client cannot decode is reported, never fatal: the subscription
/// stays alive and the caller is told which line was lost
/// (`docs/adr/0003-typed-event-stream-as-delivery-surface.md`).
pub(crate) type SubscriptionOutcome = Result<SubscriptionEvent, SubscriptionError>;

/// Something the server said about itself, rather than about the data
/// [`docs/spec/02-session-lifecycle.md` §5.3].
///
/// A typed alternative to passing the `SERVNAME`, `CLIENTIP`, `SYNC` and `CONS`
/// notifications upward verbatim: the parser's own types stop at this layer, so
/// the façade above cannot come to depend on them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ServerAnnouncement {
    /// `SERVNAME` — the server's configured name
    /// [`docs/spec/04-notifications.md` §5.4].
    Name(String),
    /// `CLIENTIP` — the address the server sees this client at
    /// [`docs/spec/04-notifications.md` §5.5].
    ClientIp(String),
    /// `SYNC` — how long the server believes the session has been running
    /// [`docs/spec/04-notifications.md` §5.3].
    Clock {
        /// Seconds since the session began, as the server counts them.
        elapsed_seconds: u64,
    },
    /// `CONS` — the bandwidth ceiling now in force
    /// [`docs/spec/04-notifications.md` §5.2].
    Bandwidth(Bandwidth),
}

/// Everything the session layer tells the layer above it.
///
/// The variants that carry the ADR-0005 distinctions are [`SessionEvent::Bound`]
/// (through [`BindKind`]), [`SessionEvent::Recovered`] and
/// [`SessionEvent::Closed`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SessionEvent {
    /// The session is bound to a stream connection and streaming
    /// [`docs/spec/02-session-lifecycle.md` §2.1].
    Bound(Box<BoundInfo>),

    /// A recovery bind reported where it resumed
    /// [`docs/spec/02-session-lifecycle.md` §5.4].
    Recovered(RecoveryOutcome),

    /// Subscriptions were re-issued on a newly bound session. Emitted whenever
    /// any `add` is sent at bind time, which after a session replacement is
    /// every one of them [`docs/spec/02-session-lifecycle.md` §6.1].
    Resubscribed(Vec<ResubscribedEntry>),

    /// The session left the bound state. A reconnect is already scheduled
    /// unless `retry_in` is `None`, which means the driver is about to stop.
    Unbound {
        /// Which edge of the state diagram was taken.
        reason: UnbindReason,
        /// How long the driver will wait before its next attempt.
        retry_in: Option<Duration>,
    },

    /// What became of a message the caller sent
    /// [`docs/spec/03-requests.md` §12].
    ///
    /// `MSGDONE` and `MSGFAIL` **are** data notifications and are counted
    /// toward the recovery progressive like any other
    /// [`docs/spec/02-session-lifecycle.md` §5.3] — they simply reach the
    /// caller through this typed variant instead of through
    /// [`SessionEvent::Data`], because it can answer "what happened to my
    /// message" in one match arm rather than three.
    Message {
        /// Its position in the session's data-notification count, or `None`
        /// when the message never left this client and so the server never
        /// counted anything.
        progressive: Option<u64>,
        /// The sequence the message was sent in, or
        /// [`MessageSequence::Unspecified`].
        sequence: MessageSequence,
        /// The `LS_msg_prog` that identifies the message, or `None` when it
        /// carried none — only possible with `LS_outcome=false`, in which case
        /// the sole result that can arrive is [`MessageResult::NotSent`] or
        /// [`MessageResult::Refused`].
        prog: Option<u64>,
        /// What became of it.
        result: MessageResult,
    },

    /// Something happened to one subscription
    /// [`docs/spec/04-notifications.md` §2, §3].
    ///
    /// The notification that caused it has already been applied to that
    /// subscription's item state here, so what travels upward is the meaning,
    /// not the line. Duplicates suppressed after a recovery never appear, and
    /// `MSGDONE`/`MSGFAIL` arrive as [`SessionEvent::Message`] instead.
    Subscription {
        /// This notification's 1-based position in the session's data-
        /// notification count — the number `LS_recovery_from` would carry if
        /// the connection broke right after it.
        progressive: u64,
        /// Which subscription it belongs to.
        key: SubscriptionKey,
        /// What it meant, or why it could not be reconciled with what the
        /// server already said. Boxed only to keep [`SessionEvent`] small.
        outcome: Box<SubscriptionOutcome>,
    },

    /// A control request was answered [`docs/spec/03-requests.md` §13].
    ///
    /// `target` is what makes this routable. A rejection carrying only a
    /// request id would name a number the caller has never seen, leaving it
    /// with a dead subscription and no way to know which one — the silent loss
    /// this crate treats as a defect.
    ControlResponse {
        /// The request being answered, or `None` for an `ERROR`, which carries
        /// no request id because the server could not parse one
        /// [`docs/spec/03-requests.md` §13.2].
        request_id: Option<RequestId>,
        /// What the request was acting on.
        target: ControlTarget,
        /// What the server said.
        outcome: ControlOutcome,
    },

    /// The server said something about itself that does not change the state
    /// machine: `CONS`, `SYNC`, `SERVNAME`, `CLIENTIP`
    /// [`docs/spec/02-session-lifecycle.md` §5.3].
    ServerInfo(ServerAnnouncement),

    /// A line arrived that this client could not parse.
    ///
    /// Never fatal: "a future server version must not crash an old client".
    /// The raw line is preserved so the caller can log or surface it.
    Unparsed {
        /// The line as it arrived, terminator stripped.
        line: String,
        /// Why it could not be parsed.
        error: ProtocolError,
    },

    /// Terminal. The driver has stopped and no further event will follow.
    Closed(SessionClosed),
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

/// What the layer above asks the session to do.
#[derive(Debug)]
pub(crate) enum SessionCommand {
    /// Add a subscription, now or as soon as a session is bound
    /// [`docs/spec/03-requests.md` §6].
    Subscribe {
        /// The key the caller was handed synchronously.
        key: SubscriptionKey,
        /// What to subscribe to.
        spec: Box<SubscriptionSpec>,
    },

    /// Remove a subscription [`docs/spec/03-requests.md` §7].
    Unsubscribe {
        /// Which one.
        key: SubscriptionKey,
    },

    /// Change a subscription's maximum update frequency
    /// [`docs/spec/03-requests.md` §8].
    Reconfigure {
        /// Which one.
        key: SubscriptionKey,
        /// The new ceiling.
        max_frequency: MaxFrequencyLimit,
    },

    /// Send a message to the Metadata Adapter
    /// [`docs/spec/03-requests.md` §12].
    ///
    /// Unlike a subscription this is never buffered; see
    /// [`MessageResult::NotSent`].
    SendMessage {
        /// What to send.
        message: Box<OutgoingMessage>,
    },

    /// Ask the server to unbind the session so it can be rebound — T2
    /// [`docs/spec/02-session-lifecycle.md` §2.2, T2].
    ForceRebind {
        /// `LS_close_socket`: whether the server should close the stream
        /// connection rather than leave it reusable
        /// [`docs/spec/02-session-lifecycle.md` §4.3].
        close_socket: Option<bool>,
    },

    /// Destroy the session — T7 while bound, T8 while unbound
    /// [`docs/spec/02-session-lifecycle.md` §2.2, T7/T8].
    Destroy {
        /// The cause to report in the resulting `END`, or `None` for the
        /// server's default code `31`.
        cause: Option<DestroyCause>,
    },

    /// Stop the driver without destroying the session. The server keeps the
    /// session buffering until its own timeout
    /// [`docs/spec/02-session-lifecycle.md` §6.3].
    Shutdown,
}

/// The caller's end of the session. Cheap to clone; the driver stops when the
/// last clone is dropped.
#[derive(Debug, Clone)]
pub(crate) struct SessionHandle {
    commands: mpsc::Sender<SessionCommand>,
    next_key: Arc<AtomicU64>,
    /// Raised to order an immediate stop. See the module documentation's *stop
    /// lane*: it is the one signal that is never subject to backpressure.
    stop: Arc<watch::Sender<bool>>,
}

impl SessionHandle {
    /// Allocates a subscription key without asking for anything yet.
    ///
    /// Separate from [`SessionHandle::subscribe`] so that the layer above can
    /// register everything that key names — a stream, a decoder — *before* the
    /// request that could produce a notification for it is queued. Registering
    /// afterwards leaves a window in which an immediate `SUBOK` or `REQERR`
    /// has nowhere to go [`docs/spec/03-requests.md` §13].
    ///
    /// # Errors
    ///
    /// [`SessionError::Exhausted`] if the 64-bit key space ran out. Keys are
    /// never reused, so this is reported rather than wrapped.
    pub(crate) fn allocate_key(&self) -> Result<SubscriptionKey, SessionError> {
        self.next_key
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |key| {
                key.checked_add(1)
            })
            .map(SubscriptionKey)
            .map_err(|_| SessionError::Exhausted {
                what: "subscription key",
            })
    }

    /// Requests a subscription under a key already allocated by
    /// [`SessionHandle::allocate_key`].
    ///
    /// # Errors
    ///
    /// [`SessionError::Stopped`] if the driver has already stopped.
    pub(crate) async fn subscribe_with_key(
        &self,
        key: SubscriptionKey,
        spec: SubscriptionSpec,
    ) -> Result<(), SessionError> {
        self.send(SessionCommand::Subscribe {
            key,
            spec: Box::new(spec),
        })
        .await
    }

    /// Requests a subscription and returns its key immediately.
    ///
    /// The key is allocated here rather than by the driver so that the caller
    /// can record it before the `add` request has even been encoded. The
    /// subscription is issued as soon as a session is bound, and re-issued on
    /// every session that replaces it.
    ///
    /// # Errors
    ///
    /// [`SessionError::Stopped`] if the driver has already stopped, or
    /// [`SessionError::Exhausted`] if the key space ran out.
    pub(crate) async fn subscribe(
        &self,
        spec: SubscriptionSpec,
    ) -> Result<SubscriptionKey, SessionError> {
        let key = self.allocate_key()?;
        self.subscribe_with_key(key, spec).await?;
        Ok(key)
    }

    /// Orders the driver to stop at once, whatever it is waiting for.
    ///
    /// Unlike [`SessionHandle::shutdown`] this does not queue a command, so it
    /// cannot be held up by a full command channel or by a driver blocked
    /// emitting into an event stream nobody is reading. It is how "dropping the
    /// client shuts the session down" stays true under backpressure. The
    /// server is *not* told: use [`SessionHandle::destroy`] for that.
    pub(crate) fn stop(&self) {
        let _ = self.stop.send(true);
    }

    /// A receiver for the stop lane, for the other tasks a client owns.
    pub(crate) fn stop_signal(&self) -> watch::Receiver<bool> {
        self.stop.subscribe()
    }

    /// Removes a subscription.
    ///
    /// # Errors
    ///
    /// [`SessionError::Stopped`] if the driver has already stopped.
    pub(crate) async fn unsubscribe(&self, key: SubscriptionKey) -> Result<(), SessionError> {
        self.send(SessionCommand::Unsubscribe { key }).await
    }

    /// Changes a subscription's maximum update frequency.
    ///
    /// # Errors
    ///
    /// [`SessionError::Stopped`] if the driver has already stopped.
    pub(crate) async fn reconfigure(
        &self,
        key: SubscriptionKey,
        max_frequency: MaxFrequencyLimit,
    ) -> Result<(), SessionError> {
        self.send(SessionCommand::Reconfigure { key, max_frequency })
            .await
    }

    /// Sends a message to the Metadata Adapter
    /// [`docs/spec/03-requests.md` §12].
    ///
    /// Returning `Ok` means the message was **queued for this driver**, not
    /// that it reached the server. Its fate arrives as a
    /// [`SessionEvent::Message`] carrying the sequence and progressive the
    /// message identified itself with — including when it never left this
    /// client, which is what happens if nothing is bound at the time
    /// ([`MessageResult::NotSent`]; messages are deliberately not buffered).
    ///
    /// # Errors
    ///
    /// [`SessionError::Stopped`] if the driver has already stopped.
    pub(crate) async fn send_message(&self, message: OutgoingMessage) -> Result<(), SessionError> {
        self.send(SessionCommand::SendMessage {
            message: Box::new(message),
        })
        .await
    }

    /// Asks the server to unbind the session so it is rebound at once.
    ///
    /// # Errors
    ///
    /// [`SessionError::Stopped`] if the driver has already stopped.
    pub(crate) async fn force_rebind(
        &self,
        close_socket: Option<bool>,
    ) -> Result<(), SessionError> {
        self.send(SessionCommand::ForceRebind { close_socket })
            .await
    }

    /// Destroys the session and stops the driver.
    ///
    /// # Errors
    ///
    /// [`SessionError::Stopped`] if the driver has already stopped.
    pub(crate) async fn destroy(&self, cause: Option<DestroyCause>) -> Result<(), SessionError> {
        self.send(SessionCommand::Destroy { cause }).await
    }

    /// Stops the driver without destroying the session.
    ///
    /// # Errors
    ///
    /// [`SessionError::Stopped`] if the driver has already stopped.
    pub(crate) async fn shutdown(&self) -> Result<(), SessionError> {
        self.send(SessionCommand::Shutdown).await
    }

    async fn send(&self, command: SessionCommand) -> Result<(), SessionError> {
        self.commands
            .send(command)
            .await
            .map_err(|_| SessionError::Stopped)
    }
}

// ---------------------------------------------------------------------------
// Error-code classification
// ---------------------------------------------------------------------------

/// What a session-level error code leaves the client able to do.
///
/// The specification "has **no formal recoverable/lost taxonomy**"
/// [`docs/spec/05-error-codes.md` §1] and states a client action for only a
/// handful of codes. This enum is therefore a client-side policy built on the
/// statements the spec does make; every arm of [`classify`] cites the sentence
/// it rests on, and the arms that rest on nothing say so.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Recovery {
    /// The old session is gone but a new one may be created at once, and all
    /// subscriptions re-executed
    /// [`docs/spec/02-session-lifecycle.md` §6.1].
    RecreateSession,
    /// The condition is stated to be temporary; retry after a backoff.
    RetryLater,
    /// Retrying cannot help. The session is definitively lost.
    Fatal,
}

/// Classifies an Appendix A session code
/// [`docs/spec/05-error-codes.md` §2, §5].
#[must_use]
pub(crate) fn classify(code: i64) -> Recovery {
    match code {
        // "the recovery attempt failed" — and the spec's general rule for that
        // is that "the Client can't but create a new stream connection as if it
        // was the first time it connects" [§5.1].
        4 => Recovery::RecreateSession,

        // "Specified session not found on a `bind_session` request" — the
        // "too much time passed" case, whose prescribed answer is to create a
        // new session and re-execute the subscriptions
        // [`docs/spec/02-session-lifecycle.md` §6.2, §6.1].
        20 => Recovery::RecreateSession,

        // "the client should recover by opening a new session immediately" —
        // the one code for which the spec prescribes the action outright
        // [`docs/spec/05-error-codes.md` §5.4].
        48 => Recovery::RecreateSession,

        // "retry later", stated verbatim for 5 and 6; 10 is "New sessions
        // temporarily blocked" [`docs/spec/05-error-codes.md` §5.5].
        5 | 6 | 10 => Recovery::RetryLater,

        // SPEC-AMBIGUITY: the spec records no recoverability for 7, 8 and 9
        // (licensed / configured session limits, server load on
        // `create_session`) [`docs/spec/05-error-codes.md` §2]. They describe
        // capacity, which is transient by nature, so they are retried under
        // the same backoff as 5/6/10. This is a client-side reading, not a
        // spec statement, and is reversible.
        7..=9 => Recovery::RetryLater,

        // SPEC-AMBIGUITY: 33 and 34 share one description, "An unexpected
        // error occurred on the Server while the session was in activity", and
        // the spec neither distinguishes them nor states recoverability
        // [`docs/spec/05-error-codes.md` §2]. An unexpected server-side error
        // is treated as transient and retried; a fresh session is the most a
        // client can do about it either way.
        33 | 34 => Recovery::RetryLater,

        // SPEC-AMBIGUITY: code 21 is the session id "not compatible with this
        // Server instance… it might pertain to a different instance", which
        // matches the description of the permanent cluster-affinity failure
        // that "will not be overcome by a reconnection but only by fixing the
        // configuration" — but `docs/spec/05-error-codes.md` §5.3 warns that
        // "the document never states the linkage. Do not assume it." Treating
        // 21 as fatal is the defensive reading: retrying a permanent
        // misconfiguration is a reconnect storm against a server that can
        // never answer, and the caller is told the code so it can decide
        // otherwise.
        21 => Recovery::Fatal,

        // Everything else, including authentication and adapter-set failures
        // (1, 2), protocol incompatibility (3), licence restrictions (11),
        // administrative closure (31, 32), the same-user eviction (35), the
        // foreign manual rebind (40), and the malformed-request family
        // (60, 64–71).
        //
        // SPEC-AMBIGUITY: the spec "neither reserves [the gaps in the
        // numbering] nor says what a client should do with a positive code it
        // does not recognise" [`docs/spec/05-error-codes.md` §2]. An unknown
        // code is therefore fatal rather than retried: a client that retries
        // codes it does not understand turns one server-side refusal into a
        // reconnect storm, whereas a client that stops hands the caller a code
        // and a message to act on.
        //
        // Codes `<= 0` are Metadata-Adapter-defined and "application-specific"
        // [`docs/spec/05-error-codes.md` §2]; this client cannot know whether
        // retrying one is safe, so it does not.
        _ => Recovery::Fatal,
    }
}

// ---------------------------------------------------------------------------
// Internal bookkeeping
// ---------------------------------------------------------------------------

/// Where a subscription stands with the server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SubscriptionState {
    /// Not yet issued on the current session, or issued and not yet confirmed.
    Pending,
    /// Confirmed by `SUBOK` or `SUBCMD` [`docs/spec/04-notifications.md` §3.1].
    Active,
    /// A `delete` has been sent; awaiting `UNSUB`
    /// [`docs/spec/04-notifications.md` §3.4].
    Removing,
}

/// One desired subscription and its binding, if any, in the current session.
#[derive(Debug, Clone)]
struct RegistryEntry {
    spec: SubscriptionSpec,
    /// The `LS_subId` this subscription holds in the current session, or `None`
    /// when it has not been issued on it. Cleared whenever the session is
    /// replaced, because `LS_subId` is unique *within the session*
    /// [`docs/spec/02-session-lifecycle.md` §4.4].
    wire: Option<NonZeroU32>,
    state: SubscriptionState,
    /// Whether the server ever confirmed it, in any session. Reported on
    /// re-subscription so the caller knows what it is getting back.
    was_active: bool,
    /// The item and field state of this subscription: schema, baselines, and
    /// the §2.4 snapshot classification.
    ///
    /// It lives here because the notifications that drive it are routed by this
    /// layer and by nothing else. The façade above never sees a
    /// [`Notification`]; it receives the [`SubscriptionEvent`] this produced.
    manager: SubscriptionManager,
}

/// The desired subscription set, and its mapping onto the current session's
/// `LS_subId` space.
#[derive(Debug, Default)]
struct Registry {
    entries: BTreeMap<SubscriptionKey, RegistryEntry>,
    by_sub_id: BTreeMap<u32, SubscriptionKey>,
    /// The next `LS_subId` to allocate. "A progressive integer number starting
    /// with 1… unique within the session", and, because IDs "survive rebinds
    /// and must not be reused", never decremented within a session
    /// [`docs/spec/02-session-lifecycle.md` §4.4].
    next_sub_id: u32,
}

impl Registry {
    fn new() -> Self {
        Self {
            entries: BTreeMap::new(),
            by_sub_id: BTreeMap::new(),
            next_sub_id: 1,
        }
    }

    /// Drops every wire binding, for use when a **new** session replaces the
    /// old one. Subscription ids restart at 1 in the new session
    /// [`docs/spec/02-session-lifecycle.md` §9.5].
    ///
    /// Entries that were being removed are dropped outright: the new session
    /// never had them, so there is nothing left to delete.
    fn reset_for_new_session(&mut self) {
        self.entries
            .retain(|_, entry| entry.state != SubscriptionState::Removing);
        for entry in self.entries.values_mut() {
            entry.wire = None;
            entry.state = SubscriptionState::Pending;
            // A new session is a new flow: the server restarts the subscription
            // from scratch, so every field baseline, every schema and every
            // snapshot classification held for it is stale
            // [`docs/spec/02-session-lifecycle.md` §9.5]. Keeping one would let
            // an unchanged token or a diff from the new session be applied to a
            // value from the old.
            entry.manager = entry.spec.new_manager();
        }
        self.by_sub_id.clear();
        self.next_sub_id = 1;
    }

    /// Allocates the next `LS_subId`, or `None` if the session has somehow
    /// exhausted the 32-bit space.
    fn allocate(&mut self) -> Option<NonZeroU32> {
        let id = NonZeroU32::new(self.next_sub_id)?;
        self.next_sub_id = self.next_sub_id.checked_add(1)?;
        Some(id)
    }
}

/// What a control request was for, so its response can be acted on.
#[derive(Debug, Clone, PartialEq, Eq)]
enum PendingKind {
    Subscribe(SubscriptionKey),
    Unsubscribe(SubscriptionKey),
    /// A `reconf`, carrying the value it asked for so that an accepted change
    /// can be committed to the desired state and survive a session that has to
    /// be recreated [`docs/spec/03-requests.md` §8].
    Reconfigure {
        key: SubscriptionKey,
        max_frequency: MaxFrequencyLimit,
    },
    /// A `msg`. It carries the message's own identity because a `REQERR`
    /// correlates by `LS_reqId` while the caller thinks in terms of the
    /// sequence and progressive that `MSGDONE`/`MSGFAIL` would have used
    /// [`docs/spec/04-notifications.md` §4.1] — keeping both here is what lets
    /// a refusal be reported on the same surface as a processing failure.
    SendMessage {
        sequence: MessageSequence,
        prog: Option<u64>,
    },
    ForceRebind,
    Destroy,
}

impl PendingKind {
    /// What this request acts on, for the response event that will report it.
    #[must_use]
    fn target(&self) -> ControlTarget {
        match self {
            Self::Subscribe(key) => ControlTarget::Subscription {
                key: *key,
                operation: SubscriptionOperation::Subscribe,
            },
            Self::Unsubscribe(key) => ControlTarget::Subscription {
                key: *key,
                operation: SubscriptionOperation::Unsubscribe,
            },
            Self::Reconfigure { key, .. } => ControlTarget::Subscription {
                key: *key,
                operation: SubscriptionOperation::Reconfigure,
            },
            Self::SendMessage { .. } | Self::ForceRebind | Self::Destroy => ControlTarget::Session,
        }
    }
}

/// One control request awaiting its `REQOK` or `REQERR`, tagged with the
/// session it belongs to.
///
/// The tag exists because §4.5 promises the awkward case outright: "after
/// binding the second session, it is possible that late responses to control
/// requests related with the first session arrive interspersed with
/// notifications for the second session"
/// [`docs/spec/02-session-lifecycle.md` §4.5]. Without a generation, a late
/// `REQERR` naming a subscription that has since been re-established on a new
/// session would drop it from the desired set — a subscription lost to a
/// message about a session that no longer exists.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Pending {
    kind: PendingKind,
    /// Which session generation issued it. See
    /// [`SessionDriver::generation`].
    generation: u64,
}

/// The desired-state form of a value accepted by a `reconf`.
///
/// `reconf` cannot ask for `unfiltered`, so every value it can carry has a
/// counterpart among the subscription request's own
/// [`docs/spec/03-requests.md` §6.1, §8.1].
#[must_use]
fn as_requested_frequency(limit: MaxFrequencyLimit) -> RequestedMaxFrequency {
    match limit {
        MaxFrequencyLimit::Unlimited => RequestedMaxFrequency::Unlimited,
        MaxFrequencyLimit::Limited(number) => RequestedMaxFrequency::Limited(number),
    }
}

/// Which kind of stream-opening request is in flight, and what it means for
/// the `CONOK` that answers it.
#[derive(Debug, Clone, PartialEq, Eq)]
enum OpenIntent {
    /// `create_session` [`docs/spec/02-session-lifecycle.md` §3.1].
    Create,
    /// `bind_session` with no `LS_recovery_from`
    /// [`docs/spec/02-session-lifecycle.md` §4.4].
    Rebind,
    /// `bind_session` with `LS_recovery_from`
    /// [`docs/spec/02-session-lifecycle.md` §5.4].
    Recover {
        /// The progressive the request carried.
        requested: u64,
    },
}

/// The two moments of the specification's *Bound* state that a client must
/// tell apart: before `CONOK`, when an unanswered handshake must time out, and
/// after it, when the negotiated keep-alive governs
/// [`docs/spec/02-session-lifecycle.md` §2.1, §8.1].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StreamState {
    /// A stream-opening request is in flight; no `CONOK` yet.
    Establishing,
    /// `CONOK` arrived: the session is bound and streaming.
    Bound,
}

/// What to open next, and after how long.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Reopen {
    intent: OpenIntent,
    delay: Duration,
}

/// What the line pump decided.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Flow {
    /// Keep pumping.
    Continue,
    /// Leave the bound state and reopen.
    Unbind(UnbindReason),
    /// Stop for good.
    Closed(SessionClosed),
}

/// Which future woke the pump.
enum Woke {
    Line(Option<Result<String, TransportError>>),
    Command(Option<SessionCommand>),
    Timer,
    /// The stop lane fired. See the module documentation.
    Stop,
}

/// How an attempt to establish a stream connection ended.
enum Opened {
    /// The transport accepted the request; `CONOK` or `CONERR` will follow on
    /// the line stream.
    Established,
    /// It could not be established, or not within the configured budget.
    Failed {
        /// What went wrong. Never contains a credential.
        detail: String,
    },
    /// A stop was ordered while the connection was being established.
    Stopped,
}

// ---------------------------------------------------------------------------
// The driver
// ---------------------------------------------------------------------------

/// Creates a session from a caller's configuration, choosing and building the
/// transport it asks for.
///
/// This is the **composition root**: the one place in the crate that turns a
/// public [`ClientConfig`] into a concrete transport adapter and the options
/// that drive it. Keeping it here rather than in the façade is what lets a
/// second transport be added without touching a semver-governed module
/// (`docs/adr/0007-transport-port-shape.md`, §3); the façade above never names
/// an adapter, and the adapters below never see a `ClientConfig`.
///
/// # Errors
///
/// - [`Error::Transport`] if the configured address cannot be turned into an
///   endpoint for the chosen transport.
/// - [`Error::Internal`] if the options and the transport contradict each
///   other, which only a bug in this translation can produce.
pub(crate) fn connect_configured(
    config: ClientConfig,
) -> Result<
    (
        SessionDriver<AnyTransport>,
        SessionHandle,
        mpsc::Receiver<SessionEvent>,
    ),
    crate::error::Error,
> {
    let transport = match config.transport() {
        crate::config::Transport::WebSocket => {
            AnyTransport::WebSocket(WsTransport::try_new(config.address().as_str())?)
        }
    };
    let options = SessionOptions::from_client_config(config);
    connect(transport, options).map_err(|error| crate::error::Error::Internal {
        reason: error.to_string(),
    })
}

/// Creates a session over `transport`.
///
/// Returns the driver — a single future that must be polled to completion,
/// typically by `tokio::spawn` — the caller's handle, and the event stream.
///
/// # Errors
///
/// [`SessionError::Configuration`] if the options ask for a polling connection
/// on a transport that does not poll, or the reverse. The two must agree
/// because `LS_polling` changes the meaning of the `CONOK` keep-alive argument
/// and forbids `LS_keepalive_millis` and `LS_inactivity_millis` entirely
/// [`docs/spec/02-session-lifecycle.md` §7.2, §8.4].
pub(crate) fn connect<T: Transport>(
    transport: T,
    options: SessionOptions,
) -> Result<
    (
        SessionDriver<T>,
        SessionHandle,
        mpsc::Receiver<SessionEvent>,
    ),
    SessionError,
> {
    let properties = transport.properties();
    if properties.is_polling != options.is_polling() {
        return Err(SessionError::Configuration {
            reason: format!(
                "transport declares is_polling={} but the connection mode asks for polling={}",
                properties.is_polling,
                options.is_polling()
            ),
        });
    }

    let (command_tx, command_rx) = mpsc::channel(options.command_capacity.get());
    let (event_tx, event_rx) = mpsc::channel(options.event_capacity.get());
    let (stop_tx, stop_rx) = watch::channel(false);

    let handle = SessionHandle {
        commands: command_tx,
        next_key: Arc::new(AtomicU64::new(1)),
        stop: Arc::new(stop_tx),
    };
    let driver = SessionDriver::new(
        transport, properties, options, command_rx, event_tx, stop_rx,
    );
    Ok((driver, handle, event_rx))
}

/// Resolves as soon as an immediate stop has been ordered, or every handle
/// that could order one has been dropped.
///
/// Cancel-safe: [`watch::Receiver::changed`] is, and the flag is re-read on
/// every turn, so dropping this future in a `select!` loses nothing.
async fn stopped(stop: &mut watch::Receiver<bool>) {
    loop {
        if *stop.borrow_and_update() {
            return;
        }
        if stop.changed().await.is_err() {
            // Every `SessionHandle` is gone, which is the other way the layer
            // above says it is finished.
            return;
        }
    }
}

/// How many consecutive input lines the pump processes before it must poll the
/// command channel and the liveness clocks, whatever the socket still holds.
///
/// The pump is biased towards the server so that the state machine cannot act
/// on a command against a session a line already ended. Bias without a bound is
/// starvation: a connection that always has a line ready would postpone a
/// heartbeat, a `destroy` and a shutdown indefinitely. This is the ceiling on
/// that latency — a control action waits at most this many notifications.
const MAX_LINES_PER_TURN: u32 = 32;

/// How many consecutive failures to send the outbound heartbeat are tolerated
/// before the stream connection is treated as lost.
///
/// A failed heartbeat leaves the outbound commitment unmet
/// [`docs/spec/02-session-lifecycle.md` §8.4], and a client that cannot write
/// to a socket has nothing to gain by holding it: after this many attempts the
/// connection is reopened, which is what the retry budget is there to bound.
const MAX_HEARTBEAT_FAILURES: u32 = 3;

/// The session state machine. One future, no spawned tasks.
#[derive(Debug)]
pub(crate) struct SessionDriver<T: Transport> {
    transport: T,
    properties: TransportProperties,
    options: SessionOptions,
    commands: mpsc::Receiver<SessionCommand>,
    events: mpsc::Sender<SessionEvent>,
    /// The stop lane. See the module documentation.
    stop: watch::Receiver<bool>,
    /// Set when a stop was ordered while the driver was blocked, so that the
    /// current turn unwinds instead of carrying on.
    stopping: bool,

    /// The current stream state, meaningful only while a stream connection is
    /// open.
    stream_state: StreamState,
    /// What the in-flight stream-opening request was.
    intent: OpenIntent,
    /// The session the server gave us, while it is still usable.
    session: Option<SessionId>,
    /// The last session that was abandoned — rejected on a bind, or ended by
    /// the server with a code that asks for a fresh one. Kept only so the
    /// replacement can be reported as [`BindKind::Recreated`] naming what it
    /// replaced (`docs/adr/0005-recovery-is-visible-in-the-event-stream.md`);
    /// it is never sent on the wire again.
    previous_session: Option<SessionId>,

    /// "The count of all the notifications received since the start of the
    /// session", which `LS_recovery_from` carries
    /// [`docs/spec/02-session-lifecycle.md` §5.2].
    progressive: u64,
    /// How many duplicated data notifications are still to be discarded after
    /// a `PROG` that resumed earlier than requested
    /// [`docs/spec/02-session-lifecycle.md` §5.2, precondition 4].
    skip_remaining: u64,

    /// Whether a `PROG` has already been honoured on the current stream
    /// connection. `PROG` is one of the notifications sent immediately after
    /// `CONOK` and only in answer to `LS_recovery_from`
    /// [`docs/spec/02-session-lifecycle.md` §5.4], so a second one — or one on
    /// a connection that asked for no recovery — correlates with nothing this
    /// client requested and must not move the recovery baseline.
    prog_honoured: bool,

    registry: Registry,
    pending: HashMap<String, Pending>,
    /// Which session this driver is on: incremented every time a session is
    /// created or recreated, never reset. Pending control requests carry it so
    /// that a late response from a replaced session cannot mutate the
    /// replacement's state [`docs/spec/02-session-lifecycle.md` §4.5].
    generation: u64,
    /// `LS_reqId` is "unique within the connection", where "connection" is
    /// never defined [`docs/spec/02-session-lifecycle.md` §4.4, ambiguity A5].
    ///
    /// SPEC-AMBIGUITY (A5): this counter is monotonic for the **whole life of
    /// the driver** and is never reset — not per socket, not per stream
    /// connection, not per session. That is at least as strict as every reading
    /// of the ambiguity, so it cannot collide under any of them, and it makes
    /// the late responses of §4.5 — "after binding the second session, it is
    /// possible that late responses to control requests related with the first
    /// session arrive interspersed with notifications for the second session" —
    /// harmless: a stale id can never be confused with a live one.
    next_request_id: u64,

    /// Set when `force_rebind` is sent, so the `LOOP` it provokes is attributed
    /// to T2 rather than to the transport's own reason
    /// [`docs/spec/02-session-lifecycle.md` §2.2, T2].
    force_rebind_outstanding: bool,
    /// Set when `destroy` is sent, so the `END` it provokes is attributed to
    /// the client [`docs/spec/02-session-lifecycle.md` §2.2, T7].
    destroy_requested: bool,

    /// Consecutive session refreshes (server code 48) with no productive
    /// session in between. See [`SessionDriver::plan_after`].
    refresh_streak: u32,
    /// Consecutive failures to send the outbound heartbeat.
    heartbeat_failures: u32,

    liveness: Liveness,
    backoff: Backoff,
}

impl<T: Transport> SessionDriver<T> {
    fn new(
        transport: T,
        properties: TransportProperties,
        options: SessionOptions,
        commands: mpsc::Receiver<SessionCommand>,
        events: mpsc::Sender<SessionEvent>,
        stop: watch::Receiver<bool>,
    ) -> Self {
        let liveness = Liveness::new(
            options.keepalive_slack,
            options.heartbeat_interval(),
            Instant::now(),
        );
        let backoff = Backoff::new(options.backoff);
        Self {
            transport,
            properties,
            options,
            commands,
            events,
            stop,
            stopping: false,
            stream_state: StreamState::Establishing,
            intent: OpenIntent::Create,
            session: None,
            previous_session: None,
            progressive: 0,
            skip_remaining: 0,
            prog_honoured: false,
            registry: Registry::new(),
            pending: HashMap::new(),
            generation: 1,
            next_request_id: 1,
            force_rebind_outstanding: false,
            destroy_requested: false,
            refresh_streak: 0,
            heartbeat_failures: 0,
            liveness,
            backoff,
        }
    }

    /// The session's data-notification count so far — what a recovery bind
    /// would ask to resume from [`docs/spec/02-session-lifecycle.md` §5.2].
    #[must_use]
    #[inline]
    pub(crate) const fn progressive(&self) -> u64 {
        self.progressive
    }

    /// The session identifier, once the server has supplied one.
    #[must_use]
    #[inline]
    pub(crate) const fn session_id(&self) -> Option<&SessionId> {
        self.session.as_ref()
    }

    /// Runs the session to its terminal state.
    ///
    /// Returns why it ended; the same value is also the last event on the event
    /// channel. The transport is closed before this returns on **every** path.
    pub(crate) async fn run(mut self) -> SessionClosed {
        let closed = self.drive().await;
        if let Err(error) = self.transport.close().await {
            tracing::warn!(%error, "transport did not close cleanly");
        }
        // Best effort: a caller that dropped the receiver is not an error, and
        // a caller that stopped reading must not be able to keep this task
        // alive — `emit` gives the terminal event up when a stop is ordered.
        self.emit(SessionEvent::Closed(closed.clone())).await;
        tracing::info!(?closed, "session ended");
        closed
    }

    /// The outer loop: open a stream connection, pump it, decide what to open
    /// next. Each turn is one edge of the specification's state diagram
    /// [`docs/spec/02-session-lifecycle.md` §2.3].
    async fn drive(&mut self) -> SessionClosed {
        let mut next = Reopen {
            intent: OpenIntent::Create,
            delay: Duration::ZERO,
        };
        loop {
            if let Some(closed) = self.wait_before_reopen(next.delay).await {
                return closed;
            }

            let request = match self.build_open(&next.intent) {
                Ok(request) => request,
                Err(error) => {
                    return SessionClosed::Internal {
                        reason: error.to_string(),
                    };
                }
            };
            self.intent = next.intent.clone();
            self.stream_state = StreamState::Establishing;
            self.skip_remaining = 0;
            self.prog_honoured = false;

            let flow = match self.open_stream(request).await {
                Opened::Established => {
                    self.liveness
                        .on_stream_opened(self.options.open_timeout, Instant::now());
                    self.pump().await
                }
                Opened::Failed { detail } => {
                    Flow::Unbind(UnbindReason::ConnectionFailed { detail })
                }
                Opened::Stopped => {
                    return SessionClosed::ByClient {
                        destroy_confirmed: false,
                        cause: None,
                    };
                }
            };

            match flow {
                Flow::Closed(closed) => return closed,
                Flow::Continue | Flow::Unbind(_) => {}
            }
            let reason = match flow {
                Flow::Unbind(reason) => reason,
                // The pump only ever returns `Unbind` or `Closed`; treating a
                // `Continue` as a connection failure keeps this total without
                // an `unreachable!`.
                _ => UnbindReason::ConnectionFailed {
                    detail: "stream ended without a reason".to_owned(),
                },
            };

            self.liveness.on_stream_closed();
            self.stream_state = StreamState::Establishing;

            // After a clean `LOOP` the connection may legitimately be reused —
            // with WS "the server sends the *Loop* command exactly as in the
            // HTTP case, but it does not close the connection", and the client
            // "is free to choose whether to rebind the session to the same
            // stream connection or open a new one"
            // [`docs/spec/02-session-lifecycle.md` §4.3], a choice the
            // transport is the only layer able to make.
            //
            // After anything else it may not. A stalled connection must be
            // "closed and reopened" [ibid. §8.1], and a recovery request
            // "invalidates any further notifications (of any type) that may be
            // later received on the original connection" [ibid. §5.1] — so the
            // wedged socket is torn down here rather than left to be collected
            // whenever the transport happens to replace it.
            if !reason.is_clean_loop()
                && let Err(error) = self.transport.close().await
            {
                tracing::debug!(%error, "closing the failed stream connection");
            }

            match self.plan_after(&reason) {
                Some(reopen) => {
                    self.emit(SessionEvent::Unbound {
                        reason: reason.clone(),
                        retry_in: Some(reopen.delay),
                    })
                    .await;
                    if self.stopping {
                        return SessionClosed::ByClient {
                            destroy_confirmed: false,
                            cause: None,
                        };
                    }
                    tracing::debug!(?reason, delay = ?reopen.delay, intent = ?reopen.intent, "reopening");
                    next = reopen;
                }
                None => {
                    self.emit(SessionEvent::Unbound {
                        reason: reason.clone(),
                        retry_in: None,
                    })
                    .await;
                    return SessionClosed::RetriesExhausted {
                        attempts: self.backoff.attempts(),
                        last: Some(reason),
                    };
                }
            }
        }
    }

    /// Opens the next stream connection under the configured budget.
    ///
    /// The budget covers the **whole** establishment — name resolution, the
    /// TCP and TLS handshakes, the protocol handshake, and the first write —
    /// not merely the wait for `CONOK` that follows it. The specification
    /// defines no such limit; this is the client-side one of
    /// [`SessionOptions::open_timeout`], and without it a connect that hangs
    /// below the port hangs the session for as long as the operating system
    /// allows. The stop lane is honoured throughout, so a client being torn
    /// down does not wait out a dial to an unreachable host.
    async fn open_stream(&mut self, request: StreamOpen) -> Opened {
        let budget = self.options.open_timeout;
        let transport = &mut self.transport;
        let stop = &mut self.stop;
        tokio::select! {
            biased;
            result = tokio::time::timeout(budget, transport.open_stream(request)) => match result {
                Ok(Ok(())) => Opened::Established,
                Ok(Err(error)) => Opened::Failed { detail: error.to_string() },
                Err(_elapsed) => {
                    tracing::warn!(?budget, "the stream connection could not be established in time");
                    Opened::Failed {
                        detail: format!("the stream connection was not established within {budget:?}"),
                    }
                }
            },
            () = stopped(stop) => Opened::Stopped,
        }
    }

    /// Waits out a reconnection delay while still serving commands, so a
    /// shutdown or a destroy is not stuck behind a backoff.
    async fn wait_before_reopen(&mut self, delay: Duration) -> Option<SessionClosed> {
        if delay.is_zero() {
            return None;
        }
        let deadline = Instant::now()
            .checked_add(delay)
            .unwrap_or_else(Instant::now);
        loop {
            let woke = tokio::select! {
                biased;
                command = self.commands.recv() => Woke::Command(command),
                () = stopped(&mut self.stop) => Woke::Stop,
                () = tokio::time::sleep_until(deadline) => Woke::Timer,
            };
            match woke {
                Woke::Timer => return None,
                Woke::Stop => {
                    return Some(SessionClosed::ByClient {
                        destroy_confirmed: false,
                        cause: None,
                    });
                }
                Woke::Command(None) => {
                    return Some(SessionClosed::ByClient {
                        destroy_confirmed: false,
                        cause: None,
                    });
                }
                Woke::Command(Some(command)) => match self.on_command(command).await {
                    Flow::Closed(closed) => return Some(closed),
                    Flow::Continue | Flow::Unbind(_) => {
                        if self.stopping {
                            return Some(SessionClosed::ByClient {
                                destroy_confirmed: false,
                                cause: None,
                            });
                        }
                    }
                },
                Woke::Line(_) => {}
            }
        }
    }

    /// Builds the request that opens the next stream connection.
    ///
    /// `LS_reduce_head` is never set. On a `bind_session` it would suppress
    /// `CONOK` itself, and the spec then defines no substitute signal for a
    /// successful bind, nor any way to tell success from failure
    /// [`docs/spec/02-session-lifecycle.md` §3.2, ambiguity A4]. `CONOK` is
    /// this state machine's only evidence of the *Unbound → Bound* transition,
    /// so it is never asked to be withheld.
    fn build_open(&mut self, intent: &OpenIntent) -> Result<StreamOpen, ProtocolError> {
        let session = match (&self.session, intent) {
            (Some(session), OpenIntent::Rebind | OpenIntent::Recover { .. }) => Some(session),
            _ => None,
        };
        match (session, intent) {
            (Some(session), OpenIntent::Recover { requested }) => {
                Ok(StreamOpen::Bind(Box::new(BindSession {
                    // Always explicit, even on WS where it may be defaulted to
                    // "the last session that was bound to the WS connection"
                    // [`docs/spec/02-session-lifecycle.md` §4.4]. Naming it
                    // removes any question of which session a rebind applies to
                    // when late responses from a previous one are still in
                    // flight [ibid. §4.5].
                    session: Some(session.as_str().to_owned()),
                    recovery_from: Some(*requested),
                    content_length: self.options.content_length,
                    connection: self.options.connection,
                    reduce_head: None,
                })))
            }
            (Some(session), OpenIntent::Rebind) => Ok(StreamOpen::Bind(Box::new(BindSession {
                session: Some(session.as_str().to_owned()),
                recovery_from: None,
                content_length: self.options.content_length,
                connection: self.options.connection,
                reduce_head: None,
            }))),
            _ => {
                let credentials = &self.options.credentials;
                Ok(StreamOpen::Create(Box::new(CreateSession {
                    user: credentials.user.clone(),
                    password: credentials.password.clone(),
                    adapter_set: credentials.adapter_set.clone(),
                    requested_max_bandwidth: None,
                    content_length: self.options.content_length,
                    connection: self.options.connection,
                    reduce_head: None,
                    ttl: None,
                })))
            }
        }
    }

    /// Chooses what to open after an unbind, and how long to wait first.
    ///
    /// `None` means the retry budget is spent and the session is definitively
    /// lost.
    fn plan_after(&mut self, reason: &UnbindReason) -> Option<Reopen> {
        // A clean `LOOP` is not a failure: the server told us to rebind and
        // guaranteed continuity, so the backoff schedule is irrelevant and the
        // only delay is the one `LOOP` asked for
        // [`docs/spec/02-session-lifecycle.md` §4.2, §4.4].
        if reason.is_clean_loop() {
            self.backoff.reset();
            self.refresh_streak = 0;
            return Some(Reopen {
                intent: OpenIntent::Rebind,
                delay: reason.expected_delay().unwrap_or(Duration::ZERO),
            });
        }

        if !matches!(reason, UnbindReason::ServerRefresh { .. }) {
            self.refresh_streak = 0;
        }

        match reason {
            // T5, and the mute-connection case of §5.1. The session is not
            // closed, so recovery — not a plain rebind — is the right request:
            // a plain rebind after a drop "silently loses data"
            // [`docs/spec/02-session-lifecycle.md` §4.4].
            UnbindReason::ConnectionFailed { .. } | UnbindReason::KeepaliveExpired { .. } => {
                let delay = self.backoff.next_delay()?;
                let intent = if self.session.is_some() {
                    OpenIntent::Recover {
                        requested: self.progressive,
                    }
                } else {
                    OpenIntent::Create
                };
                Some(Reopen { intent, delay })
            }

            // "the client should create a new stream connection, as if it was
            // the first time it connects to the Server, and re-execute all the
            // subscriptions" [`docs/spec/02-session-lifecycle.md` §6.1].
            UnbindReason::Rejected { .. } => {
                let delay = self.backoff.next_delay()?;
                Some(Reopen {
                    intent: OpenIntent::Create,
                    delay,
                })
            }

            // Code 48: "the client should recover by opening a new session
            // immediately" [`docs/spec/05-error-codes.md` §5.4]. Immediately
            // means no backoff, and the schedule is reset because nothing
            // failed.
            //
            // Only the first one, though. A server that answers every fresh
            // session with another refresh would otherwise get an unthrottled
            // reconnect loop that never exhausts, because the `CONOK` it sends
            // first resets the retry budget every time. Consecutive refreshes
            // with no session productive enough to deliver a single data
            // notification in between are therefore delayed like any other
            // failed attempt, and counted towards a definitive loss. The
            // streak is cleared by the first data notification
            // ([`SessionDriver::on_data`]) and by any other unbind reason, so
            // a deployment that legitimately refreshes sessions periodically
            // never accumulates one.
            UnbindReason::ServerRefresh { .. } => {
                self.refresh_streak = self.refresh_streak.saturating_add(1);
                if self.refresh_streak <= 1 {
                    self.backoff.reset();
                    return Some(Reopen {
                        intent: OpenIntent::Create,
                        delay: Duration::ZERO,
                    });
                }
                if let Some(max) = self.options.backoff.max_attempts
                    && self.refresh_streak > max.get()
                {
                    tracing::warn!(
                        streak = self.refresh_streak,
                        "the server kept asking for a fresh session; giving up"
                    );
                    return None;
                }
                let delay = self.backoff.next_delay()?;
                tracing::warn!(
                    streak = self.refresh_streak,
                    ?delay,
                    "repeated session refresh; delaying the next attempt"
                );
                Some(Reopen {
                    intent: OpenIntent::Create,
                    delay,
                })
            }

            // Handled by `is_clean_loop` above.
            UnbindReason::ForcedByClient { .. }
            | UnbindReason::ContentLengthReached { .. }
            | UnbindReason::PollCycleExpired { .. }
            | UnbindReason::Looped { .. } => Some(Reopen {
                intent: OpenIntent::Rebind,
                delay: Duration::ZERO,
            }),
        }
    }

    /// Reads lines until the stream connection ends or the session does.
    async fn pump(&mut self) -> Flow {
        let mut consecutive_lines: u32 = 0;
        loop {
            let deadline = self.liveness.next_deadline();
            // Deliberately biased, server first. The state machine must stay in
            // step with the server: a command issued against a session that has
            // already been unbound by a line sitting unread in the buffer would
            // be sent into a connection that no longer exists. Commands wait at
            // most until the line stream goes quiet, which on any real
            // connection is immediately — and the liveness timer, being last,
            // can only fire when neither of the other two has anything, which
            // is exactly the condition it exists to detect.
            //
            // Every `MAX_LINES_PER_TURN` lines the bias is inverted for one
            // turn. Without that, a connection that always has a line ready —
            // a busy server, or a hostile one — would postpone every command
            // and every liveness deadline for as long as it kept talking. The
            // inversion costs one reordering per 32 notifications and bounds
            // the latency of a `destroy`, a shutdown and a heartbeat.
            //
            // This requires `next_line` to be cancel-safe: the future is
            // dropped whenever another branch wins, and a line must not be lost
            // with it.
            let woke = if consecutive_lines >= MAX_LINES_PER_TURN {
                consecutive_lines = 0;
                tokio::select! {
                    biased;
                    command = self.commands.recv() => Woke::Command(command),
                    () = stopped(&mut self.stop) => Woke::Stop,
                    () = sleep_until_option(deadline) => Woke::Timer,
                    line = self.transport.next_line() => Woke::Line(line),
                }
            } else {
                tokio::select! {
                    biased;
                    line = self.transport.next_line() => Woke::Line(line),
                    command = self.commands.recv() => Woke::Command(command),
                    () = stopped(&mut self.stop) => Woke::Stop,
                    () = sleep_until_option(deadline) => Woke::Timer,
                }
            };
            consecutive_lines = if matches!(woke, Woke::Line(_)) {
                consecutive_lines.saturating_add(1)
            } else {
                0
            };

            let flow = match woke {
                Woke::Line(Some(Ok(line))) => self.on_line(line).await,
                Woke::Line(Some(Err(error))) => Flow::Unbind(UnbindReason::ConnectionFailed {
                    detail: error.to_string(),
                }),
                Woke::Line(None) => self.on_stream_end(),
                Woke::Command(Some(command)) => self.on_command(command).await,
                // Every handle was dropped: nobody can command the session and
                // nobody is left to care about its events.
                Woke::Command(None) | Woke::Stop => Flow::Closed(SessionClosed::ByClient {
                    destroy_confirmed: false,
                    cause: None,
                }),
                Woke::Timer => self.on_timer().await,
            };

            // A stop ordered while an event was waiting for room unwinds here:
            // the event is gone, and so is any reason to keep pumping.
            if self.stopping {
                return Flow::Closed(SessionClosed::ByClient {
                    destroy_confirmed: false,
                    cause: None,
                });
            }

            match flow {
                Flow::Continue => {}
                other => return other,
            }
        }
    }

    /// The transport reported a clean end of stream, with no `LOOP` and no
    /// error.
    fn on_stream_end(&mut self) -> Flow {
        if self.destroy_requested {
            // The destroy was sent; the connection ending without the `END`
            // that §2.2 T7 promises is still an ended session.
            return Flow::Closed(SessionClosed::ByClient {
                destroy_confirmed: false,
                cause: None,
            });
        }
        if self.properties.is_polling {
            // SPEC-AMBIGUITY: §7.1 describes the long-polling cycle as the
            // server unbinding "and closing the stream connection" without
            // mentioning `LOOP`, while §2.2 T4 says a `LOOP` is what marks the
            // poll cycle expiring. A polling transport that ends its stream
            // without a `LOOP` is therefore read as T4 — the benign reading —
            // rather than as T5. Reading it as T5 would ask the server to
            // recover a session that was never interrupted, on every single
            // poll cycle.
            return Flow::Unbind(UnbindReason::PollCycleExpired {
                expected_delay: Duration::ZERO,
            });
        }
        // T5: "a stream connection is dropped by any cause… upon connection
        // drop, the session is not closed, hence the Client is still allowed to
        // issue a rebind" [`docs/spec/02-session-lifecycle.md` §2.2, T5].
        Flow::Unbind(UnbindReason::ConnectionFailed {
            detail: "stream connection closed by the peer".to_owned(),
        })
    }

    /// A liveness deadline fell due.
    async fn on_timer(&mut self) -> Flow {
        match self.liveness.due(Instant::now()) {
            LivenessAction::Idle => Flow::Continue,
            LivenessAction::SendHeartbeat => self.send_heartbeat().await,
            LivenessAction::InboundStalled { budget } => match self.stream_state {
                // Nothing answered the handshake within the client's own limit.
                // The spec sets no such limit; see `SessionOptions::open_timeout`.
                StreamState::Establishing => Flow::Unbind(UnbindReason::ConnectionFailed {
                    detail: format!("no response to the stream-opening request within {budget:?}"),
                }),
                // §8.1: "if after the expected interval plus a configurable
                // timeout no `PROBE` has been received, the connection is closed
                // and reopened".
                StreamState::Bound => Flow::Unbind(UnbindReason::KeepaliveExpired { budget }),
            },
        }
    }

    /// One line from the server.
    async fn on_line(&mut self, line: String) -> Flow {
        // Every line resets the inbound clock, not only `PROBE`: the keep-alive
        // is the longest interval with no traffic of any kind
        // [`docs/spec/02-session-lifecycle.md` §8.1].
        self.liveness.on_inbound(Instant::now());

        match parse_line(&line) {
            Ok(notification) => self.on_notification(notification).await,
            Err(error) => {
                // SPEC-AMBIGUITY: an unparsed line is *not* counted toward the
                // recovery progressive, because this client cannot know whether
                // it was a data notification — and the one group the spec
                // leaves unclassified, MPN, is exactly the group this client
                // does not parse [`docs/spec/02-session-lifecycle.md` §5.3,
                // ambiguity A6]. Under-counting is the safe direction: it makes
                // a later recovery ask to resume from an *earlier* point, which
                // the protocol handles by re-delivering duplicates (§5.2), the
                // very case §5.2's fourth precondition is written for.
                // Over-counting would silently skip data.
                tracing::warn!(%error, "unrecognized line preserved for the caller");
                self.emit(SessionEvent::Unparsed { line, error }).await;
                Flow::Continue
            }
        }
    }

    /// The state machine proper: one arm per notification that means something
    /// to the lifecycle, everything else forwarded or counted.
    async fn on_notification(&mut self, notification: Notification) -> Flow {
        match notification {
            // T1 and T6 — the only evidence of a successful bind
            // [`docs/spec/02-session-lifecycle.md` §2.2, T1/T6].
            Notification::ConnectionOk {
                session_id,
                request_limit_bytes,
                keep_alive_millis,
                control_link,
            } => {
                self.on_conok(
                    SessionId::new(session_id),
                    request_limit_bytes,
                    Duration::from_millis(keep_alive_millis),
                    control_link,
                )
                .await;
                Flow::Continue
            }

            // The session could not be created or bound
            // [`docs/spec/02-session-lifecycle.md` §6.2].
            Notification::ConnectionError { code, message } => {
                let cause = ServerCause { code, message };
                match classify(code) {
                    Recovery::Fatal => {
                        tracing::warn!(code, "session refused with a code that admits no retry");
                        Flow::Closed(SessionClosed::ByServer { cause })
                    }
                    Recovery::RecreateSession | Recovery::RetryLater => {
                        // A rejected bind means the old session is unusable:
                        // it is retired so the next attempt creates a new one
                        // and re-executes every subscription [§6.1], while its
                        // identifier is kept for one more bind so the caller
                        // learns what the new session replaced.
                        self.previous_session = self.session.take();
                        Flow::Unbind(UnbindReason::Rejected { cause })
                    }
                }
            }

            // The server closed the session
            // [`docs/spec/02-session-lifecycle.md` §2.2 note, §6.2].
            Notification::End {
                cause_code,
                cause_message,
            } => self.on_end(cause_code, cause_message),

            // T2, T3 or T4, depending on why
            // [`docs/spec/02-session-lifecycle.md` §2.2, §4.2].
            Notification::Loop {
                expected_delay_millis,
            } => Flow::Unbind(self.loop_reason(Duration::from_millis(expected_delay_millis))),

            // Where a recovered flow resumes
            // [`docs/spec/02-session-lifecycle.md` §5.4].
            Notification::Progressive { progressive } => {
                self.on_prog(progressive).await;
                Flow::Continue
            }

            // A control request was answered [`docs/spec/03-requests.md` §13].
            Notification::RequestOk { request_id } => {
                self.on_request_ok(request_id).await;
                Flow::Continue
            }
            Notification::RequestError {
                request_id,
                code,
                message,
            } => {
                self.on_request_error(request_id, ServerCause { code, message })
                    .await;
                Flow::Continue
            }
            // `ERROR` carries no request id "because the server could not parse
            // one" [`docs/spec/03-requests.md` §13.2], so nothing can be
            // correlated; it is surfaced uncorrelated rather than guessed at.
            Notification::Error { code, message } => {
                tracing::warn!(code, "server rejected a request it could not correlate");
                self.emit(SessionEvent::ControlResponse {
                    request_id: None,
                    // `ERROR` names no request, so there is nothing to route
                    // it to [`docs/spec/03-requests.md` §13.2].
                    target: ControlTarget::Session,
                    outcome: ControlOutcome::Rejected {
                        cause: ServerCause { code, message },
                    },
                })
                .await;
                Flow::Continue
            }

            // Keep-alive: already accounted for by `on_line`, and explicitly
            // not a data notification
            // [`docs/spec/02-session-lifecycle.md` §5.3, §8.1].
            Notification::Probe => {
                tracing::trace!("probe");
                Flow::Continue
            }
            // "The purpose of this notification is to fill the receive buffer";
            // the payload "is to be ignored"
            // [`docs/spec/04-notifications.md` §5.6].
            Notification::NoOp { .. } => Flow::Continue,
            // The WebSocket establishment check's echo; it means the channel
            // carries TLCP end to end [`docs/spec/03-requests.md` §15.2].
            Notification::WsOk => {
                tracing::debug!("websocket establishment check acknowledged");
                Flow::Continue
            }

            // Session-related, not data
            // [`docs/spec/02-session-lifecycle.md` §5.3].
            Notification::Sync { elapsed_seconds } => {
                self.emit(SessionEvent::ServerInfo(ServerAnnouncement::Clock {
                    elapsed_seconds,
                }))
                .await;
                Flow::Continue
            }
            Notification::ClientIp { address } => {
                self.emit(SessionEvent::ServerInfo(ServerAnnouncement::ClientIp(
                    address,
                )))
                .await;
                Flow::Continue
            }
            Notification::ServerName { name } => {
                self.emit(SessionEvent::ServerInfo(ServerAnnouncement::Name(name)))
                    .await;
                Flow::Continue
            }
            Notification::ConstraintsChanged { bandwidth } => {
                self.emit(SessionEvent::ServerInfo(ServerAnnouncement::Bandwidth(
                    bandwidth,
                )))
                .await;
                Flow::Continue
            }

            // Everything left is a data notification
            // [`docs/spec/02-session-lifecycle.md` §5.3].
            data => {
                self.on_data(data).await;
                Flow::Continue
            }
        }
    }

    /// Handles `CONOK`: the *Unbound → Bound* transition
    /// [`docs/spec/02-session-lifecycle.md` §2.2, T1/T6].
    async fn on_conok(
        &mut self,
        session_id: SessionId,
        request_limit_bytes: u64,
        keep_alive: Duration,
        control_link: Option<String>,
    ) {
        self.stream_state = StreamState::Bound;
        self.liveness.on_bound(keep_alive, Instant::now());
        self.backoff.reset();

        // "upon the first time the 'control link' address is used to open a new
        // stream connection to rebind to a session, the streaming activity…
        // will switch to the 'control link'"; the literal `*` means keep using
        // the current address [`docs/spec/02-session-lifecycle.md` §3.1].
        self.transport.set_control_link(control_link.as_deref());

        let previous = self.session.take().or_else(|| self.previous_session.take());
        let same_session = previous.as_ref() == Some(&session_id);
        self.session = Some(session_id.clone());
        self.previous_session = None;

        let kind = match (&self.intent, same_session) {
            (OpenIntent::Rebind, true) => BindKind::Rebound,
            (OpenIntent::Recover { requested }, true) => BindKind::Recovering {
                requested_progressive: *requested,
            },
            // A `bind_session` answered with a *different* session id. The spec
            // does not contemplate it — every transcript shows the id unchanged
            // across a rebind and a recovery
            // [`docs/spec/02-session-lifecycle.md` §10, F7/F8].
            //
            // SPEC-AMBIGUITY: nothing says what a client should do here.
            // Treating it as a replacement is the only safe reading: the
            // progressive and the `LS_subId` space belong to a session, so
            // carrying them into a different one would corrupt both.
            (OpenIntent::Rebind | OpenIntent::Recover { .. }, false) => {
                tracing::warn!(
                    old = %previous.as_ref().map_or("<none>", SessionId::as_str),
                    new = %session_id,
                    "bind returned a different session id; treating it as a new session"
                );
                BindKind::Recreated { previous }
            }
            (OpenIntent::Create, _) => match previous {
                Some(previous) => BindKind::Recreated {
                    previous: Some(previous),
                },
                None => BindKind::Created,
            },
        };

        // A new session restarts the progressive at zero and the `LS_subId`
        // space at 1 [`docs/spec/02-session-lifecycle.md` §9.5].
        let fresh = matches!(kind, BindKind::Created | BindKind::Recreated { .. });
        if fresh {
            self.progressive = 0;
            self.skip_remaining = 0;
            self.registry.reset_for_new_session();
            self.generation = self.generation.saturating_add(1);
            self.retire_stale_pending().await;
        }

        tracing::info!(session = %session_id, ?kind, ?keep_alive, "session bound");
        self.emit(SessionEvent::Bound(Box::new(BoundInfo {
            session_id,
            kind,
            keep_alive,
            request_limit_bytes,
            control_link,
        })))
        .await;

        // On a rebind or a recovery the server preserves every subscription
        // "with all their items and fields", so re-issuing them would create
        // duplicates [`docs/spec/02-session-lifecycle.md` §4.4]. Only
        // subscriptions with no binding in the current session are issued —
        // which, after a replacement, is all of them.
        self.issue_unbound_subscriptions().await;
    }

    /// Retires every control request issued by a session that has been
    /// replaced.
    ///
    /// §4.5 promises the case outright: "after binding the second session, it
    /// is possible that late responses to control requests related with the
    /// first session arrive interspersed with notifications for the second
    /// session" [`docs/spec/02-session-lifecycle.md` §4.5]. Once the session
    /// they named is gone, their responses can say nothing true about the
    /// replacement — an `add` refused on the old session says nothing about the
    /// `add` just re-issued on the new one — so they are closed out here, at
    /// the boundary, and anything that arrives for them afterwards is reported
    /// but acts on nothing.
    ///
    /// Every retired request produces exactly one outcome, which is what keeps
    /// "every message gets exactly one report" true across a session
    /// replacement.
    async fn retire_stale_pending(&mut self) {
        let generation = self.generation;
        let stale: Vec<(String, PendingKind)> = self
            .pending
            .extract_if(|_, pending| pending.generation != generation)
            .map(|(id, pending)| (id, pending.kind))
            .collect();
        if stale.is_empty() {
            return;
        }
        tracing::debug!(
            count = stale.len(),
            "retiring control requests issued by a session that was replaced"
        );
        for (request_id, kind) in stale {
            let target = kind.target();
            // No `LOOP` can now be attributed to a `force_rebind` the previous
            // session never answered.
            if matches!(kind, PendingKind::ForceRebind) {
                self.force_rebind_outstanding = false;
            }
            if let PendingKind::SendMessage { sequence, prog } = kind {
                self.emit(SessionEvent::Message {
                    progressive: None,
                    sequence,
                    prog,
                    result: MessageResult::NotSent {
                        reason: "the session the message was sent on was replaced".to_owned(),
                    },
                })
                .await;
            }
            self.emit(SessionEvent::ControlResponse {
                request_id: RequestId::try_new(request_id).ok(),
                target,
                outcome: ControlOutcome::NotSent {
                    reason: "the session the request was issued on was replaced".to_owned(),
                },
            })
            .await;
        }
    }

    /// Handles `END` [`docs/spec/02-session-lifecycle.md` §6.2].
    fn on_end(&mut self, cause_code: i64, cause_message: String) -> Flow {
        let cause = ServerCause {
            code: cause_code,
            message: cause_message,
        };
        if self.destroy_requested {
            // T7: "If successful, an `END` notification is sent on the stream
            // connection. After that… the session is terminated"
            // [`docs/spec/02-session-lifecycle.md` §2.2, T7]. The default code
            // is 31 [ibid. §10, F5].
            return Flow::Closed(SessionClosed::ByClient {
                destroy_confirmed: true,
                cause: Some(cause),
            });
        }
        // SPEC-AMBIGUITY (A2): "the p.8 diagram has no edge labelled for a
        // server-initiated `END` on a bound session… The spec does not
        // reconcile the diagram with the `END` cause codes of Appendix A"
        // [`docs/spec/02-session-lifecycle.md` §2.2]. It is treated here as an
        // unlabelled edge out of *Bound*, terminal by default, with the single
        // documented exception of code 48.
        match classify(cause_code) {
            Recovery::RecreateSession => {
                tracing::info!(code = cause_code, "server asked for a fresh session");
                self.previous_session = self.session.take();
                Flow::Unbind(UnbindReason::ServerRefresh { cause })
            }
            Recovery::RetryLater => {
                self.previous_session = self.session.take();
                Flow::Unbind(UnbindReason::Rejected { cause })
            }
            Recovery::Fatal => {
                tracing::warn!(code = cause_code, "server ended the session");
                Flow::Closed(SessionClosed::ByServer { cause })
            }
        }
    }

    /// Attributes a `LOOP` to the transition that caused it.
    ///
    /// The attribution reads declared transport properties, never a transport
    /// identity (`docs/adr/0007-transport-port-shape.md`).
    fn loop_reason(&mut self, expected_delay: Duration) -> UnbindReason {
        if self.force_rebind_outstanding {
            self.force_rebind_outstanding = false;
            // T2 — "If successful, a `LOOP` notification is sent on the stream
            // connection" [`docs/spec/02-session-lifecycle.md` §2.2, T2].
            return UnbindReason::ForcedByClient { expected_delay };
        }
        if self.properties.is_polling {
            // T4 — "The polling cycle is complete"
            // [`docs/spec/02-session-lifecycle.md` §2.2, T4].
            return UnbindReason::PollCycleExpired { expected_delay };
        }
        if self.properties.ends_on_content_length {
            // T3 — "The `Content-Length` of an HTTP stream connection has been
            // reached" [`docs/spec/02-session-lifecycle.md` §2.2, T3].
            return UnbindReason::ContentLengthReached { expected_delay };
        }
        UnbindReason::Looped { expected_delay }
    }

    /// Handles `PROG` [`docs/spec/02-session-lifecycle.md` §5.4].
    ///
    /// Only a `PROG` that answers a recovery this client asked for is obeyed.
    /// The notification is "sent only when requested via `LS_recovery_from` on
    /// `bind_session` requests", and it is one of the notifications "sent
    /// immediately after `CONOK`" [ibid. §5.4] — so exactly one is expected per
    /// recovery bind and none at all otherwise. Obeying an unsolicited one
    /// would let a single line move the recovery baseline to any value at all,
    /// and every later decision about continuity and duplicates rests on that
    /// number. An unexpected one is reported and ignored.
    async fn on_prog(&mut self, resumed_at: u64) {
        let requested = match self.intent {
            OpenIntent::Recover { requested } if !self.prog_honoured => requested,
            OpenIntent::Recover { .. } => {
                tracing::warn!(
                    resumed_at,
                    "ignoring a second PROG on one stream connection"
                );
                return;
            }
            _ => {
                tracing::warn!(
                    resumed_at,
                    "ignoring a PROG that answers no recovery request"
                );
                return;
            }
        };
        self.prog_honoured = true;

        let kind = match resumed_at.cmp(&requested) {
            std::cmp::Ordering::Equal => RecoveryKind::Exact,

            // Precondition 4: "the first data notifications received are
            // discarded, until the desired point is reached"
            // [`docs/spec/02-session-lifecycle.md` §5.2].
            std::cmp::Ordering::Less => match requested.checked_sub(resumed_at) {
                Some(count) => {
                    self.skip_remaining = count;
                    RecoveryKind::Duplicated { count }
                }
                // Unreachable: the ordering was established by the match.
                None => RecoveryKind::Exact,
            },

            // SPEC-AMBIGUITY (A7): see `RecoveryKind::Gap`.
            std::cmp::Ordering::Greater => match resumed_at.checked_sub(requested) {
                Some(missing) => {
                    self.progressive = resumed_at;
                    tracing::warn!(
                        requested,
                        resumed_at,
                        missing,
                        "recovery resumed past the requested point"
                    );
                    RecoveryKind::Gap { missing }
                }
                None => RecoveryKind::Exact,
            },
        };

        self.emit(SessionEvent::Recovered(RecoveryOutcome {
            requested,
            resumed_at,
            kind,
        }))
        .await;
    }

    /// Counts, annotates and forwards a data notification
    /// [`docs/spec/02-session-lifecycle.md` §5.3].
    async fn on_data(&mut self, notification: Notification) {
        // A session that delivered data was productive, whatever it does next:
        // it is not part of a refresh loop. See [`SessionDriver::plan_after`].
        self.refresh_streak = 0;

        if self.skip_remaining > 0 {
            // Guarded by the branch above, so the subtraction cannot wrap.
            if let Some(remaining) = self.skip_remaining.checked_sub(1) {
                self.skip_remaining = remaining;
            }
            tracing::trace!(
                remaining = self.skip_remaining,
                "discarding a duplicate re-delivered by recovery"
            );
            return;
        }

        self.progressive = match self.progressive.checked_add(1) {
            Some(next) => next,
            None => {
                tracing::error!(
                    "data-notification counter overflowed; recovery is no longer exact"
                );
                self.progressive
            }
        };

        let progressive = self.progressive;

        // `MSGDONE` and `MSGFAIL` were counted above like every other data
        // notification [`docs/spec/02-session-lifecycle.md` §5.3]; they are
        // merely delivered on the message surface instead of the raw one,
        // because they answer a `msg` the caller sent.
        match notification {
            Notification::MessageDone {
                sequence,
                prog,
                response,
            } => {
                self.emit(SessionEvent::Message {
                    progressive: Some(progressive),
                    sequence,
                    prog: Some(prog),
                    result: MessageResult::Done { response },
                })
                .await;
            }
            Notification::MessageFailed {
                sequence,
                prog,
                code,
                message,
            } => {
                self.emit(SessionEvent::Message {
                    progressive: Some(progressive),
                    sequence,
                    prog: Some(prog),
                    result: MessageResult::Failed {
                        cause: ServerCause { code, message },
                    },
                })
                .await;
            }
            notification => {
                if let Some(event) = self.interpret_for_subscription(progressive, &notification) {
                    self.emit(event).await;
                }
            }
        }
    }

    /// Applies a subscription-bearing notification to the subscription it names
    /// and produces the event that goes upward, if any.
    ///
    /// The wire line stops here. The layer above receives a typed
    /// [`SubscriptionEvent`] — or the typed reason the line could not be
    /// reconciled — and never a [`Notification`], which is what keeps the
    /// public facade independent of the parser.
    fn interpret_for_subscription(
        &mut self,
        progressive: u64,
        notification: &Notification,
    ) -> Option<SessionEvent> {
        let Some((sub_id, key)) = self.resolve_subscription(notification) else {
            // A data notification naming no subscription this client knows:
            // either it carries no `<subscription-ID>` at all, or it names one
            // that has already gone. Logged rather than invented into an event.
            tracing::debug!(?notification, "unattributed data notification");
            return None;
        };
        let Some(entry) = self.registry.entries.get_mut(&key) else {
            tracing::debug!(
                key = key.get(),
                "a notification named a retired subscription"
            );
            return None;
        };

        // The manager sees the line **before** the registry acts on it, so that
        // the `UNSUB` which removes the entry is still delivered as the
        // subscription's own terminal event.
        let outcome = entry.manager.handle(notification);

        match notification {
            // "Real-time updates for the subscription start after this line"
            // [`docs/spec/04-notifications.md` §3.1].
            Notification::SubscriptionOk { .. } | Notification::SubscriptionCommandOk { .. } => {
                entry.state = SubscriptionState::Active;
                entry.was_active = true;
            }
            // "After this line no further update for the subscription will be
            // sent" [`docs/spec/04-notifications.md` §3.4], so the desired set
            // loses it too — the caller asked for that.
            Notification::Unsubscribed { .. } => {
                self.registry.entries.remove(&key);
                self.registry.by_sub_id.remove(&sub_id);
            }
            _ => {}
        }

        match outcome {
            // A notification this subscription does not care about.
            Ok(None) => None,
            Ok(Some(event)) => Some(SessionEvent::Subscription {
                progressive,
                key,
                outcome: Box::new(Ok(event)),
            }),
            Err(error) => Some(SessionEvent::Subscription {
                progressive,
                key,
                outcome: Box::new(Err(error)),
            }),
        }
    }

    /// The `LS_subId` a notification names and the caller-facing key it maps
    /// to, when it names one this session knows.
    fn resolve_subscription(&self, notification: &Notification) -> Option<(u32, SubscriptionKey)> {
        let sub_id = match notification {
            Notification::Update {
                subscription_id, ..
            }
            | Notification::SubscriptionOk {
                subscription_id, ..
            }
            | Notification::SubscriptionCommandOk {
                subscription_id, ..
            }
            | Notification::Unsubscribed { subscription_id }
            | Notification::EndOfSnapshot {
                subscription_id, ..
            }
            | Notification::ClearSnapshot {
                subscription_id, ..
            }
            | Notification::Overflow {
                subscription_id, ..
            }
            | Notification::SubscriptionReconfigured {
                subscription_id, ..
            } => *subscription_id,
            _ => return None,
        };
        let key = self.registry.by_sub_id.get(&sub_id).copied()?;
        Some((sub_id, key))
    }

    /// Takes the pending request a response names, if it still belongs to the
    /// session this driver is on.
    ///
    /// A request issued by a session that has since been replaced is retired at
    /// the boundary ([`SessionDriver::retire_stale_pending`]), so finding one
    /// here with an older generation means the response outran the retirement.
    /// It is dropped rather than acted on: §4.5 warns that these arrive, and
    /// letting one mutate the replacement session's registry is how a live
    /// subscription disappears.
    fn take_pending(&mut self, request_id: &str) -> Option<PendingKind> {
        let pending = self.pending.remove(request_id)?;
        if pending.generation == self.generation {
            return Some(pending.kind);
        }
        tracing::debug!(
            request_id,
            issued_on = pending.generation,
            current = self.generation,
            "ignoring a late response from a session that was replaced"
        );
        None
    }

    /// Correlates a `REQOK` with the request that caused it
    /// [`docs/spec/03-requests.md` §13.1].
    async fn on_request_ok(&mut self, request_id: Option<String>) {
        let Some(request_id) = request_id else {
            // The bare `REQOK` that answers an HTTP `heartbeat`, which carries
            // no `LS_reqId` [`docs/spec/03-requests.md` §14.3].
            tracing::trace!("heartbeat acknowledged");
            return;
        };

        let target = match self.take_pending(&request_id) {
            Some(kind) => {
                tracing::debug!(request_id, ?kind, "control request accepted");
                let target = kind.target();
                // The one acceptance that changes desired state. Everything
                // else the server accepts is reported by the notification that
                // follows it — `SUBOK` for an `add`, `UNSUB` for a `delete` —
                // but `reconf` has no such notification of its own, and the
                // value it granted has to survive a session that is recreated
                // and re-issues every `add` [ibid. §6.1]. Committing it here,
                // and only here, is what stops a reconnection silently putting
                // the caller's frequency ceiling back where it started.
                if let PendingKind::Reconfigure { key, max_frequency } = kind
                    && let Some(entry) = self.registry.entries.get_mut(&key)
                {
                    entry.spec.requested_max_frequency =
                        Some(as_requested_frequency(max_frequency));
                    tracing::debug!(
                        key = key.get(),
                        "frequency reconfiguration accepted and committed to the desired state"
                    );
                }
                target
            }
            // §4.5: "late responses to control requests related with the first
            // session [may] arrive interspersed with notifications for the
            // second session". Request ids are never reused, so an unknown one
            // is a stale response: it is reported, but it names nothing this
            // driver is still tracking.
            None => {
                tracing::debug!(request_id, "response to an unknown or stale request");
                ControlTarget::Session
            }
        };

        let outcome = ControlOutcome::Accepted;
        match RequestId::try_new(request_id.clone()) {
            Ok(id) => {
                self.emit(SessionEvent::ControlResponse {
                    request_id: Some(id),
                    target,
                    outcome,
                })
                .await;
            }
            Err(error) => {
                tracing::warn!(%error, "server echoed a request id this client cannot represent");
                self.emit(SessionEvent::ControlResponse {
                    request_id: None,
                    target,
                    outcome,
                })
                .await;
            }
        }
    }

    /// Correlates a `REQERR` and undoes whatever the request had optimistically
    /// recorded [`docs/spec/03-requests.md` §13.2].
    async fn on_request_error(&mut self, request_id: String, cause: ServerCause) {
        let pending = self.take_pending(&request_id);
        let target = pending
            .as_ref()
            .map_or(ControlTarget::Session, PendingKind::target);

        if let Some(kind) = pending {
            match kind {
                // The server refused the subscription. Dropping it from the
                // desired set is what stops it being re-issued on every future
                // session — and the caller is told, so nothing is lost
                // silently.
                PendingKind::Subscribe(key) => {
                    if let Some(entry) = self.registry.entries.remove(&key)
                        && let Some(sub_id) = entry.wire
                    {
                        self.registry.by_sub_id.remove(&sub_id.get());
                    }
                    tracing::warn!(
                        key = key.get(),
                        code = cause.code,
                        "subscription refused by the server"
                    );
                }
                // The delete failed, so the subscription is still there.
                PendingKind::Unsubscribe(key) => {
                    if let Some(entry) = self.registry.entries.get_mut(&key) {
                        entry.state = if entry.was_active {
                            SubscriptionState::Active
                        } else {
                            SubscriptionState::Pending
                        };
                    }
                }
                // The subscription keeps the ceiling it already had: the new
                // one is only committed on `REQOK`.
                PendingKind::Reconfigure { .. } => {}
                // The server refused the submission, so no `MSGDONE`/`MSGFAIL`
                // will follow [`docs/spec/03-requests.md` §13.2]. Reporting it
                // on the message surface as well is what keeps "every message
                // gets exactly one outcome" true.
                PendingKind::SendMessage { sequence, prog } => {
                    self.emit(SessionEvent::Message {
                        progressive: None,
                        sequence,
                        prog,
                        result: MessageResult::Refused {
                            cause: cause.clone(),
                        },
                    })
                    .await;
                }
                // No `LOOP` will follow, so the next one must not be
                // misattributed to T2.
                PendingKind::ForceRebind => self.force_rebind_outstanding = false,
                // No `END` will follow.
                PendingKind::Destroy => self.destroy_requested = false,
            }
        }

        let outcome = ControlOutcome::Rejected { cause };
        let id = RequestId::try_new(request_id).ok();
        self.emit(SessionEvent::ControlResponse {
            request_id: id,
            target,
            outcome,
        })
        .await;
    }

    // -----------------------------------------------------------------------
    // Commands
    // -----------------------------------------------------------------------

    async fn on_command(&mut self, command: SessionCommand) -> Flow {
        match command {
            SessionCommand::Subscribe { key, spec } => {
                let manager = spec.new_manager();
                self.registry.entries.insert(
                    key,
                    RegistryEntry {
                        spec: *spec,
                        wire: None,
                        state: SubscriptionState::Pending,
                        was_active: false,
                        manager,
                    },
                );
                if self.stream_state == StreamState::Bound {
                    self.issue_subscription(key).await;
                }
                Flow::Continue
            }

            SessionCommand::Unsubscribe { key } => {
                self.unsubscribe(key).await;
                Flow::Continue
            }

            SessionCommand::Reconfigure { key, max_frequency } => {
                self.reconfigure(key, max_frequency).await;
                Flow::Continue
            }

            SessionCommand::SendMessage { message } => {
                self.send_message(*message).await;
                Flow::Continue
            }

            SessionCommand::ForceRebind { close_socket } => {
                if self.stream_state != StreamState::Bound {
                    tracing::debug!("force_rebind ignored: no stream connection is bound");
                    return Flow::Continue;
                }
                let Some(session) = self.session.clone() else {
                    return Flow::Continue;
                };
                let Some(request_id) = self.next_request_id() else {
                    self.report_id_exhaustion(ControlTarget::Session).await;
                    return Flow::Continue;
                };
                // Set before sending: the `LOOP` may well arrive before the
                // `REQOK` [`docs/spec/02-session-lifecycle.md` §4.5].
                self.force_rebind_outstanding = true;
                let request = ForceRebind {
                    session: Some(session.as_str().to_owned()),
                    request_id: request_id.clone(),
                    polling_millis: None,
                    close_socket,
                };
                self.dispatch(&request, request_id, PendingKind::ForceRebind)
                    .await;
                Flow::Continue
            }

            SessionCommand::Destroy { cause } => self.destroy(cause).await,

            SessionCommand::Shutdown => Flow::Closed(SessionClosed::ByClient {
                destroy_confirmed: false,
                cause: None,
            }),
        }
    }

    /// T7 while bound, T8 while unbound
    /// [`docs/spec/02-session-lifecycle.md` §2.2, T7/T8].
    async fn destroy(&mut self, cause: Option<DestroyCause>) -> Flow {
        let Some(session) = self.session.clone() else {
            return Flow::Closed(SessionClosed::ByClient {
                destroy_confirmed: false,
                cause: None,
            });
        };
        let Some(request_id) = self.next_request_id() else {
            self.report_id_exhaustion(ControlTarget::Session).await;
            return Flow::Closed(SessionClosed::Internal {
                reason: "the request-id space is exhausted".to_owned(),
            });
        };
        self.destroy_requested = true;
        let request = DestroySession {
            session: Some(session.as_str().to_owned()),
            request_id: request_id.clone(),
            cause,
            close_socket: None,
        };
        let sent = self
            .dispatch(&request, request_id, PendingKind::Destroy)
            .await;

        if self.stream_state == StreamState::Bound && sent {
            // Stay in the pump and wait for the `END` that T7 promises, so the
            // caller learns the server's cause code.
            return Flow::Continue;
        }

        // SPEC-AMBIGUITY (A1): with no stream connection bound "there is
        // nowhere for the `END` notification… to be delivered; the spec does
        // not say whether `END` is dropped, buffered, or suppressed in this
        // case" [`docs/spec/02-session-lifecycle.md` §2.2, T8]. The driver
        // therefore does not wait for one: the request has been sent, and
        // waiting for a notification the spec does not promise would hang the
        // shutdown.
        Flow::Closed(SessionClosed::ByClient {
            destroy_confirmed: false,
            cause: None,
        })
    }

    /// Sends one message, or reports at once that it could not be sent
    /// [`docs/spec/03-requests.md` §12].
    ///
    /// There is no queue behind this. See [`MessageResult::NotSent`] for why a
    /// message is not held the way a subscription is.
    async fn send_message(&mut self, message: OutgoingMessage) {
        let (sequence, prog) = message.identity();

        // `msg` needs a bound session for the same reason every control
        // request does, and — unlike a subscription — there is nothing
        // meaningful to do with it until one exists.
        let session = match (self.stream_state, self.session.clone()) {
            (StreamState::Bound, Some(session)) => session,
            _ => {
                tracing::debug!(?sequence, ?prog, "message not sent: no session is bound");
                self.emit(SessionEvent::Message {
                    progressive: None,
                    sequence,
                    prog,
                    result: MessageResult::NotSent {
                        reason: "no session is bound to send the message on".to_owned(),
                    },
                })
                .await;
                return;
            }
        };

        let Some(request_id) = self.next_request_id() else {
            self.report_id_exhaustion(ControlTarget::Session).await;
            self.emit(SessionEvent::Message {
                progressive: None,
                sequence,
                prog,
                result: MessageResult::NotSent {
                    reason: "the request-id space is exhausted".to_owned(),
                },
            })
            .await;
            return;
        };
        let request = MessageSend {
            session: Some(session.as_str().to_owned()),
            request_id: request_id.clone(),
            message: message.message,
            sequence: message.sequence,
            msg_prog: message.msg_prog,
            max_wait_millis: message.max_wait_millis,
            // Left at the server's default of `true`: the `REQOK` is what
            // correlates the submission with this request
            // [`docs/spec/03-requests.md` §12.1].
            ack: None,
            outcome: message.outcome,
        };

        let kind = PendingKind::SendMessage {
            sequence: sequence.clone(),
            prog,
        };
        if !self.dispatch(&request, request_id, kind).await {
            // `dispatch` has already reported the transport or encoding
            // failure as a `ControlResponse`; it is repeated here on the
            // message surface so a caller watching only that surface cannot
            // lose a message silently.
            self.emit(SessionEvent::Message {
                progressive: None,
                sequence,
                prog,
                result: MessageResult::NotSent {
                    reason: "the message request could not be sent".to_owned(),
                },
            })
            .await;
        }
    }

    async fn unsubscribe(&mut self, key: SubscriptionKey) {
        let Some(entry) = self.registry.entries.get_mut(&key) else {
            tracing::debug!(key = key.get(), "unsubscribe for an unknown subscription");
            return;
        };
        let Some(sub_id) = entry.wire else {
            // Never issued on this session, so there is nothing to delete: the
            // caller simply stops wanting it.
            self.registry.entries.remove(&key);
            return;
        };
        entry.state = SubscriptionState::Removing;

        let Some(session) = self.session.clone() else {
            return;
        };
        let Some(request_id) = self.next_request_id() else {
            self.report_id_exhaustion(ControlTarget::Subscription {
                key,
                operation: SubscriptionOperation::Unsubscribe,
            })
            .await;
            return;
        };
        let request = Unsubscribe {
            session: Some(session.as_str().to_owned()),
            request_id: request_id.clone(),
            subscription_id: sub_id,
            // Left at the server's default of `true`: the `REQOK` is what
            // correlates the response to this request
            // [`docs/spec/03-requests.md` §7.1].
            ack: None,
        };
        self.dispatch(&request, request_id, PendingKind::Unsubscribe(key))
            .await;
    }

    /// Asks the server to change a subscription's frequency ceiling
    /// [`docs/spec/03-requests.md` §8].
    ///
    /// The new value is **not** written into the desired state here. Until the
    /// server has accepted the `reconf` the subscription still has its old
    /// ceiling, and a session recreated in between must re-issue what the
    /// server actually granted, not what it was last asked for. The commit
    /// happens on `REQOK` [`docs/spec/03-requests.md` §13.1], in
    /// [`SessionDriver::on_request_ok`].
    async fn reconfigure(&mut self, key: SubscriptionKey, max_frequency: MaxFrequencyLimit) {
        let Some(entry) = self.registry.entries.get(&key) else {
            return;
        };
        let Some(sub_id) = entry.wire else {
            return;
        };
        let Some(session) = self.session.clone() else {
            return;
        };
        let Some(request_id) = self.next_request_id() else {
            self.report_id_exhaustion(ControlTarget::Subscription {
                key,
                operation: SubscriptionOperation::Reconfigure,
            })
            .await;
            return;
        };
        let request = ReconfigureSubscription {
            session: Some(session.as_str().to_owned()),
            request_id: request_id.clone(),
            subscription_id: sub_id,
            requested_max_frequency: Some(max_frequency.clone()),
        };
        self.dispatch(
            &request,
            request_id,
            PendingKind::Reconfigure { key, max_frequency },
        )
        .await;
    }

    // -----------------------------------------------------------------------
    // Subscriptions
    // -----------------------------------------------------------------------

    /// Issues every subscription that has no binding in the current session.
    ///
    /// After a rebind or a recovery that is nothing, because the server
    /// preserved them all [`docs/spec/02-session-lifecycle.md` §4.4]. After a
    /// session replacement it is every one of them, which is precisely the
    /// obligation of §6.1: "re-execute all the subscriptions that were active
    /// at the moment the previous stream connection terminated".
    async fn issue_unbound_subscriptions(&mut self) {
        let keys: Vec<SubscriptionKey> = self
            .registry
            .entries
            .iter()
            .filter(|(_, entry)| entry.wire.is_none() && entry.state != SubscriptionState::Removing)
            .map(|(key, _)| *key)
            .collect();
        if keys.is_empty() {
            return;
        }

        let mut issued = Vec::with_capacity(keys.len());
        for key in keys {
            if let Some(entry) = self.issue_subscription(key).await {
                issued.push(entry);
            }
        }
        if !issued.is_empty() {
            tracing::info!(count = issued.len(), "subscriptions re-established");
            self.emit(SessionEvent::Resubscribed(issued)).await;
        }
    }

    /// Sends one `add` for a subscription that has no binding yet.
    async fn issue_subscription(&mut self, key: SubscriptionKey) -> Option<ResubscribedEntry> {
        let session = self.session.clone()?;
        let sub_id = self.registry.allocate()?;
        let entry = self.registry.entries.get_mut(&key)?;
        entry.wire = Some(sub_id);
        entry.state = SubscriptionState::Pending;
        let spec = entry.spec.clone();
        let previously_active = entry.was_active;
        self.registry.by_sub_id.insert(sub_id.get(), key);

        let Some(request_id) = self.next_request_id() else {
            if let Some(entry) = self.registry.entries.get_mut(&key) {
                entry.wire = None;
            }
            self.registry.by_sub_id.remove(&sub_id.get());
            self.report_id_exhaustion(ControlTarget::Subscription {
                key,
                operation: SubscriptionOperation::Subscribe,
            })
            .await;
            return None;
        };
        let request = Subscribe {
            session: Some(session.as_str().to_owned()),
            request_id: request_id.clone(),
            subscription_id: sub_id,
            group: spec.group.clone(),
            schema: spec.schema.clone(),
            mode: spec.mode,
            data_adapter: spec.data_adapter.clone(),
            selector: spec.selector.clone(),
            requested_buffer_size: spec.requested_buffer_size,
            requested_max_frequency: spec.requested_max_frequency.clone(),
            snapshot: spec.snapshot,
            // Left at the server's default of `true`: `REQOK` is what
            // correlates the response to this request
            // [`docs/spec/03-requests.md` §6.1].
            ack: None,
        };
        if !self
            .dispatch(&request, request_id, PendingKind::Subscribe(key))
            .await
        {
            // The `add` never reached the server, so the binding recorded a
            // moment ago is a fiction: the subscription would sit in the
            // registry looking issued, be skipped by every later
            // `issue_unbound_subscriptions`, and never be established again —
            // a subscription lost in silence. Releasing the binding puts it
            // back in the set that is re-issued at the next bind.
            //
            // The `LS_subId` itself is **not** returned to the pool: ids
            // "must not be reused" within a session
            // [`docs/spec/02-session-lifecycle.md` §4.4], so the next attempt
            // allocates a fresh one.
            if let Some(entry) = self.registry.entries.get_mut(&key) {
                entry.wire = None;
            }
            self.registry.by_sub_id.remove(&sub_id.get());
            tracing::warn!(
                key = key.get(),
                "subscription could not be issued; it will be retried on the next bind"
            );
            return None;
        }

        Some(ResubscribedEntry {
            key,
            subscription_id: sub_id,
            previously_active,
            snapshot_requested: spec.wants_snapshot(),
        })
    }

    // -----------------------------------------------------------------------
    // Sending
    // -----------------------------------------------------------------------

    /// The next `LS_reqId`, or `None` once the space is exhausted.
    ///
    /// See the note on the [`SessionDriver::next_request_id`] field for why it
    /// never resets. It also never wraps: an id that came back round would
    /// collide with a request whose late response §4.5 says may still be in
    /// flight, and correlating a response to the wrong request is worse than
    /// refusing to send a new one. Exhausting a 64-bit space takes longer than
    /// any session lives, so the refusal is a proof obligation rather than an
    /// operating condition.
    fn next_request_id(&mut self) -> Option<RequestId> {
        let next = self.next_request_id.checked_add(1)?;
        let id = RequestId::from_progressive(self.next_request_id);
        self.next_request_id = next;
        Some(id)
    }

    /// Reports that a request could not be given an id, on the surface that
    /// request would have been answered on.
    async fn report_id_exhaustion(&mut self, target: ControlTarget) {
        tracing::error!(
            "the request-id space is exhausted; no further control request can be sent"
        );
        self.emit(SessionEvent::ControlResponse {
            request_id: None,
            target,
            outcome: ControlOutcome::NotSent {
                reason: "the request-id space is exhausted".to_owned(),
            },
        })
        .await;
    }

    /// Which encoding rules apply to a control request on this transport.
    ///
    /// A declared property, not a transport identity: `control_shares_stream`
    /// is exactly the WebSocket case, where `LS_ack` is meaningful and
    /// `LS_session` may be defaulted [`docs/spec/03-requests.md` §5.1;
    /// `docs/adr/0007-transport-port-shape.md`].
    #[must_use]
    const fn control_kind(&self) -> TransportKind {
        if self.properties.control_shares_stream {
            TransportKind::WebSocket
        } else {
            TransportKind::Http
        }
    }

    /// Encodes and sends a control request, recording it for correlation.
    ///
    /// Returns whether it reached the transport. A failure is surfaced to the
    /// caller as [`ControlOutcome::NotSent`] rather than swallowed.
    async fn dispatch<R: TlcpRequest>(
        &mut self,
        request: &R,
        request_id: RequestId,
        kind: PendingKind,
    ) -> bool {
        let target = kind.target();
        let parameters = match request.encode_parameters(self.control_kind()) {
            Ok(parameters) => parameters,
            Err(error) => {
                tracing::error!(%error, "cannot encode control request");
                self.emit(SessionEvent::ControlResponse {
                    request_id: Some(request_id),
                    target,
                    outcome: ControlOutcome::NotSent {
                        reason: error.to_string(),
                    },
                })
                .await;
                return false;
            }
        };

        self.pending.insert(
            request_id.as_str().to_owned(),
            Pending {
                kind,
                generation: self.generation,
            },
        );
        let encoded = EncodedRequest {
            name: R::NAME,
            path: R::PATH,
            parameters,
        };
        match self.transport.send_control(encoded).await {
            Ok(()) => {
                // "any Control Request has the same effect of a Heartbeat"
                // [`docs/spec/02-session-lifecycle.md` §8.4].
                self.liveness.on_outbound(Instant::now());
                true
            }
            Err(error) => {
                self.pending.remove(request_id.as_str());
                tracing::warn!(%error, "control request could not be sent");
                self.emit(SessionEvent::ControlResponse {
                    request_id: Some(request_id),
                    target,
                    outcome: ControlOutcome::NotSent {
                        reason: error.to_string(),
                    },
                })
                .await;
                false
            }
        }
    }

    /// Sends the `heartbeat` pseudo-request that keeps the client's
    /// `LS_inactivity_millis` commitment
    /// [`docs/spec/02-session-lifecycle.md` §8.4].
    /// Sends the `heartbeat` pseudo-request, or gives the connection up.
    ///
    /// A failure here is not merely logged. The outbound deadline is already
    /// past when this runs, so leaving it where it is would make the timer fire
    /// again on the very next turn of the pump — a spin that burns a core and
    /// floods the log without ever satisfying the commitment. The deadline is
    /// therefore advanced whatever happens, which spaces the retries one
    /// interval apart, and after [`MAX_HEARTBEAT_FAILURES`] consecutive
    /// failures the connection is treated as lost: a client that cannot write
    /// to a socket cannot keep the `LS_inactivity_millis` commitment on it
    /// [`docs/spec/02-session-lifecycle.md` §8.4], and the server will drop it
    /// regardless.
    async fn send_heartbeat(&mut self) -> Flow {
        let session = self.session.as_ref().map(|s| s.as_str().to_owned());
        // `LS_session` "is only needed for the purpose 3 above, whereby the
        // client declares that it is still listening to the stream connection
        // for the specified session" — which is the purpose of every heartbeat
        // this driver sends [`docs/spec/02-session-lifecycle.md` §8.4].
        let request = Heartbeat { session };
        let failure = match request.encode_parameters(self.control_kind()) {
            Ok(parameters) => {
                let encoded = EncodedRequest {
                    name: Heartbeat::NAME,
                    path: Heartbeat::PATH,
                    parameters,
                };
                match self.transport.send_control(encoded).await {
                    Ok(()) => None,
                    Err(error) => Some(error.to_string()),
                }
            }
            Err(error) => Some(error.to_string()),
        };

        // Either way the clock moves: a heartbeat that failed is not retried
        // sooner than one that succeeded.
        self.liveness.on_outbound(Instant::now());

        let Some(detail) = failure else {
            self.heartbeat_failures = 0;
            return Flow::Continue;
        };
        self.heartbeat_failures = self.heartbeat_failures.saturating_add(1);
        tracing::warn!(
            detail,
            attempts = self.heartbeat_failures,
            "heartbeat could not be sent"
        );
        if self.heartbeat_failures >= MAX_HEARTBEAT_FAILURES {
            self.heartbeat_failures = 0;
            return Flow::Unbind(UnbindReason::ConnectionFailed {
                detail: format!(
                    "the outbound heartbeat failed {MAX_HEARTBEAT_FAILURES} times in a row: {detail}"
                ),
            });
        }
        Flow::Continue
    }

    /// Publishes an event, applying backpressure when the caller is slow.
    ///
    /// Nothing is dropped and nothing is reordered: the driver waits for room
    /// rather than discarding, because a discarded data notification would
    /// desynchronise the recovery progressive that makes recovery correct
    /// [`docs/spec/02-session-lifecycle.md` §5.2].
    ///
    /// The one thing that can end the wait early is the stop lane. A caller
    /// that has stopped reading is entitled to stall the session — that is the
    /// documented contract — but not to make it unstoppable, so an ordered stop
    /// abandons the undelivered event and marks the driver as unwinding. The
    /// stream it was going to is about to end anyway.
    ///
    /// A closed receiver is not an error either: the caller may legitimately
    /// stop listening, and the driver still has a session to shut down in
    /// order.
    async fn emit(&mut self, event: SessionEvent) {
        let events = &self.events;
        let stop = &mut self.stop;
        let interrupted = tokio::select! {
            biased;
            permit = events.reserve() => match permit {
                Ok(permit) => {
                    permit.send(event);
                    false
                }
                Err(_) => {
                    tracing::debug!("event receiver dropped; events are no longer delivered");
                    false
                }
            },
            () = stopped(stop) => true,
        };
        if interrupted {
            tracing::debug!(
                "a stop was ordered while the event stream was full; the event was not delivered"
            );
            self.stopping = true;
        }
    }
}

/// Sleeps until an instant, or forever when there is none.
async fn sleep_until_option(deadline: Option<Instant>) {
    match deadline {
        Some(instant) => tokio::time::sleep_until(instant).await,
        None => std::future::pending().await,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use std::collections::VecDeque;
    use std::sync::Mutex;

    use super::*;
    use crate::protocol::request::ConnectionMode;
    use crate::session::backoff::BackoffPolicy;
    use crate::session::options::{Credentials, SessionOptions};

    // -----------------------------------------------------------------------
    // A transport that replays a script
    // -----------------------------------------------------------------------
    //
    // This is the whole point of `docs/adr/0007-transport-port-shape.md`: the
    // port yields one ordered line stream, so the state machine can be driven
    // through every transition with no socket, no network and no real timer.
    // Several scripts below are the verbatim transcripts of
    // `docs/spec/02-session-lifecycle.md` §10.

    /// One thing the mock does when the driver asks for a line.
    #[derive(Debug, Clone, PartialEq, Eq)]
    enum Step {
        /// Yield a line.
        Line(String),
        /// Yield a transport failure — transition T5.
        Fail(&'static str),
        /// Yield `None`: the stream ended cleanly.
        End,
        /// Never yield anything again. Models a wedged connection, which is
        /// what the keepalive budget exists to detect.
        Silence,
        /// Yield nothing until at least this many control requests have been
        /// sent. Lets a script react to what the driver does.
        AwaitControls(usize),
        /// Yield this line, always immediately ready, until at least this many
        /// control requests have been sent. Models the busy server the pump's
        /// fairness bound exists for: the line stream is never *not* ready, so
        /// a purely biased pump would never look at anything else.
        FloodUntilControls(&'static str, usize),
    }

    fn line(text: &str) -> Step {
        Step::Line(text.to_owned())
    }

    /// Lines split from a transcript block, blank lines and elisions removed.
    fn transcript(text: &str) -> Vec<Step> {
        text.lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && *l != "[…]")
            .map(line)
            .collect()
    }

    /// What the driver asked the transport to open.
    #[derive(Debug, Clone, PartialEq, Eq)]
    enum OpenRecord {
        Create,
        Bind {
            session: Option<String>,
            recovery_from: Option<u64>,
        },
    }

    #[derive(Debug, Default)]
    struct MockLog {
        opens: Vec<OpenRecord>,
        controls: Vec<EncodedRequest>,
        control_links: Vec<Option<String>>,
        closes: usize,
    }

    impl MockLog {
        fn control_parameters(&self) -> Vec<String> {
            self.controls
                .iter()
                .map(|request| request.parameters.clone())
                .collect()
        }
    }

    struct MockTransport {
        properties: TransportProperties,
        scripts: VecDeque<Vec<Step>>,
        current: VecDeque<Step>,
        /// How many of the next `send_control` calls fail. Models a control
        /// request that never reaches the server.
        control_failures: usize,
        /// Whether `open_stream` never completes. Models a dial that hangs
        /// below the port — in DNS, in TCP, in TLS, or in the handshake.
        hangs_on_open: bool,
        log: Arc<Mutex<MockLog>>,
    }

    impl MockTransport {
        fn new(properties: TransportProperties, scripts: Vec<Vec<Step>>) -> Self {
            Self {
                properties,
                scripts: scripts.into(),
                current: VecDeque::new(),
                control_failures: 0,
                hangs_on_open: false,
                log: Arc::new(Mutex::new(MockLog::default())),
            }
        }

        fn failing_controls(mut self, count: usize) -> Self {
            self.control_failures = count;
            self
        }

        const fn hanging_open(mut self) -> Self {
            self.hangs_on_open = true;
            self
        }
    }

    impl Transport for MockTransport {
        fn properties(&self) -> TransportProperties {
            self.properties
        }

        fn set_control_link(&mut self, host: Option<&str>) {
            self.log
                .lock()
                .expect("mock log poisoned")
                .control_links
                .push(host.map(str::to_owned));
        }

        async fn open_stream(&mut self, request: StreamOpen) -> Result<(), TransportError> {
            let record = match request {
                StreamOpen::Create(_) => OpenRecord::Create,
                StreamOpen::Bind(bind) => OpenRecord::Bind {
                    session: bind.session.clone(),
                    recovery_from: bind.recovery_from,
                },
            };
            self.log
                .lock()
                .expect("mock log poisoned")
                .opens
                .push(record);
            if self.hangs_on_open {
                return std::future::pending().await;
            }
            self.current = self.scripts.pop_front().unwrap_or_default().into();
            Ok(())
        }

        async fn next_line(&mut self) -> Option<Result<String, TransportError>> {
            loop {
                let Some(step) = self.current.front().cloned() else {
                    return std::future::pending().await;
                };
                match step {
                    Step::Line(text) => {
                        self.current.pop_front();
                        return Some(Ok(text));
                    }
                    Step::Fail(reason) => {
                        self.current.pop_front();
                        return Some(Err(TransportError::ConnectionLost {
                            reason: reason.to_owned(),
                        }));
                    }
                    Step::End => {
                        self.current.pop_front();
                        return None;
                    }
                    Step::Silence => return std::future::pending().await,
                    Step::FloodUntilControls(text, wanted) => {
                        let sent = self.log.lock().expect("mock log poisoned").controls.len();
                        if sent >= wanted {
                            self.current.pop_front();
                            continue;
                        }
                        return Some(Ok(text.to_owned()));
                    }
                    Step::AwaitControls(wanted) => {
                        let sent = self.log.lock().expect("mock log poisoned").controls.len();
                        if sent >= wanted {
                            self.current.pop_front();
                            continue;
                        }
                        return std::future::pending().await;
                    }
                }
            }
        }

        async fn send_control(&mut self, request: EncodedRequest) -> Result<(), TransportError> {
            let name = request.name;
            // The attempt is logged either way, so an `AwaitControls` gate
            // still fires for a request that failed to go out.
            self.log
                .lock()
                .expect("mock log poisoned")
                .controls
                .push(request);
            if self.control_failures > 0
                && let Some(remaining) = self.control_failures.checked_sub(1)
            {
                self.control_failures = remaining;
                return Err(TransportError::Send {
                    name,
                    reason: "control connection refused".to_owned(),
                });
            }
            Ok(())
        }

        async fn close(&mut self) -> Result<(), TransportError> {
            self.log.lock().expect("mock log poisoned").closes += 1;
            Ok(())
        }
    }

    // -----------------------------------------------------------------------
    // Harness
    // -----------------------------------------------------------------------

    /// WebSocket-shaped properties: control travels on the stream connection,
    /// the stream has no content-length end, and it does not poll
    /// (`docs/adr/0007-transport-port-shape.md`).
    const fn websocket() -> TransportProperties {
        TransportProperties {
            control_shares_stream: true,
            ends_on_content_length: false,
            is_polling: false,
        }
    }

    /// HTTP-streaming-shaped properties: control needs its own connection and
    /// the stream ends when its content length is reached — transition T3.
    const fn http_streaming() -> TransportProperties {
        TransportProperties {
            control_shares_stream: false,
            ends_on_content_length: true,
            is_polling: false,
        }
    }

    /// HTTP long-polling properties — transition T4.
    const fn http_polling() -> TransportProperties {
        TransportProperties {
            control_shares_stream: false,
            ends_on_content_length: false,
            is_polling: true,
        }
    }

    fn options() -> SessionOptions {
        SessionOptions::default()
            .with_credentials(Credentials {
                user: None,
                password: None,
                adapter_set: Some("WELCOME".to_owned()),
            })
            .with_backoff(BackoffPolicy {
                initial: Duration::from_millis(10),
                max: Duration::from_millis(40),
                max_attempts: NonZeroU32::new(4),
            })
    }

    struct Outcome {
        closed: SessionClosed,
        events: Vec<SessionEvent>,
        log: Arc<Mutex<MockLog>>,
    }

    impl Outcome {
        fn opens(&self) -> Vec<OpenRecord> {
            self.log.lock().expect("mock log poisoned").opens.clone()
        }

        fn controls(&self) -> Vec<String> {
            self.log
                .lock()
                .expect("mock log poisoned")
                .control_parameters()
        }

        fn closes(&self) -> usize {
            self.log.lock().expect("mock log poisoned").closes
        }

        fn bound(&self) -> Vec<&BoundInfo> {
            self.events
                .iter()
                .filter_map(|event| match event {
                    SessionEvent::Bound(info) => Some(info.as_ref()),
                    _ => None,
                })
                .collect()
        }

        fn unbinds(&self) -> Vec<&UnbindReason> {
            self.events
                .iter()
                .filter_map(|event| match event {
                    SessionEvent::Unbound { reason, .. } => Some(reason),
                    _ => None,
                })
                .collect()
        }

        /// Every subscription event, with the progressive that carried it.
        fn data(&self) -> Vec<(u64, SubscriptionKey, &SubscriptionOutcome)> {
            self.events
                .iter()
                .filter_map(|event| match event {
                    SessionEvent::Subscription {
                        progressive,
                        key,
                        outcome,
                    } => Some((*progressive, *key, outcome.as_ref())),
                    _ => None,
                })
                .collect()
        }

        fn recoveries(&self) -> Vec<RecoveryOutcome> {
            self.events
                .iter()
                .filter_map(|event| match event {
                    SessionEvent::Recovered(outcome) => Some(*outcome),
                    _ => None,
                })
                .collect()
        }

        fn resubscribes(&self) -> Vec<&Vec<ResubscribedEntry>> {
            self.events
                .iter()
                .filter_map(|event| match event {
                    SessionEvent::Resubscribed(entries) => Some(entries),
                    _ => None,
                })
                .collect()
        }
    }

    /// Runs a driver over a mock transport to its terminal state, with every
    /// command pre-queued before the first line is read.
    async fn drive(
        properties: TransportProperties,
        options: SessionOptions,
        scripts: Vec<Vec<Step>>,
        commands: Vec<SessionCommand>,
    ) -> Outcome {
        drive_transport(MockTransport::new(properties, scripts), options, commands).await
    }

    /// As [`drive`], but over a transport the test built itself.
    async fn drive_transport(
        transport: MockTransport,
        options: SessionOptions,
        commands: Vec<SessionCommand>,
    ) -> Outcome {
        let log = Arc::clone(&transport.log);
        let (driver, handle, mut events) =
            connect(transport, options).expect("options must match the transport");
        for command in commands {
            handle
                .commands
                .send(command)
                .await
                .expect("the driver has not started yet");
        }
        let closed = driver.run().await;
        drop(handle);

        let mut collected = Vec::new();
        while let Ok(event) = events.try_recv() {
            collected.push(event);
        }
        Outcome {
            closed,
            events: collected,
            log,
        }
    }

    /// The `END` this suite uses to stop a driver whose scenario does not end
    /// on its own: "The session was closed on the Server side, via software or
    /// by the administrator" [`docs/spec/05-error-codes.md` §2, code 32].
    const SERVER_END: &str = "END,32,session closed on the Server side";

    /// The `CONOK` of the session-creation transcript
    /// [`docs/spec/02-session-lifecycle.md` §10, F1].
    const F1_CONOK: &str = "CONOK,S1d7c802482843a26T5626355,50000,5000,*";
    const F1_SESSION: &str = "S1d7c802482843a26T5626355";

    /// The `CONOK` of the rebinding transcripts
    /// [`docs/spec/02-session-lifecycle.md` §10, F6/F7].
    const F6_CONOK: &str = "CONOK,Se939a67a9be2d336T3823582,50000,5000,*";
    const F6_SESSION: &str = "Se939a67a9be2d336T3823582";

    /// The `CONOK` of the recovery transcript
    /// [`docs/spec/02-session-lifecycle.md` §10, F8].
    const F8_CONOK: &str = "CONOK,S22dee113e3f71b1fT4327493,50000,5000,*";
    const F8_SESSION: &str = "S22dee113e3f71b1fT4327493";

    /// The `add` that the transcripts' `SUBOK,1,…` answers.
    ///
    /// The published transcripts omit the control request that created the
    /// subscription; a driver that never received one has nothing to route a
    /// `U` to, so the suite issues it.
    fn subscribe() -> SessionCommand {
        SessionCommand::Subscribe {
            key: SubscriptionKey(1),
            spec: Box::new(spec()),
        }
    }

    /// A transcript with a wait for the `add` inserted after its `CONOK`, so
    /// the subscription is bound to `LS_subId` 1 before the server answers it.
    fn subscribed_transcript(text: &str) -> Vec<Step> {
        let mut steps = transcript(text);
        steps.insert(1, Step::AwaitControls(1));
        steps
    }

    fn spec() -> SubscriptionSpec {
        // The Hands On subscription: `LS_group=item2`, the three-field stock
        // schema, `MERGE` [`docs/spec/02-session-lifecycle.md` §10, F8].
        let mut spec = SubscriptionSpec::new(
            "item2",
            "stock_name time last_price",
            SubscriptionMode::Merge,
        );
        spec.data_adapter = Some("STOCKS".to_owned());
        spec
    }

    // -----------------------------------------------------------------------
    // T1 — initial → Bound, `Session Created`
    // -----------------------------------------------------------------------

    #[tokio::test(start_paused = true)]
    async fn test_t1_create_session_binds_and_reports_a_new_session() {
        // The whole of fixture F1, verbatim
        // [`docs/spec/02-session-lifecycle.md` §10, F1].
        let mut script = transcript(
            "
            CONOK,S1d7c802482843a26T5626355,50000,5000,*
            SERVNAME,Lightstreamer HTTP Server
            CLIENTIP,0:0:0:0:0:0:0:1
            NOOP,sending placeholder data
            CONS,unlimited
            PROBE
            PROBE
            PROBE
            ",
        );
        script.push(line(SERVER_END));

        let outcome = drive(websocket(), options(), vec![script], Vec::new()).await;

        let bound = outcome.bound();
        assert_eq!(bound.len(), 1);
        let info = bound.first().expect("one bind");
        // T1: "Record session ID, request limit, keep-alive time and control
        // link" [`docs/spec/02-session-lifecycle.md` §2.2, T1].
        assert_eq!(info.session_id.as_str(), F1_SESSION);
        assert_eq!(info.request_limit_bytes, 50_000);
        assert_eq!(info.keep_alive, Duration::from_millis(5000));
        // The literal `*` means "no control link configured" [§3.1].
        assert_eq!(info.control_link, None);
        assert_eq!(info.kind, BindKind::Created);
        assert_eq!(outcome.opens(), vec![OpenRecord::Create]);
        // The transport is closed on the way out, always.
        assert_eq!(outcome.closes(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn test_t1_head_notifications_are_surfaced_but_not_counted() {
        // `SERVNAME`, `CLIENTIP` and `CONS` are session-related, not data
        // notifications [`docs/spec/02-session-lifecycle.md` §5.3]; `NOOP` and
        // `PROBE` carry nothing at all.
        let mut script = transcript(
            "
            CONOK,S1d7c802482843a26T5626355,50000,5000,*
            SERVNAME,Lightstreamer HTTP Server
            CLIENTIP,0:0:0:0:0:0:0:1
            NOOP,sending placeholder data
            CONS,unlimited
            PROBE
            ",
        );
        script.push(line(SERVER_END));

        let outcome = drive(websocket(), options(), vec![script], Vec::new()).await;

        let infos = outcome
            .events
            .iter()
            .filter(|event| matches!(event, SessionEvent::ServerInfo(_)))
            .count();
        assert_eq!(infos, 3, "SERVNAME, CLIENTIP and CONS reach the caller");
        assert!(
            outcome.data().is_empty(),
            "none of them is a data notification"
        );
    }

    // -----------------------------------------------------------------------
    // T2 — Bound → Unbound, `Force-Unbound by Client`
    // -----------------------------------------------------------------------

    #[tokio::test(start_paused = true)]
    async fn test_t2_force_rebind_unbinds_and_rebinds_the_same_session() {
        let first = vec![
            line(F6_CONOK),
            // Wait for the `force_rebind` control request, then answer as T2
            // says the server does: "If successful, a `LOOP` notification is
            // sent on the stream connection"
            // [`docs/spec/02-session-lifecycle.md` §2.2, T2].
            Step::AwaitControls(1),
            line("REQOK,1"),
            line("LOOP,0"),
            Step::End,
        ];
        let second = vec![line(F6_CONOK), line(SERVER_END)];

        let outcome = drive(
            websocket(),
            options(),
            vec![first, second],
            vec![SessionCommand::ForceRebind {
                close_socket: Some(true),
            }],
        )
        .await;

        assert_eq!(
            outcome.unbinds(),
            vec![&UnbindReason::ForcedByClient {
                expected_delay: Duration::ZERO
            }]
        );
        let controls = outcome.controls();
        assert!(
            controls
                .iter()
                .any(|parameters| parameters.contains("LS_op=force_rebind")
                    && parameters.contains("LS_close_socket=true")),
            "{controls:?}"
        );
        // T6: the rebind is a plain `bind_session`, with no recovery, because
        // a `LOOP` is a clean unbind [§4.4].
        assert_eq!(
            outcome.opens().get(1),
            Some(&OpenRecord::Bind {
                session: Some(F6_SESSION.to_owned()),
                recovery_from: None,
            })
        );
    }

    // -----------------------------------------------------------------------
    // T3 — Bound → Unbound, `Content-Length Reached`
    // -----------------------------------------------------------------------

    #[tokio::test(start_paused = true)]
    async fn test_t3_content_length_reached_rebinds_on_a_new_connection() {
        // The tail of F6 and the whole of F7, verbatim
        // [`docs/spec/02-session-lifecycle.md` §10, F6, F7].
        let mut first = transcript(
            "
            CONOK,Se939a67a9be2d336T3823582,50000,5000,*
            SUBOK,1,1,3
            U,1,1,stock|15:57:51|16.27
            U,1,1,||16.24
            U,1,1,||16.11
            U,1,1,|15:57:52|15.99
            U,1,1,||15.93
            LOOP,0
            ",
        );
        first.insert(1, Step::AwaitControls(1));
        first.push(Step::End);

        let mut second = transcript(
            "
            CONOK,Se939a67a9be2d336T3823582,50000,5000,*
            NOOP,sending placeholder data
            CONS,unlimited
            U,1,1,|15:57:53|17.7
            U,1,1,||17.81
            ",
        );
        second.push(line(SERVER_END));

        let outcome = drive(
            http_streaming(),
            options(),
            vec![first, second],
            vec![subscribe()],
        )
        .await;

        assert_eq!(
            outcome.unbinds(),
            vec![&UnbindReason::ContentLengthReached {
                expected_delay: Duration::ZERO
            }]
        );
        // F7's three test-relevant observations: the session id is identical,
        // the rebind carries no recovery, and nothing is re-subscribed.
        let bound = outcome.bound();
        assert_eq!(bound.len(), 2);
        assert_eq!(
            bound.first().map(|info| info.session_id.clone()),
            bound.get(1).map(|info| info.session_id.clone())
        );
        assert_eq!(
            bound.get(1).map(|info| info.kind.clone()),
            Some(BindKind::Rebound)
        );
        assert!(outcome.resubscribes().is_empty());
        // The progressive keeps counting across the rebind: `SUBOK` and five
        // `U` before, two `U` after.
        assert_eq!(
            outcome
                .data()
                .iter()
                .map(|(p, _, _)| *p)
                .collect::<Vec<_>>(),
            vec![1, 2, 3, 4, 5, 6, 7, 8]
        );
    }

    // -----------------------------------------------------------------------
    // T4 — Bound → Unbound, `Poll Cycle Expired`
    // -----------------------------------------------------------------------

    #[tokio::test(start_paused = true)]
    async fn test_t4_poll_cycle_expired_rebinds_after_the_expected_delay() {
        // "A value greater than 0 is actually used only on polling sessions"
        // [`docs/spec/02-session-lifecycle.md` §4.2].
        let first = vec![line(F1_CONOK), line("LOOP,2000"), Step::End];
        let second = vec![line(F1_CONOK), line(SERVER_END)];

        let polling = options().with_connection(ConnectionMode::Polling {
            polling_millis: 2000,
            idle_millis: Some(10_000),
        });
        let outcome = drive(http_polling(), polling, vec![first, second], Vec::new()).await;

        assert_eq!(
            outcome.unbinds(),
            vec![&UnbindReason::PollCycleExpired {
                expected_delay: Duration::from_millis(2000)
            }]
        );
        // The delay `LOOP` asked for is the delay the driver waits, not a
        // backoff [§4.2].
        let retry_in = outcome.events.iter().find_map(|event| match event {
            SessionEvent::Unbound { retry_in, .. } => *retry_in,
            _ => None,
        });
        assert_eq!(retry_in, Some(Duration::from_millis(2000)));
    }

    #[tokio::test(start_paused = true)]
    async fn test_t4_polling_stream_ending_without_a_loop_is_still_a_poll_cycle() {
        // SPEC-AMBIGUITY exercised: §7.1 describes the cycle without naming
        // `LOOP`, so a clean end on a polling transport is read as T4.
        let first = vec![line(F1_CONOK), Step::End];
        let second = vec![line(F1_CONOK), line(SERVER_END)];

        let polling = options().with_connection(ConnectionMode::Polling {
            polling_millis: 1000,
            idle_millis: None,
        });
        let outcome = drive(http_polling(), polling, vec![first, second], Vec::new()).await;

        assert_eq!(
            outcome.unbinds(),
            vec![&UnbindReason::PollCycleExpired {
                expected_delay: Duration::ZERO
            }]
        );
        // A poll cycle is not a failure, so the rebind carries no recovery.
        assert_eq!(
            outcome.opens().get(1),
            Some(&OpenRecord::Bind {
                session: Some(F1_SESSION.to_owned()),
                recovery_from: None,
            })
        );
    }

    #[tokio::test(start_paused = true)]
    async fn test_t3_and_t4_are_told_apart_by_declared_properties_only() {
        // The same `LOOP,0` on two transports whose only difference is a
        // declared property (`docs/adr/0007-transport-port-shape.md`).
        let script = || {
            vec![
                vec![line(F1_CONOK), line("LOOP,0"), Step::End],
                vec![line(F1_CONOK), line(SERVER_END)],
            ]
        };

        let a = drive(http_streaming(), options(), script(), Vec::new()).await;
        assert!(matches!(
            a.unbinds().first(),
            Some(UnbindReason::ContentLengthReached { .. })
        ));

        let b = drive(websocket(), options(), script(), Vec::new()).await;
        assert!(matches!(
            b.unbinds().first(),
            Some(UnbindReason::Looped { .. })
        ));
    }

    // -----------------------------------------------------------------------
    // T5 — Bound → Unbound, `Connection Failed`
    // -----------------------------------------------------------------------

    #[tokio::test(start_paused = true)]
    async fn test_t5_connection_failed_recovers_from_the_counted_progressive() {
        // The stream of F8 up to the interruption, verbatim, whose data
        // notifications the spec counts as 15
        // [`docs/spec/02-session-lifecycle.md` §10, F8].
        let mut first = subscribed_transcript(F8_STREAM);
        first.push(Step::Fail("connection reset"));

        let second = vec![line(F8_CONOK), line("PROG,15"), line(SERVER_END)];

        let outcome = drive(
            websocket(),
            options(),
            vec![first, second],
            vec![subscribe()],
        )
        .await;

        assert!(matches!(
            outcome.unbinds().first(),
            Some(UnbindReason::ConnectionFailed { .. })
        ));
        // T5's client action: "Attempt **recovery** (`bind_session` +
        // `LS_recovery_from`), not a plain rebind"
        // [`docs/spec/02-session-lifecycle.md` §2.2, T5].
        assert_eq!(
            outcome.opens().get(1),
            Some(&OpenRecord::Bind {
                session: Some(F8_SESSION.to_owned()),
                recovery_from: Some(15),
            })
        );
        assert_eq!(
            outcome.recoveries(),
            vec![RecoveryOutcome {
                requested: 15,
                resumed_at: 15,
                kind: RecoveryKind::Exact,
            }]
        );
        assert_eq!(
            outcome.bound().get(1).map(|info| info.kind.clone()),
            Some(BindKind::Recovering {
                requested_progressive: 15
            })
        );
    }

    /// The stream of F8 up to the manual interruption, verbatim
    /// [`docs/spec/02-session-lifecycle.md` §10, F8, p.77].
    const F8_STREAM: &str = "
        CONOK,S22dee113e3f71b1fT4327493,50000,5000,*
        SERVNAME,Lightstreamer HTTP Server
        CLIENTIP,0:0:0:0:0:0:0:1
        NOOP,sending placeholder data
        CONS,unlimited
        PROBE
        SUBOK,1,1,3
        CONF,1,unlimited,filtered
        PROBE
        U,1,1,Ations Europe|15:55:08|14.81
        U,1,1,||14.66
        U,1,1,|15:55:09|14.62
        U,1,1,||14.71
        U,1,1,|15:55:10|14.63
        U,1,1,||14.77
        U,1,1,|15:55:11|
        U,1,1,||14.61
        U,1,1,|15:55:12|14.5
        U,1,1,|15:55:13|14.64
        U,1,1,|15:55:14|
        SYNC,25
        U,1,1,||14.74
        U,1,1,||14.66
        ";

    #[tokio::test(start_paused = true)]
    async fn test_data_notification_count_matches_the_spec_worked_example() {
        // "The count should start with the `SUBOK` notification and include the
        // `CONF` and all the `U` notifications. In our example, the count
        // yields 15." [`docs/spec/02-session-lifecycle.md` §5.3, §10 F8].
        let mut script = subscribed_transcript(F8_STREAM);
        script.push(line(SERVER_END));

        let outcome = drive(websocket(), options(), vec![script], vec![subscribe()]).await;

        let data = outcome.data();
        assert_eq!(data.len(), 15, "1 SUBOK + 1 CONF + 13 U");
        assert_eq!(data.last().map(|(p, _, _)| *p), Some(15));
        assert!(
            data.iter().all(|(_, _, outcome)| outcome.is_ok()),
            "PROBE and SYNC are not data notifications, and every counted line decoded"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn test_recovery_resuming_earlier_discards_the_duplicates() {
        // The recovery response of F8, verbatim: the server answers `PROG,11`
        // to a request that asked for a later point, "in order to simulate
        // notifications already sent by the Server but not received. As a
        // consequence, a few notifications will be duplicated."
        // [`docs/spec/02-session-lifecycle.md` §10, F8].
        let mut first = subscribed_transcript(F8_STREAM);
        first.push(Step::Fail("connection reset"));

        let mut second = transcript(
            "
            CONOK,S22dee113e3f71b1fT4327493,50000,5000,*
            NOOP,sending placeholder data
            CONS,unlimited
            PROG,11
            U,1,1,|15:55:13|14.64
            U,1,1,|15:55:14|
            U,1,1,||14.74
            U,1,1,||14.66
            U,1,1,|15:55:15|17.7
            U,1,1,||17.81
            SYNC,5
            ",
        );
        second.push(line(SERVER_END));

        let outcome = drive(
            websocket(),
            options(),
            vec![first, second],
            vec![subscribe()],
        )
        .await;

        assert_eq!(
            outcome.recoveries(),
            vec![RecoveryOutcome {
                requested: 15,
                resumed_at: 11,
                kind: RecoveryKind::Duplicated { count: 4 },
            }]
        );
        // Precondition 4: the four duplicates are discarded before the caller
        // sees them, and counting resumes at 16
        // [`docs/spec/02-session-lifecycle.md` §5.2].
        let progressives: Vec<u64> = outcome.data().iter().map(|(p, _, _)| *p).collect();
        assert_eq!(progressives.len(), 15 + 2);
        assert_eq!(progressives.get(15), Some(&16));
        assert_eq!(progressives.last(), Some(&17));
    }

    #[tokio::test(start_paused = true)]
    async fn test_recovery_resuming_later_is_reported_as_a_gap() {
        // SPEC-AMBIGUITY A7: `PROG` higher than requested is undefined. It is
        // reported rather than hidden, because telling an application that
        // continuity held when it did not is worse than saying nothing
        // (`docs/adr/0005-recovery-is-visible-in-the-event-stream.md`).
        let first = vec![
            line(F8_CONOK),
            Step::AwaitControls(1),
            line("SUBOK,1,1,3"),
            line("U,1,1,a|b|c"),
            Step::Fail("connection reset"),
        ];
        let second = vec![
            line(F8_CONOK),
            line("PROG,9"),
            line("U,1,1,d|e|f"),
            line(SERVER_END),
        ];

        let outcome = drive(
            websocket(),
            options(),
            vec![first, second],
            vec![subscribe()],
        )
        .await;

        assert_eq!(
            outcome.recoveries(),
            vec![RecoveryOutcome {
                requested: 2,
                resumed_at: 9,
                kind: RecoveryKind::Gap { missing: 7 },
            }]
        );
        // The counter follows the server, otherwise every later recovery would
        // ask from the wrong point.
        assert_eq!(outcome.data().last().map(|(p, _, _)| *p), Some(10));
    }

    // -----------------------------------------------------------------------
    // T6 — Unbound → Bound, `Session Rebound`
    // -----------------------------------------------------------------------

    #[tokio::test(start_paused = true)]
    async fn test_t6_rebind_preserves_the_session_and_its_subscriptions() {
        // "all subscriptions are preserved, with all their items and fields"
        // [`docs/spec/02-session-lifecycle.md` §4.4], which is why the client
        // must **not** re-subscribe after a rebind.
        let first = vec![
            line(F6_CONOK),
            Step::AwaitControls(1),
            line("REQOK,1"),
            line("SUBOK,1,1,3"),
            line("LOOP,0"),
            Step::End,
        ];
        let second = vec![line(F6_CONOK), line("U,1,1,x|y|z"), line(SERVER_END)];

        let outcome = drive(
            websocket(),
            options(),
            vec![first, second],
            vec![SessionCommand::Subscribe {
                key: SubscriptionKey(1),
                spec: Box::new(spec()),
            }],
        )
        .await;

        assert_eq!(
            outcome.bound().get(1).map(|info| info.kind.clone()),
            Some(BindKind::Rebound)
        );
        let adds = outcome
            .controls()
            .iter()
            .filter(|parameters| parameters.contains("LS_op=add"))
            .count();
        assert_eq!(adds, 1, "the subscription must not be issued twice");
        assert!(outcome.resubscribes().is_empty());
    }

    // -----------------------------------------------------------------------
    // T7 / T8 — `Destroyed by Client`
    // -----------------------------------------------------------------------

    #[tokio::test(start_paused = true)]
    async fn test_t7_destroy_while_bound_ends_with_the_servers_cause() {
        // Fixture F5: `REQOK,3` then `END,31,Destroy invoked by client`
        // [`docs/spec/02-session-lifecycle.md` §10, F5].
        let script = vec![
            line(F1_CONOK),
            Step::AwaitControls(1),
            line("REQOK,1"),
            line("END,31,Destroy invoked by client"),
            Step::End,
        ];

        let outcome = drive(
            websocket(),
            options(),
            vec![script],
            vec![SessionCommand::Destroy { cause: None }],
        )
        .await;

        assert_eq!(
            outcome.closed,
            SessionClosed::ByClient {
                destroy_confirmed: true,
                cause: Some(ServerCause {
                    code: 31,
                    message: "Destroy invoked by client".to_owned(),
                }),
            }
        );
        assert!(
            outcome
                .controls()
                .iter()
                .any(|parameters| parameters.contains("LS_op=destroy")),
            "{:?}",
            outcome.controls()
        );
        assert_eq!(outcome.closes(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn test_t8_destroy_while_unbound_ends_without_waiting_for_an_end() {
        // SPEC-AMBIGUITY A1: with nothing bound there is nowhere for the `END`
        // of T7 to be delivered, and the spec does not say whether it is
        // dropped, buffered or suppressed. The driver does not wait for one.
        let first = vec![line(F1_CONOK), line("LOOP,60000"), Step::End];

        let outcome = drive(
            websocket(),
            options(),
            vec![first],
            vec![SessionCommand::Destroy { cause: None }],
        )
        .await;

        assert_eq!(
            outcome.closed,
            SessionClosed::ByClient {
                destroy_confirmed: false,
                cause: None,
            }
        );
        // Exactly one stream connection was ever opened: the destroy landed
        // during the rebind delay.
        assert_eq!(outcome.opens(), vec![OpenRecord::Create]);
    }

    // -----------------------------------------------------------------------
    // T9 — Unbound → final, `Timed Out`
    // -----------------------------------------------------------------------

    #[tokio::test(start_paused = true)]
    async fn test_t9_timed_out_session_is_recreated_and_subscriptions_re_executed() {
        // T9: the unbound session is discarded server-side, "a later
        // `bind_session` then fails with `CONERR,20`", and the client must
        // "`create_session` and re-execute all subscriptions"
        // [`docs/spec/02-session-lifecycle.md` §2.2, T9; §6.1].
        let first = vec![
            line(F1_CONOK),
            Step::AwaitControls(2),
            line("REQOK,1"),
            line("REQOK,2"),
            line("SUBOK,1,1,3"),
            line("SUBOK,2,1,3"),
            Step::Fail("connection reset"),
        ];
        let second = vec![line("CONERR,20,Specified session not found"), Step::End];
        let third = vec![line(F6_CONOK), line(SERVER_END)];

        let outcome = drive(
            websocket(),
            options(),
            vec![first, second, third],
            vec![
                SessionCommand::Subscribe {
                    key: SubscriptionKey(1),
                    spec: Box::new(spec()),
                },
                SessionCommand::Subscribe {
                    key: SubscriptionKey(2),
                    spec: Box::new(SubscriptionSpec::new(
                        "item1",
                        "last_price",
                        SubscriptionMode::Merge,
                    )),
                },
            ],
        )
        .await;

        // The failed bind is retryable, so it produced an unbind, not a close.
        assert!(matches!(
            outcome.unbinds().get(1),
            Some(UnbindReason::Rejected {
                cause: ServerCause { code: 20, .. }
            })
        ));
        // A brand-new session: continuity is gone and the caller is told so.
        assert_eq!(
            outcome.bound().get(1).map(|info| info.kind.clone()),
            Some(BindKind::Recreated {
                previous: Some(SessionId::new(F1_SESSION)),
            })
        );
        // Losing a subscription silently is the defect this asserts against.
        let resubscribed = outcome.resubscribes();
        let entries = resubscribed.first().expect("a resubscribe batch");
        assert_eq!(entries.len(), 2);
        assert_eq!(
            entries.iter().map(|e| e.key).collect::<Vec<_>>(),
            vec![SubscriptionKey(1), SubscriptionKey(2)]
        );
        // `LS_subId` restarts at 1 in the new session
        // [`docs/spec/02-session-lifecycle.md` §9.5], while the caller's keys
        // do not change.
        assert_eq!(
            entries
                .iter()
                .map(|e| e.subscription_id.get())
                .collect::<Vec<_>>(),
            vec![1, 2]
        );
        assert!(entries.iter().all(|e| e.previously_active));
        // Four `add` requests in total: two per session.
        let adds = outcome
            .controls()
            .iter()
            .filter(|parameters| parameters.contains("LS_op=add"))
            .count();
        assert_eq!(adds, 4);
    }

    // -----------------------------------------------------------------------
    // Server-initiated END on a bound session (§2.2 note, ambiguity A2)
    // -----------------------------------------------------------------------

    #[tokio::test(start_paused = true)]
    async fn test_server_initiated_end_on_a_bound_session_is_definitive_loss() {
        // Code 35: "the Metadata Adapter… requested the closure of the current
        // session upon opening of a new session for the same user"
        // [`docs/spec/02-session-lifecycle.md` §6.2]. Reconnecting would fight
        // the other client, so it is fatal.
        let script = vec![
            line(F1_CONOK),
            line("END,35,Another session was opened for this user"),
            Step::End,
        ];

        let outcome = drive(websocket(), options(), vec![script], Vec::new()).await;

        assert_eq!(
            outcome.closed,
            SessionClosed::ByServer {
                cause: ServerCause {
                    code: 35,
                    message: "Another session was opened for this user".to_owned(),
                },
            }
        );
        assert_eq!(outcome.opens().len(), 1, "no retry after a fatal cause");
        assert!(matches!(
            outcome.events.last(),
            Some(SessionEvent::Closed(SessionClosed::ByServer { .. }))
        ));
    }

    #[tokio::test(start_paused = true)]
    async fn test_server_end_code_48_recreates_the_session_immediately() {
        // "the client should recover by opening a new session immediately"
        // [`docs/spec/02-session-lifecycle.md` §6.2, code 48].
        let first = vec![
            line(F1_CONOK),
            line("END,48,Maximum session duration reached"),
            Step::End,
        ];
        let second = vec![line(F6_CONOK), line(SERVER_END)];

        let outcome = drive(websocket(), options(), vec![first, second], Vec::new()).await;

        assert!(matches!(
            outcome.unbinds().first(),
            Some(UnbindReason::ServerRefresh {
                cause: ServerCause { code: 48, .. }
            })
        ));
        // Immediately: no backoff delay at all.
        let retry_in = outcome.events.iter().find_map(|event| match event {
            SessionEvent::Unbound { retry_in, .. } => *retry_in,
            _ => None,
        });
        assert_eq!(retry_in, Some(Duration::ZERO));
        assert_eq!(outcome.opens().get(1), Some(&OpenRecord::Create));
    }

    // -----------------------------------------------------------------------
    // CONERR: definitive loss versus permitted retry (§6.2)
    // -----------------------------------------------------------------------

    #[tokio::test(start_paused = true)]
    async fn test_conerr_authentication_failure_is_definitive_loss() {
        // Code 1, "User/password check failed" — retrying cannot help
        // [`docs/spec/05-error-codes.md` §2].
        let script = vec![line("CONERR,1,User/password check failed"), Step::End];

        let outcome = drive(websocket(), options(), vec![script], Vec::new()).await;

        assert_eq!(
            outcome.closed,
            SessionClosed::ByServer {
                cause: ServerCause {
                    code: 1,
                    message: "User/password check failed".to_owned(),
                },
            }
        );
        assert_eq!(outcome.opens().len(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn test_conerr_retry_later_code_is_retried_and_succeeds() {
        // Codes 5 and 6 both end with "retry later"
        // [`docs/spec/05-error-codes.md` §5.5].
        let first = vec![
            line("CONERR,5,The Server is temporarily overloaded: retry later"),
            Step::End,
        ];
        let second = vec![line(F1_CONOK), line(SERVER_END)];

        let outcome = drive(websocket(), options(), vec![first, second], Vec::new()).await;

        assert!(matches!(
            outcome.unbinds().first(),
            Some(UnbindReason::Rejected {
                cause: ServerCause { code: 5, .. }
            })
        ));
        assert_eq!(outcome.bound().len(), 1);
        assert_eq!(outcome.opens().len(), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn test_conerr_cluster_affinity_code_is_treated_as_permanent() {
        // SPEC-AMBIGUITY: code 21 matches the description of the permanent
        // clustering failure, but `docs/spec/05-error-codes.md` §5.3 warns
        // "the document never states the linkage. Do not assume it."
        // Retrying a permanent misconfiguration is a reconnect storm, so the
        // defensive reading stops and hands the caller the code.
        let script = vec![
            line("CONERR,21,Session ID not compatible with this Server instance"),
            Step::End,
        ];

        let outcome = drive(websocket(), options(), vec![script], Vec::new()).await;

        assert!(matches!(
            outcome.closed,
            SessionClosed::ByServer {
                cause: ServerCause { code: 21, .. }
            }
        ));
    }

    #[test]
    fn test_classify_covers_every_code_the_spec_prescribes_an_action_for() {
        // The only codes for which the spec states a client action
        // [`docs/spec/05-error-codes.md` §5].
        assert_eq!(classify(4), Recovery::RecreateSession);
        assert_eq!(classify(20), Recovery::RecreateSession);
        assert_eq!(classify(48), Recovery::RecreateSession);
        assert_eq!(classify(5), Recovery::RetryLater);
        assert_eq!(classify(6), Recovery::RetryLater);
        assert_eq!(classify(10), Recovery::RetryLater);
        // Fatal by policy, each documented at its arm.
        assert_eq!(classify(1), Recovery::Fatal);
        assert_eq!(classify(21), Recovery::Fatal);
        assert_eq!(classify(31), Recovery::Fatal);
        // An unknown positive code, and an adapter-supplied one, are not
        // retried: the spec says nothing about either.
        assert_eq!(classify(9999), Recovery::Fatal);
        assert_eq!(classify(0), Recovery::Fatal);
        assert_eq!(classify(-7), Recovery::Fatal);
    }

    // -----------------------------------------------------------------------
    // Liveness (§8)
    // -----------------------------------------------------------------------

    #[tokio::test(start_paused = true)]
    async fn test_keepalive_expiry_forces_a_rebind_on_a_wedged_connection() {
        // The failure mode this client exists to prevent: a connection that
        // never errors and never delivers. `CONOK` negotiates 5000 ms and the
        // configured slack is 3000 ms [`docs/spec/02-session-lifecycle.md`
        // §8.1]. Time is paused, so this test advances the virtual clock only.
        let first = vec![line(F1_CONOK), Step::Silence];
        let second = vec![line(F1_CONOK), line(SERVER_END)];

        let outcome = drive(websocket(), options(), vec![first, second], Vec::new()).await;

        assert_eq!(
            outcome.unbinds(),
            vec![&UnbindReason::KeepaliveExpired {
                budget: Duration::from_millis(8000)
            }]
        );
        // §5.1: "if a connection becomes mute, the client can issue a recovery
        // request" — so it is a recovery, not a plain rebind.
        assert_eq!(
            outcome.opens().get(1),
            Some(&OpenRecord::Bind {
                session: Some(F1_SESSION.to_owned()),
                recovery_from: Some(0),
            })
        );
        // §8.1: the wedged connection is "closed and reopened", not merely
        // abandoned — once for the stall, once on the way out.
        assert_eq!(outcome.closes(), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn test_open_timeout_abandons_an_unanswered_handshake() {
        // Nothing at all arrives, not even a `CONOK`. The keepalive budget
        // cannot apply because it has not been negotiated yet, so the client's
        // own open timeout is what ends the attempt.
        let first = vec![Step::Silence];
        let second = vec![line(F1_CONOK), line(SERVER_END)];

        let outcome = drive(websocket(), options(), vec![first, second], Vec::new()).await;

        assert!(matches!(
            outcome.unbinds().first(),
            Some(UnbindReason::ConnectionFailed { .. })
        ));
        assert_eq!(outcome.bound().len(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn test_reverse_heartbeat_is_sent_before_the_inactivity_commitment_expires() {
        // "If no request is needed, a heartbeat pseudo-request can be sent. If
        // no request is received for more than this time, the Server can assume
        // that the client is stuck" [`docs/spec/02-session-lifecycle.md` §8.4].
        // The commitment is 8000 ms, so a heartbeat is due at 4000 ms — before
        // the 8000 ms keepalive budget ends the connection.
        let first = vec![line(F1_CONOK), Step::Silence];
        let second = vec![line(F1_CONOK), line(SERVER_END)];

        let committed = options().with_connection(ConnectionMode::Streaming {
            inactivity_millis: Some(8000),
            keepalive_millis: None,
            send_sync: None,
        });
        let outcome = drive(websocket(), committed, vec![first, second], Vec::new()).await;

        let controls = outcome.controls();
        assert!(
            controls
                .iter()
                .any(|parameters| parameters.contains("LS_session=")),
            "the heartbeat declares the session it is listening to: {controls:?}"
        );
        assert!(!controls.is_empty(), "a heartbeat must have been sent");
        // And the stall still fires afterwards, because a heartbeat is an
        // outbound obligation and says nothing about the inbound one.
        assert!(matches!(
            outcome.unbinds().first(),
            Some(UnbindReason::KeepaliveExpired { .. })
        ));
    }

    #[tokio::test(start_paused = true)]
    async fn test_a_quiet_but_probing_connection_is_never_declared_stalled() {
        // `PROBE` is what the server sends "when no other activity has been
        // sent on the stream connection" [§8.1]; receiving them means the
        // connection is healthy however little data flows.
        let mut script = vec![line(F1_CONOK)];
        for _ in 0..20 {
            script.push(line("PROBE"));
        }
        script.push(line(SERVER_END));

        let outcome = drive(websocket(), options(), vec![script], Vec::new()).await;

        assert!(outcome.unbinds().is_empty());
        assert!(matches!(outcome.closed, SessionClosed::ByServer { .. }));
    }

    // -----------------------------------------------------------------------
    // Control-response correlation (§4.5, ADR-0007)
    // -----------------------------------------------------------------------

    #[tokio::test(start_paused = true)]
    async fn test_control_responses_are_correlated_when_interleaved_with_data() {
        // "responses may always arrive interspersed with notifications, when
        // control requests are sent on the stream connection"
        // [`docs/spec/02-session-lifecycle.md` §4.5]. The request id is the
        // only thing that correlates them
        // (`docs/adr/0007-transport-port-shape.md`).
        let script = vec![
            line(F1_CONOK),
            Step::AwaitControls(2),
            line("U,1,1,a|b|c"),
            // Out of order on purpose: "responses may arrive out of order"
            // [`docs/spec/02-session-lifecycle.md` §11.3].
            line("REQOK,2"),
            line("U,1,1,d|e|f"),
            line("REQOK,1"),
            line("SUBOK,1,1,3"),
            line(SERVER_END),
        ];

        let outcome = drive(
            websocket(),
            options(),
            vec![script],
            vec![
                SessionCommand::Subscribe {
                    key: SubscriptionKey(1),
                    spec: Box::new(spec()),
                },
                SessionCommand::Subscribe {
                    key: SubscriptionKey(2),
                    spec: Box::new(SubscriptionSpec::new(
                        "item1",
                        "last_price",
                        SubscriptionMode::Merge,
                    )),
                },
            ],
        )
        .await;

        let responses: Vec<String> = outcome
            .events
            .iter()
            .filter_map(|event| match event {
                SessionEvent::ControlResponse {
                    request_id: Some(id),
                    outcome: ControlOutcome::Accepted,
                    ..
                } => Some(id.as_str().to_owned()),
                _ => None,
            })
            .collect();
        assert_eq!(responses, vec!["2".to_owned(), "1".to_owned()]);

        // The `LS_reqId` on the wire is what the response echoed, and the two
        // subscriptions carry different ids.
        let controls = outcome.controls();
        assert_eq!(
            controls
                .iter()
                .filter(|parameters| parameters.contains("LS_reqId=1"))
                .count(),
            1,
            "{controls:?}"
        );
        assert_eq!(
            controls
                .iter()
                .filter(|parameters| parameters.contains("LS_reqId=2"))
                .count(),
            1,
            "{controls:?}"
        );
        // And the data around them still reached the caller, in order.
        assert_eq!(outcome.data().len(), 3);
    }

    #[tokio::test(start_paused = true)]
    async fn test_request_ids_are_never_reused_across_sessions() {
        // SPEC-AMBIGUITY A5: "unique within the connection", where
        // "connection" is undefined. A single monotonic sequence for the life
        // of the driver satisfies every reading, and makes the late responses
        // of §4.5 impossible to misattribute.
        let first = vec![
            line(F1_CONOK),
            Step::AwaitControls(1),
            line("REQOK,1"),
            line("SUBOK,1,1,3"),
            Step::Fail("connection reset"),
        ];
        let second = vec![line("CONERR,20,Specified session not found"), Step::End];
        let third = vec![line(F6_CONOK), Step::AwaitControls(2), line(SERVER_END)];

        let outcome = drive(
            websocket(),
            options(),
            vec![first, second, third],
            vec![SessionCommand::Subscribe {
                key: SubscriptionKey(1),
                spec: Box::new(spec()),
            }],
        )
        .await;

        let controls = outcome.controls();
        assert_eq!(controls.len(), 2);
        assert_eq!(
            controls
                .iter()
                .filter(|parameters| parameters.contains("LS_reqId=1"))
                .count(),
            1,
            "{controls:?}"
        );
        assert_eq!(
            controls
                .iter()
                .filter(|parameters| parameters.contains("LS_reqId=2"))
                .count(),
            1,
            "{controls:?}"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn test_rejected_subscription_is_reported_and_not_re_issued() {
        // A subscription the server refuses must not be retried on every future
        // session — but the caller must be told, because losing one silently is
        // the defect this guards against.
        let first = vec![
            line(F1_CONOK),
            Step::AwaitControls(1),
            line("REQERR,1,23,Bad Field schema name"),
            Step::Fail("connection reset"),
        ];
        let second = vec![line("CONERR,20,Specified session not found"), Step::End];
        let third = vec![line(F6_CONOK), line(SERVER_END)];

        let outcome = drive(
            websocket(),
            options(),
            vec![first, second, third],
            vec![SessionCommand::Subscribe {
                key: SubscriptionKey(1),
                spec: Box::new(spec()),
            }],
        )
        .await;

        let rejected = outcome.events.iter().any(|event| {
            matches!(
                event,
                SessionEvent::ControlResponse {
                    outcome: ControlOutcome::Rejected {
                        cause: ServerCause { code: 23, .. }
                    },
                    ..
                }
            )
        });
        assert!(rejected, "the refusal must reach the caller");
        let adds = outcome
            .controls()
            .iter()
            .filter(|parameters| parameters.contains("LS_op=add"))
            .count();
        assert_eq!(adds, 1, "a refused subscription is not re-issued");
    }

    #[tokio::test(start_paused = true)]
    async fn test_uncorrelatable_error_is_surfaced_without_a_request_id() {
        // `ERROR` "carries no request ID, because the server could not parse
        // one" [`docs/spec/03-requests.md` §13.2].
        let script = vec![
            line(F1_CONOK),
            line("ERROR,67,Malformed request"),
            line(SERVER_END),
        ];

        let outcome = drive(websocket(), options(), vec![script], Vec::new()).await;

        assert!(outcome.events.iter().any(|event| matches!(
            event,
            SessionEvent::ControlResponse {
                request_id: None,
                target: ControlTarget::Session,
                outcome: ControlOutcome::Rejected {
                    cause: ServerCause { code: 67, .. }
                },
            }
        )));
    }

    // -----------------------------------------------------------------------
    // Routing a control failure back to the subscription it concerns
    // -----------------------------------------------------------------------

    fn control_targets(outcome: &Outcome) -> Vec<(ControlTarget, ControlOutcome)> {
        outcome
            .events
            .iter()
            .filter_map(|event| match event {
                SessionEvent::ControlResponse {
                    target, outcome, ..
                } => Some((target.clone(), outcome.clone())),
                _ => None,
            })
            .collect()
    }

    #[tokio::test(start_paused = true)]
    async fn test_rejected_subscription_names_the_subscription_it_killed() {
        // A rejection the caller cannot route is a subscription lost in
        // silence: the caller's stream simply goes quiet forever. The response
        // therefore names the key **and** the operation, because "add refused"
        // means the subscription does not exist while "delete refused" means it
        // still does.
        let script = vec![
            line(F1_CONOK),
            Step::AwaitControls(1),
            line("REQERR,1,23,Bad Field schema name"),
            line(SERVER_END),
        ];

        let outcome = drive(
            websocket(),
            options(),
            vec![script],
            vec![SessionCommand::Subscribe {
                key: SubscriptionKey(7),
                spec: Box::new(spec()),
            }],
        )
        .await;

        assert_eq!(
            control_targets(&outcome),
            vec![(
                ControlTarget::Subscription {
                    key: SubscriptionKey(7),
                    operation: SubscriptionOperation::Subscribe,
                },
                ControlOutcome::Rejected {
                    cause: ServerCause {
                        code: 23,
                        message: "Bad Field schema name".to_owned(),
                    },
                },
            )]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn test_rejected_unsubscribe_names_the_subscription_that_is_still_alive() {
        // The delete failed, so the subscription is still there and still
        // delivering. A caller told only "rejected" would have the state
        // exactly backwards.
        let script = vec![
            line(F1_CONOK),
            Step::AwaitControls(1),
            line("REQOK,1"),
            line("SUBOK,1,1,3"),
            Step::AwaitControls(2),
            line("REQERR,2,19,Specified subscription not found"),
            line("U,1,1,still|coming|through"),
            line(SERVER_END),
        ];

        let outcome = drive(
            websocket(),
            options(),
            vec![script],
            vec![
                SessionCommand::Subscribe {
                    key: SubscriptionKey(1),
                    spec: Box::new(spec()),
                },
                SessionCommand::Unsubscribe {
                    key: SubscriptionKey(1),
                },
            ],
        )
        .await;

        let rejection = control_targets(&outcome)
            .into_iter()
            .find(|(_, outcome)| matches!(outcome, ControlOutcome::Rejected { .. }));
        assert_eq!(
            rejection.map(|(target, _)| target),
            Some(ControlTarget::Subscription {
                key: SubscriptionKey(1),
                operation: SubscriptionOperation::Unsubscribe,
            })
        );
        // Still alive, and its updates still route to the same key.
        let routed = outcome.data().iter().any(|(_, key, outcome)| {
            *key == SubscriptionKey(1) && matches!(outcome, Ok(SubscriptionEvent::Update(_)))
        });
        assert!(
            routed,
            "the surviving subscription still routes its updates"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn test_rejected_reconfigure_names_the_subscription_and_the_operation() {
        // Code 13 is "Subscription frequency reconfiguration not allowed
        // because the subscription is configured for unfiltered dispatching"
        // [`docs/spec/05-error-codes.md` §2].
        let script = vec![
            line(F1_CONOK),
            Step::AwaitControls(1),
            line("REQOK,1"),
            line("SUBOK,1,1,3"),
            Step::AwaitControls(2),
            line("REQERR,2,13,Reconfiguration not allowed"),
            line(SERVER_END),
        ];

        let outcome = drive(
            websocket(),
            options(),
            vec![script],
            vec![
                SessionCommand::Subscribe {
                    key: SubscriptionKey(1),
                    spec: Box::new(spec()),
                },
                SessionCommand::Reconfigure {
                    key: SubscriptionKey(1),
                    max_frequency: MaxFrequencyLimit::Unlimited,
                },
            ],
        )
        .await;

        let rejection = control_targets(&outcome)
            .into_iter()
            .find(|(_, outcome)| matches!(outcome, ControlOutcome::Rejected { .. }));
        assert_eq!(
            rejection.map(|(target, _)| target),
            Some(ControlTarget::Subscription {
                key: SubscriptionKey(1),
                operation: SubscriptionOperation::Reconfigure,
            })
        );
    }

    #[tokio::test(start_paused = true)]
    async fn test_session_level_control_responses_target_the_session() {
        // `force_rebind` and `destroy` concern no subscription, so there is
        // nothing to route them to.
        let script = vec![
            line(F1_CONOK),
            Step::AwaitControls(1),
            line("REQOK,1"),
            line("END,31,Destroy invoked by client"),
            Step::End,
        ];

        let outcome = drive(
            websocket(),
            options(),
            vec![script],
            vec![SessionCommand::Destroy { cause: None }],
        )
        .await;

        assert_eq!(
            control_targets(&outcome)
                .into_iter()
                .map(|(target, _)| target)
                .collect::<Vec<_>>(),
            vec![ControlTarget::Session]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn test_subscription_whose_add_could_not_be_sent_is_re_issued_on_the_next_bind() {
        // The other silent-loss path: the `add` never leaves the client, so no
        // `REQERR` will ever arrive to clean up after it. Without releasing the
        // wire binding the subscription would look issued forever and be
        // skipped by every later re-establishment.
        let first = vec![line(F1_CONOK), Step::AwaitControls(1), Step::Fail("reset")];
        let second = vec![line(F1_CONOK), Step::AwaitControls(2), line(SERVER_END)];

        let transport = MockTransport::new(websocket(), vec![first, second]).failing_controls(1);

        let outcome = drive_transport(
            transport,
            options(),
            vec![SessionCommand::Subscribe {
                key: SubscriptionKey(1),
                spec: Box::new(spec()),
            }],
        )
        .await;

        // The failure is reported, named, and not silent.
        assert_eq!(
            control_targets(&outcome)
                .into_iter()
                .filter(|(_, outcome)| matches!(outcome, ControlOutcome::NotSent { .. }))
                .map(|(target, _)| target)
                .collect::<Vec<_>>(),
            vec![ControlTarget::Subscription {
                key: SubscriptionKey(1),
                operation: SubscriptionOperation::Subscribe,
            }]
        );
        // And it is issued again on the recovered session, with a fresh
        // `LS_subId` — ids "must not be reused" within a session (§4.4).
        let adds: Vec<String> = outcome
            .controls()
            .into_iter()
            .filter(|parameters| parameters.contains("LS_op=add"))
            .collect();
        assert_eq!(adds.len(), 2, "{adds:?}");
        assert!(
            adds.last().is_some_and(|last| last.contains("LS_subId=2")),
            "{adds:?}"
        );
    }

    // -----------------------------------------------------------------------
    // Message send (§12)
    // -----------------------------------------------------------------------

    fn messages(
        outcome: &Outcome,
    ) -> Vec<(Option<u64>, MessageSequence, Option<u64>, MessageResult)> {
        outcome
            .events
            .iter()
            .filter_map(|event| match event {
                SessionEvent::Message {
                    progressive,
                    sequence,
                    prog,
                    result,
                } => Some((*progressive, sequence.clone(), *prog, result.clone())),
                _ => None,
            })
            .collect()
    }

    #[tokio::test(start_paused = true)]
    async fn test_message_send_is_dispatched_and_its_outcome_reported() {
        // The spec's own `msg` example orders the parameters LS_sequence,
        // LS_msg_prog, then LS_message [`docs/spec/03-requests.md` §12.4], and
        // the outcome arrives as `MSGDONE`
        // [`docs/spec/04-notifications.md` §4.1].
        let script = vec![
            line(F1_CONOK),
            Step::AwaitControls(1),
            line("REQOK,1"),
            line("MSGDONE,Orders_Sequence,3,Processed with ID 32652506"),
            line(SERVER_END),
        ];

        let sequence = SequenceName::try_new("Orders_Sequence").expect("a valid sequence name");
        let message = OutgoingMessage::numbered("buy 100", NonZeroU32::new(3).expect("non-zero"))
            .in_sequence(sequence);

        let outcome = drive(
            websocket(),
            options(),
            vec![script],
            vec![SessionCommand::SendMessage {
                message: Box::new(message),
            }],
        )
        .await;

        let controls = outcome.controls();
        assert_eq!(controls.len(), 1);
        let sent = controls.first().expect("one control request");
        assert!(sent.contains("LS_reqId=1"), "{sent}");
        assert!(sent.contains("LS_sequence=Orders_Sequence"), "{sent}");
        assert!(sent.contains("LS_msg_prog=3"), "{sent}");
        assert!(sent.contains("LS_message=buy%20100"), "{sent}");

        assert_eq!(
            messages(&outcome),
            vec![(
                Some(1),
                MessageSequence::Named("Orders_Sequence".to_owned()),
                Some(3),
                MessageResult::Done {
                    response: "Processed with ID 32652506".to_owned(),
                },
            )]
        );
        // The submission is still reported on the control surface: `REQOK`
        // means accepted, `MSGDONE` means processed
        // [`docs/spec/03-requests.md` §13.1; §4.1].
        assert!(outcome.events.iter().any(|event| matches!(
            event,
            SessionEvent::ControlResponse {
                outcome: ControlOutcome::Accepted,
                ..
            }
        )));
    }

    #[tokio::test(start_paused = true)]
    async fn test_message_outcomes_are_counted_toward_the_recovery_progressive() {
        // `MSGDONE` and `MSGFAIL` are data notifications
        // [`docs/spec/02-session-lifecycle.md` §5.3], so they must advance the
        // count even though they leave on the message surface.
        let script = vec![
            line(F1_CONOK),
            Step::AwaitControls(1),
            line("REQOK,1"),
            line("SUBOK,1,1,3"),
            line("MSGDONE,*,1,"),
            line("U,1,1,a|b|c"),
            Step::Fail("connection reset"),
        ];
        let second = vec![line(F1_CONOK), line("PROG,3"), line(SERVER_END)];

        let outcome = drive(
            websocket(),
            options(),
            vec![script, second],
            vec![SessionCommand::SendMessage {
                message: Box::new(OutgoingMessage::numbered(
                    "hello",
                    NonZeroU32::new(1).expect("non-zero"),
                )),
            }],
        )
        .await;

        assert_eq!(
            messages(&outcome)
                .first()
                .map(|(progressive, ..)| *progressive),
            Some(Some(2)),
            "the MSGDONE sits between the SUBOK and the U"
        );
        // SUBOK (1), MSGDONE (2), U (3) — so the recovery asks from 3.
        assert_eq!(
            outcome.opens().get(1),
            Some(&OpenRecord::Bind {
                session: Some(F1_SESSION.to_owned()),
                recovery_from: Some(3),
            })
        );
    }

    #[tokio::test(start_paused = true)]
    async fn test_message_failure_is_reported_with_the_servers_cause() {
        // `MSGFAIL` [`docs/spec/04-notifications.md` §4.2]; code 38 is "The
        // specified progressive number has been skipped by timeout"
        // [`docs/spec/05-error-codes.md` §2].
        let script = vec![
            line(F1_CONOK),
            Step::AwaitControls(1),
            line("REQOK,1"),
            line(
                "MSGFAIL,Orders_Sequence,4,38,The specified progressive number has been skipped by timeout",
            ),
            line(SERVER_END),
        ];

        let sequence = SequenceName::try_new("Orders_Sequence").expect("a valid sequence name");
        let message = OutgoingMessage::numbered("sell 50", NonZeroU32::new(4).expect("non-zero"))
            .in_sequence(sequence);

        let outcome = drive(
            websocket(),
            options(),
            vec![script],
            vec![SessionCommand::SendMessage {
                message: Box::new(message),
            }],
        )
        .await;

        assert_eq!(
            messages(&outcome)
                .first()
                .map(|(_, _, _, result)| result.clone()),
            Some(MessageResult::Failed {
                cause: ServerCause {
                    code: 38,
                    message: "The specified progressive number has been skipped by timeout"
                        .to_owned(),
                },
            })
        );
    }

    #[tokio::test(start_paused = true)]
    async fn test_message_refused_at_submission_gets_exactly_one_outcome() {
        // A `REQERR` means no `MSGDONE`/`MSGFAIL` will follow
        // [`docs/spec/03-requests.md` §13.2], so the refusal is the message's
        // one and only outcome.
        let script = vec![
            line(F1_CONOK),
            Step::AwaitControls(1),
            line("REQERR,1,33,A message with this number has already been enqueued"),
            line(SERVER_END),
        ];

        let outcome = drive(
            websocket(),
            options(),
            vec![script],
            vec![SessionCommand::SendMessage {
                message: Box::new(OutgoingMessage::numbered(
                    "duplicate",
                    NonZeroU32::new(2).expect("non-zero"),
                )),
            }],
        )
        .await;

        let reported = messages(&outcome);
        assert_eq!(reported.len(), 1, "exactly one outcome per message");
        assert_eq!(
            reported.first().map(|(_, _, _, result)| result.clone()),
            Some(MessageResult::Refused {
                cause: ServerCause {
                    code: 33,
                    message: "A message with this number has already been enqueued".to_owned(),
                },
            })
        );
    }

    #[tokio::test(start_paused = true)]
    async fn test_message_sent_while_unbound_fails_fast_and_is_never_buffered() {
        // A subscription is desired state and is re-issued on the next session;
        // a message is a one-shot side effect whose numbering the server
        // deduplicates per session, so it is failed instead of held.
        let first = vec![line(F1_CONOK), line("LOOP,60000"), Step::End];

        let outcome = drive(
            websocket(),
            options(),
            vec![first],
            vec![
                SessionCommand::SendMessage {
                    message: Box::new(OutgoingMessage::numbered(
                        "too late",
                        NonZeroU32::new(1).expect("non-zero"),
                    )),
                },
                SessionCommand::Shutdown,
            ],
        )
        .await;

        assert!(matches!(
            messages(&outcome)
                .first()
                .map(|(_, _, _, result)| result.clone()),
            Some(MessageResult::NotSent { .. })
        ));
        // Nothing was sent then, and nothing is sent later either.
        assert!(outcome.controls().is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn test_fire_and_forget_message_declines_its_outcome() {
        // `LS_msg_prog` "is mandatory whenever `LS_sequence` is specified or
        // `LS_outcome` is set to `true`", and `LS_outcome` defaults to `true`
        // [`docs/spec/03-requests.md` §12.1] — so a message with no progressive
        // must decline the outcome explicitly, or the encoder refuses it.
        let script = vec![line(F1_CONOK), Step::AwaitControls(1), line(SERVER_END)];

        let outcome = drive(
            websocket(),
            options(),
            vec![script],
            vec![SessionCommand::SendMessage {
                message: Box::new(OutgoingMessage::fire_and_forget("ping")),
            }],
        )
        .await;

        let controls = outcome.controls();
        let sent = controls.first().expect("one control request");
        assert!(sent.contains("LS_outcome=false"), "{sent}");
        assert!(!sent.contains("LS_msg_prog"), "{sent}");
        // No outcome was requested, so none is reported.
        assert!(messages(&outcome).is_empty());
    }

    // -----------------------------------------------------------------------
    // ADR-0005: the caller can tell the outcomes apart
    // -----------------------------------------------------------------------

    #[tokio::test(start_paused = true)]
    async fn test_adr0005_recovery_and_reestablishment_are_distinguishable() {
        // Same interruption, two server answers. An application holding derived
        // state must be able to keep it in one case and discard it in the other
        // (`docs/adr/0005-recovery-is-visible-in-the-event-stream.md`).
        let interrupted = || {
            vec![
                line(F8_CONOK),
                line("SUBOK,1,1,3"),
                Step::Fail("connection reset"),
            ]
        };

        let recovered = drive(
            websocket(),
            options(),
            vec![
                interrupted(),
                vec![line(F8_CONOK), line("PROG,1"), line(SERVER_END)],
            ],
            Vec::new(),
        )
        .await;
        assert_eq!(
            recovered.bound().get(1).map(|info| info.kind.clone()),
            Some(BindKind::Recovering {
                requested_progressive: 1
            })
        );
        assert_eq!(
            recovered.recoveries().first().map(|outcome| outcome.kind),
            Some(RecoveryKind::Exact)
        );

        let replaced = drive(
            websocket(),
            options(),
            vec![
                interrupted(),
                vec![line("CONERR,4,Recovery not possible"), Step::End],
                vec![line(F1_CONOK), line(SERVER_END)],
            ],
            Vec::new(),
        )
        .await;
        assert_eq!(
            replaced.bound().get(1).map(|info| info.kind.clone()),
            Some(BindKind::Recreated {
                previous: Some(SessionId::new(F8_SESSION)),
            })
        );
        assert!(replaced.recoveries().is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn test_adr0005_snapshot_re_delivery_is_visible_on_resubscribe() {
        // "a subscription's data restarted from a snapshot, versus resumed
        // without one" (ADR-0005). The spec says a snapshot is sent only if the
        // subscription asked for it [`docs/spec/03-requests.md` §6.1], so the
        // request is what the caller is told about.
        let mut with_snapshot = spec();
        with_snapshot.snapshot = Some(Snapshot::On);

        let first = vec![
            line(F1_CONOK),
            Step::AwaitControls(1),
            line("REQOK,1"),
            line("SUBOK,1,1,3"),
            Step::Fail("connection reset"),
        ];
        let second = vec![line("CONERR,20,Specified session not found"), Step::End];
        let third = vec![line(F6_CONOK), line(SERVER_END)];

        let outcome = drive(
            websocket(),
            options(),
            vec![first, second, third],
            vec![SessionCommand::Subscribe {
                key: SubscriptionKey(1),
                spec: Box::new(with_snapshot),
            }],
        )
        .await;

        let resubscribed = outcome.resubscribes();
        let entry = resubscribed
            .first()
            .and_then(|entries| entries.first())
            .expect("one re-subscribed entry");
        assert!(entry.snapshot_requested);
        assert!(entry.previously_active);
    }

    // -----------------------------------------------------------------------
    // Reconnection policy and shutdown
    // -----------------------------------------------------------------------

    #[tokio::test(start_paused = true)]
    async fn test_retries_are_bounded_and_reported_as_definitive_loss() {
        // An unbounded retry that never surfaces a typed outcome is exactly
        // what the backoff policy exists to prevent.
        // The stream never binds: a `CONOK` would reset the budget, which is
        // what `test_a_successful_bind_resets_the_retry_budget` covers.
        let failing = || vec![Step::Fail("connection refused")];
        let options = options().with_backoff(BackoffPolicy {
            initial: Duration::from_millis(10),
            max: Duration::from_millis(20),
            max_attempts: NonZeroU32::new(2),
        });

        let outcome = drive(
            websocket(),
            options,
            vec![failing(), failing(), failing(), failing()],
            Vec::new(),
        )
        .await;

        assert!(matches!(
            outcome.closed,
            SessionClosed::RetriesExhausted { .. }
        ));
        // The initial attempt plus two retries.
        assert_eq!(outcome.opens().len(), 3);
        // Every failed connection is torn down, and so is the last one.
        assert_eq!(outcome.closes(), 4);
    }

    #[tokio::test(start_paused = true)]
    async fn test_a_successful_bind_resets_the_retry_budget() {
        // Otherwise a long-lived session would eventually exhaust its budget on
        // unrelated hiccups spread over days.
        let options = options().with_backoff(BackoffPolicy {
            initial: Duration::from_millis(10),
            max: Duration::from_millis(20),
            max_attempts: NonZeroU32::new(1),
        });
        let outcome = drive(
            websocket(),
            options,
            vec![
                vec![line(F1_CONOK), Step::Fail("blip")],
                vec![line(F1_CONOK), Step::Fail("blip")],
                vec![line(F1_CONOK), line(SERVER_END)],
            ],
            Vec::new(),
        )
        .await;

        assert!(matches!(outcome.closed, SessionClosed::ByServer { .. }));
        assert_eq!(outcome.bound().len(), 3);
    }

    #[tokio::test(start_paused = true)]
    async fn test_shutdown_stops_the_driver_and_closes_the_transport() {
        let script = vec![line(F1_CONOK), Step::Silence];

        let outcome = drive(
            websocket(),
            options(),
            vec![script],
            vec![SessionCommand::Shutdown],
        )
        .await;

        assert_eq!(
            outcome.closed,
            SessionClosed::ByClient {
                destroy_confirmed: false,
                cause: None,
            }
        );
        assert_eq!(outcome.closes(), 1);
        assert_eq!(outcome.opens().len(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn test_dropping_every_handle_stops_the_driver() {
        let transport = MockTransport::new(websocket(), vec![vec![line(F1_CONOK), Step::Silence]]);
        let log = Arc::clone(&transport.log);
        let (driver, handle, _events) =
            connect(transport, options()).expect("options must match the transport");
        drop(handle);

        let closed = driver.run().await;

        assert_eq!(
            closed,
            SessionClosed::ByClient {
                destroy_confirmed: false,
                cause: None,
            }
        );
        assert_eq!(log.lock().expect("mock log poisoned").closes, 1);
    }

    // -----------------------------------------------------------------------
    // Configuration and forward compatibility
    // -----------------------------------------------------------------------

    #[tokio::test(start_paused = true)]
    async fn test_connect_rejects_a_polling_mismatch_between_options_and_transport() {
        // `LS_polling` changes the meaning of the `CONOK` keep-alive argument
        // and forbids `LS_keepalive_millis` [§7.2, §8.4], so the two must agree.
        let transport = MockTransport::new(http_polling(), Vec::new());
        let result = connect(transport, options());
        assert!(matches!(result, Err(SessionError::Configuration { .. })));
    }

    #[tokio::test(start_paused = true)]
    async fn test_unknown_tag_is_surfaced_and_never_fatal() {
        // "a future server version must not crash an old client". The MPN tags
        // are the concrete case in v1 [`docs/spec/06-mpn.md`].
        let script = vec![
            line(F1_CONOK),
            Step::AwaitControls(1),
            line("SUBOK,1,1,3"),
            line("MPNREG,devid,adapter"),
            line("U,1,1,a|b|c"),
            line(SERVER_END),
        ];

        let outcome = drive(websocket(), options(), vec![script], vec![subscribe()]).await;

        assert!(
            outcome
                .events
                .iter()
                .any(|event| matches!(event, SessionEvent::Unparsed { .. }))
        );
        // SPEC-AMBIGUITY A6: an unparsed line is not counted, so the `U` after
        // it is progressive 2 rather than 3. Under-counting makes a later
        // recovery ask from an earlier point, which the protocol handles by
        // re-delivering duplicates; over-counting would skip data.
        assert_eq!(
            outcome
                .data()
                .iter()
                .map(|(p, _, _)| *p)
                .collect::<Vec<_>>(),
            vec![1, 2]
        );
        assert!(matches!(outcome.closed, SessionClosed::ByServer { .. }));
    }

    #[tokio::test(start_paused = true)]
    async fn test_control_link_is_handed_to_the_transport() {
        // "the address… to which every following session rebind and control
        // request must be sent to" [`docs/spec/02-session-lifecycle.md` §3.1].
        let script = vec![
            line("CONOK,S1,50000,5000,push2.example.com:8080"),
            line(SERVER_END),
        ];

        let outcome = drive(websocket(), options(), vec![script], Vec::new()).await;

        assert_eq!(
            outcome.log.lock().expect("mock log poisoned").control_links,
            vec![Some("push2.example.com:8080".to_owned())]
        );
        assert_eq!(
            outcome
                .bound()
                .first()
                .and_then(|info| info.control_link.clone()),
            Some("push2.example.com:8080".to_owned())
        );
    }

    #[tokio::test(start_paused = true)]
    async fn test_unsubscribe_removes_the_subscription_from_the_desired_set() {
        // After `UNSUB` "no further update for the subscription will be sent"
        // [`docs/spec/04-notifications.md` §3.4], so it must not come back on
        // the next session either.
        let first = vec![
            line(F1_CONOK),
            Step::AwaitControls(1),
            line("REQOK,1"),
            line("SUBOK,1,1,3"),
            Step::AwaitControls(2),
            line("REQOK,2"),
            line("UNSUB,1"),
            Step::Fail("connection reset"),
        ];
        let second = vec![line("CONERR,20,Specified session not found"), Step::End];
        let third = vec![line(F6_CONOK), line(SERVER_END)];

        let outcome = drive(
            websocket(),
            options(),
            vec![first, second, third],
            vec![
                SessionCommand::Subscribe {
                    key: SubscriptionKey(1),
                    spec: Box::new(spec()),
                },
                SessionCommand::Unsubscribe {
                    key: SubscriptionKey(1),
                },
            ],
        )
        .await;

        let adds = outcome
            .controls()
            .iter()
            .filter(|parameters| parameters.contains("LS_op=add"))
            .count();
        assert_eq!(adds, 1, "an unsubscribed item is not re-established");
        assert!(outcome.resubscribes().is_empty());
    }

    // -----------------------------------------------------------------------
    // Concurrency, liveness and shutdown
    // -----------------------------------------------------------------------

    /// How many `LS_op=add` requests a run produced.
    fn adds(outcome: &Outcome) -> usize {
        outcome
            .controls()
            .iter()
            .filter(|parameters| parameters.contains("LS_op=add"))
            .count()
    }

    /// C-03. The pump is biased towards the server, and a server that always
    /// has a line ready would otherwise postpone every command for ever.
    ///
    /// Deliberately **not** on a paused clock: the condition this guards
    /// against is a driver that never yields, under which a paused clock never
    /// advances. Note that a regression here shows up as a hung test rather
    /// than a failing one, and unavoidably so — a future that never yields
    /// cannot be interrupted from inside the runtime it is starving. That is
    /// the defect, stated exactly.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_a_flooding_server_cannot_starve_a_destroy() {
        // `PROBE` is chosen deliberately: it produces no event, so nothing here
        // depends on the event channel having room. What is being measured is
        // the scheduling, not the backpressure.
        let script = vec![
            line(F1_CONOK),
            Step::FloodUntilControls("PROBE", 1),
            line("END,31,session destroyed"),
        ];

        let transport = MockTransport::new(websocket(), vec![script]);
        let log = Arc::clone(&transport.log);
        let (driver, handle, events) = connect(transport, options()).expect("valid options");
        handle
            .destroy(None)
            .await
            .expect("the driver has not started yet");

        let running = tokio::spawn(driver.run());
        tokio::time::timeout(Duration::from_secs(10), running)
            .await
            .expect("an unbounded line stream must not postpone a command for ever")
            .expect("the driver task did not panic");

        let controls = log.lock().expect("mock log poisoned").control_parameters();
        assert!(
            controls
                .iter()
                .any(|parameters| parameters.contains("LS_op=destroy")),
            "the destroy reached the wire: {controls:?}"
        );
        drop((handle, events));
    }

    /// C-02. A caller that stops reading its event stream stalls the session —
    /// that is the documented contract — but must not be able to make it
    /// unstoppable.
    #[tokio::test(start_paused = true)]
    async fn test_a_stop_is_honoured_while_the_event_stream_is_full() {
        // One event of capacity and nobody reading it: the driver blocks on the
        // second one, inside `emit`, where it is polling neither the socket nor
        // the command channel.
        let mut options = options();
        options.event_capacity = std::num::NonZeroUsize::MIN;
        // A flood that never stops: no control request is ever sent here, so
        // the gate never opens and the line stream stays permanently ready.
        let script = vec![
            line(F1_CONOK),
            Step::AwaitControls(1),
            line("SUBOK,1,1,1"),
            Step::FloodUntilControls("U,1,1,20.4", usize::MAX),
        ];

        let transport = MockTransport::new(websocket(), vec![script]);
        let log = Arc::clone(&transport.log);
        let (driver, handle, events) = connect(transport, options).expect("valid options");
        handle
            .commands
            .send(subscribe())
            .await
            .expect("the driver has not started yet");

        let running = tokio::spawn(driver.run());
        // Let the driver bind, fill the single slot, and block on the next one.
        tokio::time::sleep(Duration::from_millis(50)).await;

        handle.stop();
        let closed = tokio::time::timeout(Duration::from_secs(5), running)
            .await
            .expect("a stop must not be subject to backpressure")
            .expect("the driver task did not panic");

        assert!(matches!(closed, SessionClosed::ByClient { .. }));
        assert!(
            log.lock().expect("mock log poisoned").closes >= 1,
            "the transport is closed on every exit path"
        );
        drop(events);
    }

    /// C-10. The budget covers the whole establishment, not just the wait for
    /// `CONOK` that follows it.
    #[tokio::test(start_paused = true)]
    async fn test_an_establishment_that_hangs_is_abandoned_within_the_budget() {
        let mut options = options();
        options.open_timeout = Duration::from_secs(3);
        let transport = MockTransport::new(websocket(), Vec::new()).hanging_open();

        let outcome = tokio::time::timeout(
            Duration::from_secs(120),
            drive_transport(transport, options, Vec::new()),
        )
        .await
        .expect("a hanging dial must not hang the session");

        assert!(matches!(
            outcome.closed,
            SessionClosed::RetriesExhausted { .. }
        ));
        assert!(
            outcome
                .unbinds()
                .iter()
                .all(|reason| matches!(reason, UnbindReason::ConnectionFailed { .. })),
            "every attempt timed out: {:?}",
            outcome.unbinds()
        );
    }

    /// C-10. A stop is honoured while a dial is still hanging.
    #[tokio::test(start_paused = true)]
    async fn test_a_stop_is_honoured_while_a_dial_is_hanging() {
        let mut options = options();
        // Long enough that the timeout cannot be what ends this.
        options.open_timeout = Duration::from_secs(3600);
        let transport = MockTransport::new(websocket(), Vec::new()).hanging_open();
        let (driver, handle, events) = connect(transport, options).expect("valid options");

        let running = tokio::spawn(driver.run());
        tokio::time::sleep(Duration::from_millis(50)).await;
        handle.stop();

        let closed = tokio::time::timeout(Duration::from_secs(5), running)
            .await
            .expect("a stop must interrupt an establishment")
            .expect("the driver task did not panic");
        assert!(matches!(closed, SessionClosed::ByClient { .. }));
        drop(events);
    }

    /// C-11. A heartbeat that cannot be written leaves an already-expired
    /// deadline in place; retrying it immediately is a spin.
    #[tokio::test(start_paused = true)]
    async fn test_repeated_heartbeat_failures_give_the_connection_up() {
        let options = options().with_connection(ConnectionMode::Streaming {
            inactivity_millis: Some(2000),
            keepalive_millis: None,
            send_sync: None,
        });
        // Silence after the bind: nothing but the heartbeat clock can fire.
        let first = vec![line(F1_CONOK), Step::Silence];
        let second = vec![line("CONERR,1,User/password check failed")];
        let transport = MockTransport::new(websocket(), vec![first, second])
            // Every heartbeat, and every retry of it, fails.
            .failing_controls(usize::MAX);

        let outcome = tokio::time::timeout(
            Duration::from_secs(600),
            drive_transport(transport, options, Vec::new()),
        )
        .await
        .expect("a failing heartbeat must not spin for ever");

        // Bounded: the connection was given up rather than retried without end.
        assert!(
            outcome
                .unbinds()
                .iter()
                .any(|reason| matches!(reason, UnbindReason::ConnectionFailed { .. })),
            "{:?}",
            outcome.unbinds()
        );
        let heartbeats = outcome
            .controls()
            .iter()
            .filter(|parameters| parameters.contains("LS_session"))
            .count();
        assert!(
            heartbeats <= usize::try_from(MAX_HEARTBEAT_FAILURES).unwrap_or(usize::MAX),
            "at most {MAX_HEARTBEAT_FAILURES} attempts before giving up, got {heartbeats}"
        );
    }

    /// C-12. An identifier is never handed out twice, and exhaustion is
    /// reported rather than wrapped.
    #[tokio::test]
    async fn test_identifier_spaces_are_exhausted_rather_than_reused() {
        let (mut driver, handle, events) =
            connect(MockTransport::new(websocket(), Vec::new()), options()).expect("valid options");

        driver.next_request_id = u64::MAX;
        assert!(
            driver.next_request_id().is_none(),
            "the last id is never handed out, because it cannot be advanced past"
        );

        handle.next_key.store(u64::MAX, Ordering::Relaxed);
        assert!(matches!(
            handle.allocate_key(),
            Err(SessionError::Exhausted { .. })
        ));
        drop((driver, events));
    }

    /// C-17. `PROG` is "sent only when requested via `LS_recovery_from`"
    /// [`docs/spec/02-session-lifecycle.md` §5.4]. One that answers no request
    /// of ours must not move the baseline every recovery decision rests on.
    #[tokio::test(start_paused = true)]
    async fn test_an_unsolicited_prog_cannot_move_the_recovery_baseline() {
        // A plain creation, one data notification, and a `PROG` claiming the
        // flow is a thousand notifications further on than it is.
        let first = vec![
            line(F1_CONOK),
            line("U,1,1,20.4"),
            line("PROG,1000"),
            Step::Fail("connection reset"),
        ];
        let second = vec![line(F1_CONOK), line(SERVER_END)];

        let outcome = drive(websocket(), options(), vec![first, second], Vec::new()).await;

        assert_eq!(
            outcome.opens().get(1),
            Some(&OpenRecord::Bind {
                session: Some(F1_SESSION.to_owned()),
                recovery_from: Some(1),
            }),
            "the recovery asks to resume from what was actually counted"
        );
        assert!(
            outcome.recoveries().is_empty(),
            "an unsolicited PROG reports no recovery outcome"
        );
    }

    /// C-18. Code 48 asks for a new session "immediately"
    /// [`docs/spec/05-error-codes.md` §5.4]. A server that answers every new
    /// session the same way must not get an unthrottled loop.
    #[tokio::test(start_paused = true)]
    async fn test_repeated_session_refreshes_are_delayed_and_eventually_given_up() {
        let refresh = || vec![line(F1_CONOK), line("END,48,please reconnect")];
        let scripts = (0..12).map(|_| refresh()).collect();

        let outcome = tokio::time::timeout(
            Duration::from_secs(600),
            drive(websocket(), options(), scripts, Vec::new()),
        )
        .await
        .expect("a refresh loop must terminate");

        let delays: Vec<Option<Duration>> = outcome
            .events
            .iter()
            .filter_map(|event| match event {
                SessionEvent::Unbound { retry_in, .. } => Some(*retry_in),
                _ => None,
            })
            .collect();
        assert_eq!(
            delays.first(),
            Some(&Some(Duration::ZERO)),
            "the first refresh is immediate, as the spec prescribes"
        );
        assert!(
            delays
                .iter()
                .skip(1)
                .any(|delay| matches!(delay, Some(delay) if !delay.is_zero())),
            "later refreshes are delayed: {delays:?}"
        );
        assert!(
            matches!(outcome.closed, SessionClosed::RetriesExhausted { .. }),
            "a server that only ever refreshes is a definitive loss, got {:?}",
            outcome.closed
        );
    }

    /// C-18. A session that delivered data is not part of a refresh loop, so a
    /// deployment that legitimately refreshes never accumulates a streak.
    #[tokio::test(start_paused = true)]
    async fn test_a_productive_session_clears_the_refresh_streak() {
        let productive = || {
            vec![
                line(F1_CONOK),
                line("U,1,1,20.4"),
                line("END,48,please reconnect"),
            ]
        };
        let scripts = vec![
            productive(),
            productive(),
            productive(),
            vec![line(F1_CONOK), line(SERVER_END)],
        ];

        let outcome = drive(websocket(), options(), scripts, Vec::new()).await;

        let delays: Vec<Option<Duration>> = outcome
            .events
            .iter()
            .filter_map(|event| match event {
                SessionEvent::Unbound { retry_in, .. } => Some(*retry_in),
                _ => None,
            })
            .collect();
        assert!(
            delays.iter().all(|delay| *delay == Some(Duration::ZERO)),
            "every refresh follows a productive session: {delays:?}"
        );
        assert!(matches!(outcome.closed, SessionClosed::ByServer { .. }));
    }

    /// C-07. §4.5 promises that "late responses to control requests related
    /// with the first session [may] arrive interspersed with notifications for
    /// the second session". One of them must not take a live subscription with
    /// it.
    #[tokio::test(start_paused = true)]
    async fn test_a_late_rejection_from_a_replaced_session_changes_nothing() {
        // Session A issues the `add` as request 1 and is then refreshed.
        let first = vec![
            line(F1_CONOK),
            Step::AwaitControls(1),
            line("END,48,please reconnect"),
        ];
        // Session B re-issues it as request 2 — and only then does session A's
        // refusal of request 1 arrive.
        let second = vec![
            line(F6_CONOK),
            Step::AwaitControls(2),
            line("REQERR,1,19,Specified subscription not found"),
            line("U,1,1,20.4"),
            line("END,48,please reconnect"),
        ];
        // If the late refusal had been obeyed, there would be no third `add`.
        let third = vec![line(F8_CONOK), Step::AwaitControls(3), line(SERVER_END)];

        let outcome = drive(
            websocket(),
            options(),
            vec![first, second, third],
            vec![SessionCommand::Subscribe {
                key: SubscriptionKey(1),
                spec: Box::new(spec()),
            }],
        )
        .await;

        assert_eq!(
            adds(&outcome),
            3,
            "the subscription survived a refusal aimed at a session that no longer exists"
        );
        // The request that never got its answer is closed out at the boundary,
        // rather than left pending for ever.
        assert!(
            control_targets(&outcome).iter().any(|(target, outcome)| {
                matches!(
                    target,
                    ControlTarget::Subscription {
                        operation: SubscriptionOperation::Subscribe,
                        ..
                    }
                ) && matches!(outcome, ControlOutcome::NotSent { .. })
            }),
            "a retired request reports exactly one outcome: {:?}",
            control_targets(&outcome)
        );
    }

    /// C-07. The same guarantee for a message: one report per message, even
    /// when the session it was sent on is replaced before the answer arrives.
    #[tokio::test(start_paused = true)]
    async fn test_a_message_pending_across_a_replacement_still_gets_one_outcome() {
        let first = vec![
            line(F1_CONOK),
            Step::AwaitControls(1),
            line("END,48,please reconnect"),
        ];
        let second = vec![line(F6_CONOK), line(SERVER_END)];

        let outcome = drive(
            websocket(),
            options(),
            vec![first, second],
            vec![SessionCommand::SendMessage {
                message: Box::new(OutgoingMessage::numbered("BUY 100", NonZeroU32::MIN)),
            }],
        )
        .await;

        let reported = messages(&outcome);
        assert_eq!(reported.len(), 1, "exactly one report: {reported:?}");
        assert!(matches!(
            reported.first().map(|(_, _, _, result)| result),
            Some(MessageResult::NotSent { .. })
        ));
    }

    /// C-08. An accepted `reconf` changes what the subscription *is*, so a
    /// session that has to be recreated must re-issue the new value.
    #[tokio::test(start_paused = true)]
    async fn test_an_accepted_reconfiguration_survives_a_recreated_session() {
        let first = vec![
            line(F1_CONOK),
            Step::AwaitControls(1),
            line("REQOK,1"),
            line("SUBOK,1,1,3"),
            Step::AwaitControls(2),
            line("REQOK,2"),
            line("END,48,please reconnect"),
        ];
        let second = vec![line(F6_CONOK), Step::AwaitControls(3), line(SERVER_END)];

        let limit = MaxFrequencyLimit::Limited(
            crate::protocol::request::DecimalNumber::try_new("2.5").expect("a decimal number"),
        );
        let outcome = drive(
            websocket(),
            options(),
            vec![first, second],
            vec![
                SessionCommand::Subscribe {
                    key: SubscriptionKey(1),
                    spec: Box::new(spec()),
                },
                SessionCommand::Reconfigure {
                    key: SubscriptionKey(1),
                    max_frequency: limit,
                },
            ],
        )
        .await;

        let controls = outcome.controls();
        let reissued = controls
            .iter()
            .filter(|parameters| parameters.contains("LS_op=add"))
            .nth(1)
            .expect("the subscription was re-issued on the new session");
        assert!(
            reissued.contains("LS_requested_max_frequency=2.5"),
            "the recreated subscription keeps the frequency the server granted: {reissued}"
        );
    }

    /// C-08, the other half: a `reconf` the server refused must **not** change
    /// the desired state.
    #[tokio::test(start_paused = true)]
    async fn test_a_refused_reconfiguration_leaves_the_desired_state_alone() {
        let first = vec![
            line(F1_CONOK),
            Step::AwaitControls(1),
            line("REQOK,1"),
            line("SUBOK,1,1,3"),
            Step::AwaitControls(2),
            line("REQERR,2,25,Subscription is not unfiltered"),
            line("END,48,please reconnect"),
        ];
        let second = vec![line(F6_CONOK), Step::AwaitControls(3), line(SERVER_END)];

        let limit = MaxFrequencyLimit::Limited(
            crate::protocol::request::DecimalNumber::try_new("2.5").expect("a decimal number"),
        );
        let outcome = drive(
            websocket(),
            options(),
            vec![first, second],
            vec![
                SessionCommand::Subscribe {
                    key: SubscriptionKey(1),
                    spec: Box::new(spec()),
                },
                SessionCommand::Reconfigure {
                    key: SubscriptionKey(1),
                    max_frequency: limit,
                },
            ],
        )
        .await;

        let controls = outcome.controls();
        let reissued = controls
            .iter()
            .filter(|parameters| parameters.contains("LS_op=add"))
            .nth(1)
            .expect("the subscription was re-issued on the new session");
        assert!(
            !reissued.contains("LS_requested_max_frequency"),
            "a refused change is not desired state: {reissued}"
        );
    }
}
