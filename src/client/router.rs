//! The fan-out between the session and the caller's streams.
//!
//! The session layer produces one ordered sequence of events for the whole
//! connection [`crate::session::SessionEvent`]. A caller wants one stream per
//! subscription plus one for the session itself
//! (`docs/adr/0003-typed-event-stream-as-delivery-surface.md`). This module is
//! the task that turns the first into the second.
//!
//! # Why three tasks and not one
//!
//! A client runs three futures: the session driver, this router, and a small
//! command task. The split is not decoration — it is what keeps the client
//! from deadlocking against itself.
//!
//! Both of the session layer's channels are bounded and both **block** rather
//! than drop. So if the router ever awaited a send on the session's *command*
//! channel while the driver was awaiting a send on the *event* channel, each
//! would be waiting for the other. Unsubscriptions triggered by a dropped
//! stream therefore leave the router immediately, on an unbounded channel, and
//! are issued by a third task that never touches the event channel. The router
//! itself only ever awaits sends *towards the caller*, which the caller alone
//! can unblock.
//!
//! # Why the two internal queues are unbounded
//!
//! Unbounded is a deliberate choice here and not an oversight, because neither
//! queue is a data path and both are bounded in practice by something the
//! caller has already paid for:
//!
//! - **Router commands.** One `Register` per [`crate::Client::subscribe`] call
//!   and one `StreamDropped` per [`crate::Updates`] dropped — so at most one
//!   message per subscription the caller asked for and one per subscription it
//!   gave up. `subscribe` then awaits the session's *bounded* command channel,
//!   which is what throttles a caller subscribing in a loop.
//! - **Unsubscriptions.** One per `StreamDropped`, and therefore bounded by
//!   the same count.
//!
//! Bounding them would make things worse, not better. The router must never
//! await the session's command channel — that is the deadlock this module is
//! shaped around — and `Updates::drop` cannot await at all, so a bounded queue
//! would force `try_send` and a dropped unsubscription, which is precisely the
//! silent leak the design exists to prevent. Neither queue grows with server
//! traffic, which is the dimension an untrusted peer controls.
//!
//! # Backpressure
//!
//! Every send towards a caller's stream is awaited, never dropped and never
//! deferred. A caller that stops polling one stream stalls the router, and
//! through it the driver and the socket. That is the documented contract of
//! [`Updates`](crate::Updates) and [`SessionEvents`](crate::SessionEvents),
//! and the reasoning is on those types.
//!
//! The single exception is an ordered stop, which is exempt from backpressure
//! for the same reason it is in the session layer: a caller that stopped
//! reading may stall the client, but must not be able to make it unstoppable.
//! Every awaited send here races the session's stop signal, and whatever had
//! not been delivered when it fires is discarded along with the streams it was
//! going to.

use std::collections::HashMap;

use tokio::sync::{mpsc, oneshot, watch};

use crate::client::ClientId;
use crate::client::events::{
    ClosedReason, Connected, Resubscribed, ServerInfo, SessionEvent, SubscriptionId,
};
use crate::client::message::MessageOutcome;
use crate::client::updates::SubscriptionEvent;
use crate::error::{Error, ServerError};
use crate::session::{
    ControlOutcome, ControlTarget, SessionEvent as WireEvent, SessionHandle, SubscriptionKey,
    SubscriptionOperation,
};
use crate::subscription::manager::SubscriptionManager;

/// What the client asks the router to do.
#[derive(Debug)]
pub(crate) enum RouterCommand {
    /// A new subscription's stream is ready to receive events.
    ///
    /// Always sent **before** the `add` that could produce an event for it is
    /// queued for the session, so that no notification can outrun its
    /// registration. See [`crate::Client::subscribe`].
    Register {
        /// Which subscription.
        id: SubscriptionId,
        /// Its item state and its interpretation of every notification.
        manager: Box<SubscriptionManager>,
        /// Where its events go.
        events: mpsc::Sender<SubscriptionEvent>,
    },

    /// The registration could not be turned into a request: undo it.
    ///
    /// Unlike [`RouterCommand::StreamDropped`] this asks for **no**
    /// unsubscription — nothing was ever subscribed.
    Unregister {
        /// Which subscription.
        id: SubscriptionId,
    },

    /// The caller dropped an [`Updates`](crate::Updates) stream, which means
    /// unsubscribe.
    StreamDropped {
        /// Which subscription.
        id: SubscriptionId,
    },
}

/// The session-level event a non-subscription control outcome deserves, if any.
///
/// The two failure modes stay apart all the way to the caller. A server
/// refusal carries the server's own code; a request that never left this
/// client carries no code at all, because inventing one would be
/// indistinguishable from a code a Metadata Adapter supplied
/// [`docs/spec/05-error-codes.md` §2].
fn control_failure(outcome: ControlOutcome) -> Option<SessionEvent> {
    match outcome {
        ControlOutcome::Rejected { cause } => {
            Some(SessionEvent::RequestRejected(ServerError::from(cause)))
        }
        ControlOutcome::NotSent { reason } => {
            tracing::warn!(reason, "a control request never reached the server");
            Some(SessionEvent::RequestNotSent { reason })
        }
        ControlOutcome::Accepted => None,
    }
}

/// One registered subscription.
#[derive(Debug)]
struct Registered {
    manager: SubscriptionManager,
    events: mpsc::Sender<SubscriptionEvent>,
}

/// The fan-out task.
#[derive(Debug)]
pub(crate) struct Router {
    /// The client this router belongs to, which every handle it publishes is
    /// stamped with. See [`SubscriptionId`].
    client: ClientId,
    /// Events from the session driver.
    session_events: mpsc::Receiver<WireEvent>,
    /// Requests from the client and from dropped streams.
    commands: mpsc::UnboundedReceiver<RouterCommand>,
    /// Where session-level events go, until the caller drops that stream.
    session_out: Option<mpsc::Sender<SessionEvent>>,
    /// Where unsubscriptions go, to be issued without blocking this task.
    unsubscribe: mpsc::UnboundedSender<SubscriptionKey>,
    /// Fired once, when the session first binds or first fails.
    ready: Option<oneshot::Sender<Result<Box<Connected>, Error>>>,
    /// Raised when the client is being torn down. The one thing that can
    /// interrupt a send towards a caller.
    stop: watch::Receiver<bool>,

    subscriptions: HashMap<SubscriptionKey, Registered>,
}

impl Router {
    /// Assembles the task. It does nothing until [`Router::run`] is polled.
    pub(crate) fn new(
        client: ClientId,
        session_events: mpsc::Receiver<WireEvent>,
        commands: mpsc::UnboundedReceiver<RouterCommand>,
        session_out: mpsc::Sender<SessionEvent>,
        unsubscribe: mpsc::UnboundedSender<SubscriptionKey>,
        ready: oneshot::Sender<Result<Box<Connected>, Error>>,
        stop: watch::Receiver<bool>,
    ) -> Self {
        Self {
            client,
            session_events,
            commands,
            session_out: Some(session_out),
            unsubscribe,
            ready: Some(ready),
            stop,
            subscriptions: HashMap::new(),
        }
    }

    /// Runs until the session ends.
    ///
    /// Not until the *client* ends: when the last [`crate::Client`] and
    /// [`Updates`](crate::Updates) are dropped this task stops taking commands
    /// but keeps forwarding, because the session it is fed from is shutting
    /// down at the same moment and its closing event is the one thing a caller
    /// holding [`SessionEvents`](crate::SessionEvents) is still owed. It ends
    /// when the driver's channel does, which is a bounded wait away.
    pub(crate) async fn run(mut self) {
        let mut taking_commands = true;
        loop {
            // Deliberately biased towards commands. A registration must be in
            // place before the first notification for its subscription is
            // routed, and a registration is always sent before the request
            // that could produce one has reached the wire.
            let done = tokio::select! {
                biased;
                command = self.commands.recv(), if taking_commands => match command {
                    Some(command) => {
                        self.on_command(command);
                        false
                    }
                    // The client and every stream are gone; the session is on
                    // its way out behind them.
                    None => {
                        taking_commands = false;
                        false
                    }
                },
                () = stopped(&mut self.stop) => true,
                event = self.session_events.recv() => match event {
                    Some(event) => self.on_session_event(event).await,
                    // The driver stopped without a closing event.
                    None => true,
                },
            };
            if done {
                break;
            }
        }

        // Whatever is left waiting learns that nothing more is coming: every
        // sender is dropped here, which ends every caller's stream.
        self.fail_ready(Error::Disconnected);
        tracing::debug!("event router stopped");
    }

    fn on_command(&mut self, command: RouterCommand) {
        match command {
            RouterCommand::Register {
                id,
                manager,
                events,
            } => {
                self.subscriptions.insert(
                    id.key(),
                    Registered {
                        manager: *manager,
                        events,
                    },
                );
            }

            RouterCommand::Unregister { id } => {
                self.subscriptions.remove(&id.key());
                tracing::debug!(id = id.get(), "registration rolled back");
            }

            // Unconditional, and deliberately not predicated on the removal
            // having found anything. The router also discovers a dropped
            // receiver when a send to it fails, and if that discovery removed
            // the registration first, a conditional unsubscribe here would
            // silently do nothing — leaving the server streaming into a
            // subscription whose public stream no longer exists. `Updates`
            // sends this exactly once, from its `Drop`, so it is the only
            // place the unsubscription is issued and it cannot be issued
            // twice.
            RouterCommand::StreamDropped { id } => {
                self.subscriptions.remove(&id.key());
                if self.unsubscribe.send(id.key()).is_err() {
                    tracing::debug!(id = id.get(), "cannot unsubscribe: the client has stopped");
                }
            }
        }
    }

    /// Handles one event from the session. Returns whether the router is done.
    async fn on_session_event(&mut self, event: WireEvent) -> bool {
        match event {
            WireEvent::Bound(info) => {
                let connected = Box::new(Connected::from(*info));
                self.signal_ready(connected.clone());
                self.emit(SessionEvent::Connected(connected)).await;
                false
            }

            WireEvent::Recovered(outcome) => {
                self.emit(SessionEvent::Recovered(outcome.into())).await;
                false
            }

            WireEvent::Resubscribed(entries) => {
                let client = self.client;
                let entries: Vec<Resubscribed> = entries
                    .into_iter()
                    .map(|entry| Resubscribed::from_entry(client, entry))
                    .collect();
                self.emit(SessionEvent::Resubscribed(entries)).await;
                false
            }

            WireEvent::Unbound { reason, retry_in } => {
                self.emit(SessionEvent::Disconnected {
                    reason: reason.into(),
                    retry_in,
                })
                .await;
                false
            }

            WireEvent::Message {
                sequence,
                prog,
                result,
                ..
            } => {
                self.emit(SessionEvent::Message(Box::new(MessageOutcome::new(
                    sequence, prog, result,
                ))))
                .await;
                false
            }

            WireEvent::Data {
                subscription,
                notification,
                ..
            } => {
                match subscription {
                    Some(key) => self.route(key, notification).await,
                    // A data notification naming no subscription. Message
                    // outcomes arrive as `WireEvent::Message` instead, so
                    // there is nothing left here to attribute; it is logged
                    // rather than invented into an event.
                    None => tracing::debug!(?notification, "unattributed data notification"),
                }
                false
            }

            WireEvent::ControlResponse {
                target, outcome, ..
            } => {
                self.on_control_response(target, outcome).await;
                false
            }

            WireEvent::ServerInfo(notification) => {
                if let Some(info) = ServerInfo::from_notification(notification) {
                    self.emit(SessionEvent::ServerInfo(info)).await;
                }
                false
            }

            WireEvent::Unparsed { line, error } => {
                tracing::debug!(%error, "surfacing a line this client cannot parse");
                self.emit(SessionEvent::Unrecognized { line }).await;
                false
            }

            WireEvent::Closed(closed) => {
                let reason = ClosedReason::from(closed);
                self.fail_ready(reason.clone().into_error());
                self.emit(SessionEvent::Closed(reason)).await;
                true
            }
        }
    }

    /// Routes a control response to whoever is waiting on its effect.
    ///
    /// A refusal that cannot reach the subscription it killed is a silently
    /// lost subscription, so the session layer names both the target and the
    /// **operation**, and this is where that naming is spent. The two
    /// distinctions that matter here:
    ///
    /// - *what* was attempted — a refused `add` means the subscription does
    ///   not exist, a refused `delete` means it very much still does;
    /// - *how far* the request got — the server **refusing** it is final,
    ///   whereas a request that never left this client is retried at the next
    ///   bind and so must not be reported as an ending.
    async fn on_control_response(&mut self, target: ControlTarget, outcome: ControlOutcome) {
        let ControlTarget::Subscription { key, operation } = target else {
            // `force_rebind`, `destroy`, `msg`, or a bare `ERROR` that names no
            // request at all. Nothing subscription-shaped to route it to.
            if let Some(event) = control_failure(outcome) {
                self.emit(event).await;
            }
            return;
        };

        match (operation, outcome) {
            // The server refused the `add`: the subscription was never
            // established and the session layer has dropped it from the
            // desired set, so it will not come back on a later session. This
            // is the one genuinely terminal case.
            (SubscriptionOperation::Subscribe, ControlOutcome::Rejected { cause }) => {
                let cause = ServerError::from(cause);
                tracing::warn!(key = key.get(), code = cause.code(), "subscription refused");
                match self.subscriptions.remove(&key) {
                    Some(entry) => {
                        deliver(
                            &entry.events,
                            SubscriptionEvent::Rejected(cause),
                            &mut self.stop,
                        )
                        .await;
                    }
                    None => self.emit(SessionEvent::RequestRejected(cause)).await,
                }
            }

            // The `add` never left this client. The session layer releases the
            // wire binding precisely so that the subscription is re-issued at
            // the next bind, so ending the stream here would lose a
            // subscription that is still coming — the opposite of what the
            // report is for. Non-terminal, and the registration stays.
            (SubscriptionOperation::Subscribe, ControlOutcome::NotSent { reason }) => {
                tracing::warn!(
                    key = key.get(),
                    reason,
                    "subscription request could not be sent; it will be retried at the next bind"
                );
                self.notify(key, SubscriptionEvent::Deferred { reason })
                    .await;
            }

            // The subscription is still alive: it is the caller's request to
            // change or remove it that failed. Reporting this on the
            // subscription's own stream would read as "your subscription is
            // gone", which is exactly backwards.
            (SubscriptionOperation::Unsubscribe | SubscriptionOperation::Reconfigure, outcome) => {
                // Either way the session layer has restored the entry, so the
                // subscription is still live and its stream keeps delivering.
                // What differs is whether a server ever saw the request, and
                // the caller is told which.
                match outcome {
                    ControlOutcome::Accepted => {}
                    ControlOutcome::Rejected { cause } => {
                        let cause = ServerError::from(cause);
                        tracing::warn!(
                            key = key.get(),
                            ?operation,
                            code = cause.code(),
                            "a subscription request was refused; the subscription is unchanged"
                        );
                        self.emit(SessionEvent::RequestRejected(cause)).await;
                    }
                    ControlOutcome::NotSent { reason } => {
                        tracing::warn!(
                            key = key.get(),
                            ?operation,
                            reason,
                            "a subscription request never reached the server; \
                             the subscription is unchanged"
                        );
                        self.emit(SessionEvent::RequestNotSent { reason }).await;
                    }
                }
            }

            // An accepted request is an acknowledgement, not news: the effect
            // the caller asked for is reported by the notification that
            // follows it — `SUBOK` for an `add`, `UNSUB` for a `delete`.
            (SubscriptionOperation::Subscribe, ControlOutcome::Accepted) => {}
        }
    }

    /// Sends one event to a subscription's stream without ending it.
    async fn notify(&mut self, key: SubscriptionKey, event: SubscriptionEvent) {
        let Some(entry) = self.subscriptions.get(&key) else {
            return;
        };
        let sender = entry.events.clone();
        deliver(&sender, event, &mut self.stop).await;
    }

    /// Sends one notification to the subscription it belongs to.
    async fn route(
        &mut self,
        key: SubscriptionKey,
        notification: crate::protocol::response::Notification,
    ) {
        let Some(entry) = self.subscriptions.get_mut(&key) else {
            // Registration precedes the request that could produce this, so an
            // unknown key means the stream is already gone: either it ended on
            // a terminal event or the caller dropped it. Nothing to deliver it
            // to, and nothing to hold it for.
            tracing::debug!(
                key = key.get(),
                "a notification arrived for a subscription with no stream"
            );
            return;
        };

        let event = match entry.manager.handle(&notification) {
            Ok(Some(event)) => SubscriptionEvent::from_wire(event),
            Ok(None) => return,
            Err(error) => {
                tracing::warn!(key = key.get(), %error, "a notification did not decode");
                SubscriptionEvent::Undecodable {
                    detail: error.to_string(),
                }
            }
        };
        let terminal = event.is_terminal();
        let sender = entry.events.clone();

        // Awaited, never dropped: see the module documentation. A failure
        // means the caller dropped the stream, and its `Drop` has already
        // queued the `StreamDropped` that both deregisters it and
        // unsubscribes — so the registration is deliberately left in place
        // here for that command to find.
        deliver(&sender, event, &mut self.stop).await;
        if terminal {
            self.subscriptions.remove(&key);
        }
    }

    /// Publishes a session-level event, applying backpressure.
    ///
    /// A caller that dropped [`SessionEvents`](crate::SessionEvents) is saying
    /// it does not want them; forwarding then stops for good rather than
    /// blocking the session on a receiver nobody holds.
    async fn emit(&mut self, event: SessionEvent) {
        let Some(sender) = self.session_out.clone() else {
            return;
        };
        if deliver(&sender, event, &mut self.stop).await == Delivery::ReceiverGone {
            tracing::debug!("session event stream dropped; no longer forwarding session events");
            self.session_out = None;
        }
    }

    /// Reports the first successful bind to whoever is awaiting `connect`.
    fn signal_ready(&mut self, connected: Box<Connected>) {
        if let Some(ready) = self.ready.take() {
            let _ = ready.send(Ok(connected));
        }
    }

    /// Reports to whoever is awaiting `connect` that it will never happen.
    fn fail_ready(&mut self, error: Error) {
        if let Some(ready) = self.ready.take() {
            let _ = ready.send(Err(error));
        }
    }
}

/// What became of one send towards a caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Delivery {
    /// It arrived.
    Delivered,
    /// The caller had dropped the receiving stream.
    ReceiverGone,
    /// A stop was ordered while waiting for room; the event was discarded.
    Stopped,
}

/// Sends one event towards a caller, waiting for room but not for ever.
///
/// The wait is the documented backpressure contract. The one thing that ends it
/// early is an ordered stop, because a client being torn down must not be held
/// open by a stream nobody is reading.
async fn deliver<T>(
    sender: &mpsc::Sender<T>,
    event: T,
    stop: &mut watch::Receiver<bool>,
) -> Delivery {
    tokio::select! {
        biased;
        permit = sender.reserve() => match permit {
            Ok(permit) => {
                permit.send(event);
                Delivery::Delivered
            }
            Err(_) => Delivery::ReceiverGone,
        },
        () = stopped(stop) => {
            tracing::debug!("a stop was ordered while a caller's stream was full");
            Delivery::Stopped
        }
    }
}

/// Resolves as soon as an ordered stop is signalled, or the session that could
/// signal one is gone.
async fn stopped(stop: &mut watch::Receiver<bool>) {
    loop {
        if *stop.borrow_and_update() {
            return;
        }
        if stop.changed().await.is_err() {
            return;
        }
    }
}

/// Issues the unsubscriptions that dropped streams asked for.
///
/// A task of its own for the reason in this module's documentation: it awaits
/// the session's bounded command channel, which the router must never do. It
/// holds the only [`SessionHandle`] besides the client's own, so it must stop
/// as soon as the client is dropped — which is what `shutdown` signals, by
/// being closed.
pub(crate) async fn issue_unsubscriptions(
    handle: SessionHandle,
    mut keys: mpsc::UnboundedReceiver<SubscriptionKey>,
    mut shutdown: mpsc::Receiver<()>,
    mut stop: watch::Receiver<bool>,
) {
    loop {
        let key = tokio::select! {
            key = keys.recv() => match key {
                Some(key) => key,
                None => break,
            },
            // Every sender is a live `Client`; when the last one is dropped
            // this resolves to `None` and the handle held here goes with it,
            // which is what lets the driver notice it has no owner left.
            _ = shutdown.recv() => break,
            () = stopped(&mut stop) => break,
        };
        // The session's command channel is bounded, so this can wait — but not
        // past a stop, or a driver that is itself blocked would keep this task
        // alive for good.
        let sent = tokio::select! {
            biased;
            result = handle.unsubscribe(key) => result.is_ok(),
            () = stopped(&mut stop) => break,
        };
        if !sent {
            tracing::debug!(key = key.get(), "the session stopped before unsubscribing");
            break;
        }
    }
    drop(handle);
    tracing::debug!("unsubscription task stopped");
}
