//! Liveness enforcement: a stream that has gone quiet past its budget is dead.
//!
//! TLCP gives the client two independent obligations, one per direction
//! [`docs/spec/02-session-lifecycle.md` §8.7]:
//!
//! - **Inbound.** `CONOK` argument 3 is `<keep-alive>`, "the longest inactivity
//!   time guaranteed throughout connection life time"; when nothing else has
//!   been sent for that long the server emits a `PROBE`, and "not receiving any
//!   message for longer than this time may be the signal of a problem"
//!   [`docs/spec/02-session-lifecycle.md` §8.1]. Official clients "monitor this
//!   notification to detect when the connection is stalled: if after the
//!   expected interval **plus a configurable timeout** no `PROBE` has been
//!   received, the connection is closed and reopened" [ibid.].
//! - **Outbound.** `LS_inactivity_millis` is the "maximum time the Client is
//!   committed to wait before issuing a request to the Server while this
//!   connection is open. If no request is needed, a heartbeat pseudo-request
//!   can be sent" [`docs/spec/02-session-lifecycle.md` §8.4]. Crucially, "any
//!   Control Request has the same effect of a Heartbeat, hence no Heartbeat is
//!   needed as long as Control Requests are sent" [ibid.].
//!
//! This module is the timekeeping half of both rules. It holds no transport and
//! performs no I/O: it is fed the instants at which lines arrive and requests
//! leave, and it answers one question — what, if anything, is due now.
//!
//! # Why the inbound rule is not optional
//!
//! The failure mode this client exists to prevent is a connection that is
//! wedged but never errors: no `PROBE`, no data, no `Err`, no `None` from the
//! transport, forever. Nothing below this module can detect that, because
//! nothing below it knows what silence is supposed to look like. The budget
//! here is the only thing that turns such a connection back into a recovery
//! [`docs/spec/02-session-lifecycle.md` §5.1: "if a connection becomes mute,
//! the client can issue a recovery request"].

use std::time::Duration;

use tokio::time::Instant;

/// Add a budget to an instant without risking an overflow panic.
///
/// `Instant + Duration` panics on overflow, which is unreachable with sane
/// budgets but is not something a protocol client may assume. Saturating at the
/// original instant is the conservative direction: it makes the deadline fire
/// *sooner*, never later, so a stall can never be missed.
#[must_use]
#[inline]
fn deadline(from: Instant, budget: Duration) -> Instant {
    from.checked_add(budget).unwrap_or(from)
}

/// What the driver must do because a liveness deadline has fallen due.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LivenessAction {
    /// Nothing is due yet.
    Idle,

    /// The outbound commitment is about to expire: send a `heartbeat`
    /// pseudo-request, or any control request, to keep it
    /// [`docs/spec/02-session-lifecycle.md` §8.4].
    SendHeartbeat,

    /// Nothing has arrived from the server within its budget. The stream is
    /// treated as dead and must be reopened
    /// [`docs/spec/02-session-lifecycle.md` §8.1].
    InboundStalled {
        /// The budget that elapsed with no traffic at all.
        budget: Duration,
    },
}

/// The keepalive and reverse-heartbeat clocks of one stream connection.
///
/// The inbound clock is *armed* while a stream connection is open — first with
/// the open timeout, then, once `CONOK` names the negotiated keep-alive, with
/// that value plus the configured slack. It is disarmed while no stream
/// connection exists, because silence then carries no meaning.
#[derive(Debug)]
pub(crate) struct Liveness {
    /// Extra time allowed on top of the negotiated keep-alive before the
    /// stream is declared stalled.
    ///
    /// This is the spec's "configurable timeout", whose value it leaves
    /// entirely to the client and for which it gives no number or range
    /// [`docs/spec/02-session-lifecycle.md` §8.1, ambiguity A12].
    slack: Duration,

    /// The inbound budget currently in force, or `None` while disarmed.
    inbound_budget: Option<Duration>,

    /// When the last line arrived from the server — or when the stream was
    /// opened, if none has arrived yet.
    last_inbound: Instant,

    /// How often to send an outbound request when nothing else needs sending,
    /// or `None` when the client made no `LS_inactivity_millis` commitment.
    heartbeat_every: Option<Duration>,

    /// When the last request was sent to the server. Any request counts, not
    /// just a `heartbeat` [`docs/spec/02-session-lifecycle.md` §8.4].
    last_outbound: Instant,
}

impl Liveness {
    /// Creates the clocks for a session that has not opened a stream yet.
    ///
    /// `slack` is the client-side grace added to the negotiated keep-alive.
    /// `heartbeat_every` is `None` unless the client committed to an
    /// `LS_inactivity_millis`, in which case it should be comfortably shorter
    /// than that commitment.
    #[must_use]
    pub(crate) fn new(slack: Duration, heartbeat_every: Option<Duration>, now: Instant) -> Self {
        Self {
            slack,
            inbound_budget: None,
            last_inbound: now,
            heartbeat_every,
            last_outbound: now,
        }
    }

    /// Arms the inbound clock with the open timeout, at the moment a stream
    /// connection is opened.
    ///
    /// Until `CONOK` arrives there is no negotiated keep-alive to use
    /// [`docs/spec/02-session-lifecycle.md` §3.1], so the budget is the
    /// client's own limit on how long it will wait for the handshake.
    pub(crate) fn on_stream_opened(&mut self, open_timeout: Duration, now: Instant) {
        self.inbound_budget = Some(open_timeout);
        self.last_inbound = now;
    }

    /// Re-arms the inbound clock with the keep-alive the server reported in
    /// `CONOK` [`docs/spec/02-session-lifecycle.md` §8.1].
    ///
    /// The budget becomes `keep_alive + slack`: the interval the server
    /// guarantees, plus the client-side grace the spec requires but does not
    /// quantify.
    pub(crate) fn on_bound(&mut self, keep_alive: Duration, now: Instant) {
        self.inbound_budget = Some(keep_alive.checked_add(self.slack).unwrap_or(keep_alive));
        self.last_inbound = now;
    }

    /// Disarms the inbound clock, for use while no stream connection exists.
    ///
    /// While unbound the server buffers rather than sends
    /// [`docs/spec/02-session-lifecycle.md` §2.1], so silence is expected and
    /// must not be read as a stall.
    pub(crate) fn on_stream_closed(&mut self) {
        self.inbound_budget = None;
    }

    /// Records that a line arrived from the server.
    ///
    /// **Every** line counts, not only `PROBE`: the keep-alive is defined as
    /// the longest interval with no traffic of any kind, and a `PROBE` is only
    /// what the server sends when it has nothing better
    /// [`docs/spec/02-session-lifecycle.md` §8.1].
    pub(crate) fn on_inbound(&mut self, now: Instant) {
        self.last_inbound = now;
    }

    /// Records that a request was sent to the server.
    ///
    /// Any control request satisfies the outbound commitment, so this is called
    /// for all of them and not only for `heartbeat`
    /// [`docs/spec/02-session-lifecycle.md` §8.4].
    pub(crate) fn on_outbound(&mut self, now: Instant) {
        self.last_outbound = now;
    }

    /// The next instant at which something falls due, or `None` when neither
    /// clock is running.
    #[must_use]
    pub(crate) fn next_deadline(&self) -> Option<Instant> {
        let inbound = self
            .inbound_budget
            .map(|budget| deadline(self.last_inbound, budget));
        let outbound = self
            .heartbeat_every
            .map(|every| deadline(self.last_outbound, every));
        match (inbound, outbound) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (Some(a), None) => Some(a),
            (None, Some(b)) => Some(b),
            (None, None) => None,
        }
    }

    /// What is due as of `now`.
    ///
    /// A stall outranks a heartbeat: once the stream is declared dead there is
    /// nothing left to keep alive.
    #[must_use]
    pub(crate) fn due(&self, now: Instant) -> LivenessAction {
        if let Some(budget) = self.inbound_budget
            && now >= deadline(self.last_inbound, budget)
        {
            return LivenessAction::InboundStalled { budget };
        }
        if let Some(every) = self.heartbeat_every
            && now >= deadline(self.last_outbound, every)
        {
            return LivenessAction::SendHeartbeat;
        }
        LivenessAction::Idle
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A negotiated keep-alive matching every Hands On transcript, where
    /// `CONOK,...,50000,5000,*` reports 5 s
    /// [`docs/spec/02-session-lifecycle.md` §8.1].
    const KEEP_ALIVE: Duration = Duration::from_millis(5000);
    const SLACK: Duration = Duration::from_millis(3000);

    #[tokio::test(start_paused = true)]
    async fn test_liveness_disarmed_before_open_has_no_deadline() {
        let liveness = Liveness::new(SLACK, None, Instant::now());
        assert_eq!(liveness.next_deadline(), None);
        assert_eq!(liveness.due(Instant::now()), LivenessAction::Idle);
    }

    #[tokio::test(start_paused = true)]
    async fn test_liveness_open_timeout_arms_the_inbound_clock() {
        let start = Instant::now();
        let mut liveness = Liveness::new(SLACK, None, start);
        liveness.on_stream_opened(Duration::from_secs(10), start);

        assert_eq!(liveness.due(start), LivenessAction::Idle);
        assert_eq!(
            liveness.due(deadline(start, Duration::from_secs(10))),
            LivenessAction::InboundStalled {
                budget: Duration::from_secs(10)
            }
        );
    }

    #[tokio::test(start_paused = true)]
    async fn test_liveness_budget_is_keepalive_plus_slack_after_conok() {
        let start = Instant::now();
        let mut liveness = Liveness::new(SLACK, None, start);
        liveness.on_bound(KEEP_ALIVE, start);

        // Silence for exactly the negotiated keep-alive is not yet a stall:
        // the server is entitled to that much [§8.1].
        assert_eq!(
            liveness.due(deadline(start, KEEP_ALIVE)),
            LivenessAction::Idle
        );
        // Keep-alive plus the client-side slack is.
        assert_eq!(
            liveness.due(deadline(start, Duration::from_millis(8000))),
            LivenessAction::InboundStalled {
                budget: Duration::from_millis(8000)
            }
        );
    }

    #[tokio::test(start_paused = true)]
    async fn test_liveness_any_inbound_line_resets_the_budget() {
        let start = Instant::now();
        let mut liveness = Liveness::new(SLACK, None, start);
        liveness.on_bound(KEEP_ALIVE, start);

        let later = deadline(start, Duration::from_millis(7000));
        liveness.on_inbound(later);

        // The old deadline has passed, but the line moved it.
        assert_eq!(
            liveness.due(deadline(start, Duration::from_millis(8000))),
            LivenessAction::Idle
        );
        assert_eq!(
            liveness.due(deadline(later, Duration::from_millis(8000))),
            LivenessAction::InboundStalled {
                budget: Duration::from_millis(8000)
            }
        );
    }

    #[tokio::test(start_paused = true)]
    async fn test_liveness_disarm_makes_silence_meaningless_again() {
        let start = Instant::now();
        let mut liveness = Liveness::new(SLACK, None, start);
        liveness.on_bound(KEEP_ALIVE, start);
        liveness.on_stream_closed();

        assert_eq!(
            liveness.due(deadline(start, Duration::from_secs(600))),
            LivenessAction::Idle
        );
        assert_eq!(liveness.next_deadline(), None);
    }

    #[tokio::test(start_paused = true)]
    async fn test_liveness_heartbeat_is_due_after_the_configured_interval() {
        let start = Instant::now();
        let mut liveness = Liveness::new(SLACK, Some(Duration::from_secs(2)), start);
        liveness.on_bound(KEEP_ALIVE, start);

        assert_eq!(liveness.due(start), LivenessAction::Idle);
        assert_eq!(
            liveness.due(deadline(start, Duration::from_secs(2))),
            LivenessAction::SendHeartbeat
        );
    }

    #[tokio::test(start_paused = true)]
    async fn test_liveness_any_outbound_request_defers_the_heartbeat() {
        // "any Control Request has the same effect of a Heartbeat" [§8.4].
        let start = Instant::now();
        let mut liveness = Liveness::new(SLACK, Some(Duration::from_secs(2)), start);
        liveness.on_bound(KEEP_ALIVE, start);

        let sent = deadline(start, Duration::from_millis(1500));
        liveness.on_outbound(sent);

        assert_eq!(
            liveness.due(deadline(start, Duration::from_secs(2))),
            LivenessAction::Idle
        );
        assert_eq!(
            liveness.due(deadline(sent, Duration::from_secs(2))),
            LivenessAction::SendHeartbeat
        );
    }

    #[tokio::test(start_paused = true)]
    async fn test_liveness_stall_outranks_heartbeat() {
        let start = Instant::now();
        let mut liveness = Liveness::new(SLACK, Some(Duration::from_secs(2)), start);
        liveness.on_bound(KEEP_ALIVE, start);

        // Both clocks are past due; the stall wins.
        assert_eq!(
            liveness.due(deadline(start, Duration::from_secs(60))),
            LivenessAction::InboundStalled {
                budget: Duration::from_millis(8000)
            }
        );
    }

    #[tokio::test(start_paused = true)]
    async fn test_liveness_next_deadline_is_the_earlier_clock() {
        let start = Instant::now();
        let mut liveness = Liveness::new(SLACK, Some(Duration::from_secs(2)), start);
        liveness.on_bound(KEEP_ALIVE, start);

        assert_eq!(
            liveness.next_deadline(),
            Some(deadline(start, Duration::from_secs(2)))
        );
    }
}
