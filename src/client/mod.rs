//! The client: connect, subscribe, and get out of the way.
//!
//! This is the whole public surface of the crate's behaviour. Everything below
//! it — the session state machine, the wire parser, the transports — is
//! private, because it is not something a caller should have to reason about
//! to use a protocol client.
//!
//! # The shape of a program
//!
//! ```no_run
//! use futures_util::StreamExt;
//! use lightstreamer_rs::{
//!     AdapterSet, Client, ClientConfig, FieldSchema, ItemGroup, ServerAddress,
//!     Subscription, SubscriptionEvent, SubscriptionMode,
//! };
//!
//! # async fn run() -> lightstreamer_rs::Result<()> {
//! let config = ClientConfig::builder(ServerAddress::try_new("https://push.lightstreamer.com")?)
//!     .with_adapter_set(AdapterSet::try_new("DEMO")?)
//!     .build()?;
//!
//! let (client, _session_events) = Client::connect(config).await?;
//!
//! let mut updates = client
//!     .subscribe(Subscription::new(
//!         SubscriptionMode::Merge,
//!         ItemGroup::from_items(["item1"])?,
//!         FieldSchema::from_fields(["last_price"])?,
//!     ))
//!     .await?;
//!
//! while let Some(event) = updates.next().await {
//!     if let SubscriptionEvent::Update(update) = event {
//!         println!("{}: {:?}", update.item_name(), update.changed_fields());
//!     }
//! }
//! # Ok(())
//! # }
//! ```

mod events;
mod message;
mod router;
mod subscription;
mod updates;

pub use events::{
    ClosedReason, Connected, Continuity, DisconnectReason, Recovery, RecoveryOutcome, Resubscribed,
    ServerInfo, SessionEvent, SessionEvents, SubscriptionId,
};
pub use message::{Message, MessageOutcome, MessageResult, SequenceName};
pub use subscription::{
    BufferSize, FieldSchema, FrequencyLimit, ItemGroup, MaxFrequency, Snapshot, Subscription,
    SubscriptionMode,
};
pub use updates::{CommandFields, Filtering, SubscriptionEvent, UpdateFrequency, Updates};

use tokio::sync::{mpsc, oneshot};

use crate::config::{ClientConfig, Transport};
use crate::error::{Error, Result};
use crate::session::{self, SessionHandle};
use crate::subscription::manager::SubscriptionManager;
use crate::transport::ws::WsTransport;

use self::events::SessionEvents as SessionEventStream;
use self::router::{Router, RouterCommand};

/// A live Lightstreamer session.
///
/// Get one from [`Client::connect`]. It is a handle, not the session itself:
/// the work happens on background tasks, and the client is what you use to
/// steer them.
///
/// # Shutdown is part of the contract
///
/// **Dropping the client shuts the session down.** The background tasks stop,
/// the socket is closed, and every stream you are still holding ends. You
/// never need `std::process::exit` to stop a `lightstreamer-rs` client, and
/// you never need to remember a deregistration step.
///
/// Dropping does not tell the *server* the session is over: it just goes
/// quiet, and the server keeps the session buffering until its own timeout
/// expires [`docs/spec/02-session-lifecycle.md` §6.3]. Call
/// [`Client::disconnect`] when you want the server to know at once — for
/// instance because the same user is limited to a fixed number of sessions.
///
/// # Cloning
///
/// The client is **not** `Clone`, deliberately: its identity is what decides
/// when the session ends, and a clone would make "dropping the client shuts
/// the session down" untrue in a way that is hard to see at the call site. To
/// use it from several tasks, put it in an [`std::sync::Arc`] — the session
/// then ends when the last reference goes, which is the same rule stated once.
#[derive(Debug)]
pub struct Client {
    handle: SessionHandle,
    router: mpsc::UnboundedSender<RouterCommand>,
    update_capacity: std::num::NonZeroUsize,
    /// Held only to be dropped. Closing it is how the background tasks learn
    /// the client is gone, which is what makes "dropping the client shuts the
    /// session down" true.
    _alive: mpsc::Sender<()>,
}

impl Client {
    /// Opens a session and waits until it is streaming.
    ///
    /// Returns the client and the session's event stream. The stream is handed
    /// over here rather than fetched later so that ignoring it is a decision
    /// you write down — `drop(events)` — instead of an omission that quietly
    /// costs you the recovery notifications
    /// (`docs/adr/0005-recovery-is-visible-in-the-event-stream.md`).
    ///
    /// This resolves when the server has confirmed the session, so a
    /// [`Client`] you hold is one that was streaming at least once. It does
    /// **not** mean the connection is still up now: reconnection happens
    /// underneath, and the session event stream is where you learn about it.
    ///
    /// # Errors
    ///
    /// - [`Error::Transport`] if the connection could not be established at
    ///   all, or the address could not be turned into an endpoint.
    /// - [`Error::Session`] if the server refused the session with a code that
    ///   admits no retry — bad credentials (Appendix A code 1) and an unknown
    ///   Adapter Set (code 2) are the usual ones.
    /// - [`Error::ReconnectExhausted`] if every attempt allowed by
    ///   [`RetryPolicy`](crate::RetryPolicy) failed. Codes the specification
    ///   marks temporary are retried before this is reported.
    /// - [`Error::Internal`] if this crate could not build a request it should
    ///   always be able to build.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use lightstreamer_rs::{AdapterSet, Client, ClientConfig, ServerAddress};
    ///
    /// # async fn run() -> lightstreamer_rs::Result<()> {
    /// let config = ClientConfig::builder(ServerAddress::try_new("https://push.lightstreamer.com")?)
    ///     .with_adapter_set(AdapterSet::try_new("DEMO")?)
    ///     .build()?;
    ///
    /// let (client, events) = Client::connect(config).await?;
    /// # drop((client, events));
    /// # Ok(())
    /// # }
    /// ```
    #[tracing::instrument(skip(config), fields(address = %config.address(), transport = config.transport().as_str()))]
    pub async fn connect(config: ClientConfig) -> Result<(Self, SessionEvents)> {
        let update_capacity = config.options().update_capacity();
        let session_capacity = config.options().session_event_capacity();

        let transport = match config.transport() {
            Transport::WebSocket => WsTransport::try_new(config.address().as_str())?,
        };

        let options = config.into_session_options();
        let (driver, handle, session_events) =
            session::connect(transport, options).map_err(|error| Error::Internal {
                reason: error.to_string(),
            })?;

        let (router_tx, router_rx) = mpsc::unbounded_channel();
        let (unsubscribe_tx, unsubscribe_rx) = mpsc::unbounded_channel();
        let (session_out_tx, session_out_rx) = mpsc::channel(session_capacity.get());
        let (alive_tx, alive_rx) = mpsc::channel(1);
        let (ready_tx, ready_rx) = oneshot::channel();

        let router = Router::new(
            session_events,
            router_rx,
            session_out_tx,
            unsubscribe_tx,
            ready_tx,
        );

        tokio::spawn(async move {
            let closed = driver.run().await;
            tracing::debug!(?closed, "session driver stopped");
        });
        tokio::spawn(router.run());
        tokio::spawn(router::issue_unsubscriptions(
            handle.clone(),
            unsubscribe_rx,
            alive_rx,
        ));

        // The router fires this once, on the first bind or the first terminal
        // failure. A cancelled receiver means the router stopped without
        // either, which can only happen if everything was torn down first.
        let connected = match ready_rx.await {
            Ok(Ok(connected)) => connected,
            Ok(Err(error)) => return Err(error),
            Err(_) => return Err(Error::Disconnected),
        };
        tracing::info!(
            session = %connected.session_id,
            keepalive = ?connected.keepalive,
            "session established"
        );

        Ok((
            Self {
                handle,
                router: router_tx,
                update_capacity,
                _alive: alive_tx,
            },
            SessionEventStream::new(session_out_rx),
        ))
    }

    /// Subscribes to a set of items and returns the stream of what happens to
    /// them.
    ///
    /// The subscription is requested immediately and re-created automatically
    /// on any session that replaces a lost one, so the stream you get back
    /// survives a reconnection without any action from you. Whether the server
    /// accepted it is reported *on the stream*, as
    /// [`SubscriptionEvent::Activated`] or [`SubscriptionEvent::Rejected`] —
    /// this call does not wait for that round trip, because a caller usually
    /// wants to subscribe to several things and then start reading.
    ///
    /// # Errors
    ///
    /// [`Error::Disconnected`] if the session has already stopped. Everything
    /// the *server* has to say about the subscription arrives on the returned
    /// stream instead, where it can be interleaved correctly with the data.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use futures_util::StreamExt;
    /// use lightstreamer_rs::{
    ///     Client, FieldSchema, ItemGroup, Snapshot, Subscription, SubscriptionEvent,
    ///     SubscriptionMode,
    /// };
    ///
    /// # async fn run(client: &Client) -> lightstreamer_rs::Result<()> {
    /// let mut updates = client
    ///     .subscribe(
    ///         Subscription::new(
    ///             SubscriptionMode::Merge,
    ///             ItemGroup::from_items(["item1", "item2"])?,
    ///             FieldSchema::from_fields(["last_price", "time"])?,
    ///         )
    ///         .with_snapshot(Snapshot::On),
    ///     )
    ///     .await?;
    ///
    /// while let Some(event) = updates.next().await {
    ///     match event {
    ///         SubscriptionEvent::Update(update) => {
    ///             println!("{}: {:?}", update.item_name(), update.changed_fields());
    ///         }
    ///         SubscriptionEvent::Rejected(error) => {
    ///             eprintln!("the server refused it: {error}");
    ///             break;
    ///         }
    ///         _ => {}
    ///     }
    /// }
    /// # Ok(())
    /// # }
    /// ```
    #[tracing::instrument(skip(self, subscription), fields(mode = subscription.mode().as_str()))]
    pub async fn subscribe(&self, subscription: Subscription) -> Result<Updates> {
        let manager = SubscriptionManager::new(
            subscription.mode().to_wire(),
            subscription.snapshot().is_requested(),
            subscription.declared_items(),
            subscription.declared_fields(),
        );

        let (events_tx, events_rx) = mpsc::channel(self.update_capacity.get());
        let key = self
            .handle
            .subscribe(subscription.into_spec())
            .await
            .map_err(|_| Error::Disconnected)?;
        let id = SubscriptionId::new(key);

        // Unbounded, so this cannot block and cannot fail while the router
        // lives; the router takes commands ahead of session events, so the
        // registration is in place before anything can be routed to it.
        self.router
            .send(RouterCommand::Register {
                id,
                manager: Box::new(manager),
                events: events_tx,
            })
            .map_err(|_| Error::Disconnected)?;

        tracing::debug!(id = id.get(), "subscribed");
        Ok(Updates::new(id, events_rx, self.router.clone()))
    }

    /// Unsubscribes, and waits for the request to be handed to the session.
    ///
    /// Usually unnecessary: dropping the [`Updates`] stream unsubscribes on
    /// its own. Use this when you want the unsubscription to be ordered with
    /// respect to your other calls, or when you want to keep reading the
    /// stream until [`SubscriptionEvent::Unsubscribed`] arrives — after which
    /// the server sends nothing more for those items
    /// [`docs/spec/04-notifications.md` §3.4].
    ///
    /// Updates may still arrive between this call and that event; that is the
    /// protocol's behaviour, not a race in this crate.
    ///
    /// # Errors
    ///
    /// [`Error::Disconnected`] if the session has already stopped, in which
    /// case the subscription is gone regardless.
    pub async fn unsubscribe(&self, id: SubscriptionId) -> Result<()> {
        self.handle
            .unsubscribe(id.key())
            .await
            .map_err(|_| Error::Disconnected)
    }

    /// Sends a message to the server's Metadata Adapter
    /// [`docs/spec/03-requests.md` §12].
    ///
    /// `Ok(())` means the message was **queued for the session**, not that it
    /// reached the server. What actually happened to it is reported on the
    /// session event stream as [`SessionEvent::Message`] — not here, because
    /// the round trip runs through the Metadata Adapter and may take as long
    /// as the application behind it does.
    ///
    /// The guarantee is one report per message: **every message that asked for
    /// an outcome produces exactly one [`SessionEvent::Message`]**, whether it
    /// was processed, rejected by the adapter, refused by the server, or never
    /// sent at all. A [`Message::fire_and_forget`] one produces a report only
    /// in the last two cases, since it declined the notification the other two
    /// would have arrived on.
    ///
    /// Note that the server acknowledges *acceptance* and *processing*
    /// separately: acceptance is internal to this crate, and
    /// [`MessageResult::Delivered`] is the one that means the Metadata Adapter
    /// actually ran [`docs/spec/04-notifications.md` §4.1].
    ///
    /// # Messages are never resent
    ///
    /// If the session is replaced while a message is in flight, this crate
    /// reports [`MessageResult::NotSent`] rather than sending it again. The
    /// server deduplicates messages by their progressive number *within a
    /// session*, so replaying one across a replacement would restart the
    /// numbering it deduplicates against. Whether to retry is a decision only
    /// the caller can make correctly.
    ///
    /// # Errors
    ///
    /// [`Error::Disconnected`] if the session has already stopped.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use std::num::NonZeroU32;
    /// use lightstreamer_rs::{Client, Message, SequenceName};
    ///
    /// # async fn run(client: &Client) -> lightstreamer_rs::Result<()> {
    /// client
    ///     .send_message(
    ///         Message::new("BUY 100", NonZeroU32::MIN)
    ///             .in_sequence(SequenceName::try_new("ORDERS")?),
    ///     )
    ///     .await?;
    /// # Ok(())
    /// # }
    /// ```
    #[tracing::instrument(skip(self, message))]
    pub async fn send_message(&self, message: Message) -> Result<()> {
        self.handle
            .send_message(message.into_outgoing())
            .await
            .map_err(|_| Error::Disconnected)
    }

    /// Ends the session, telling the server so.
    ///
    /// The server closes the session at once instead of holding it open until
    /// its own timeout [`docs/spec/02-session-lifecycle.md` §6.3]. Every
    /// stream this client handed out ends shortly afterwards, and the last
    /// session event is a [`SessionEvent::Closed`].
    ///
    /// Dropping the client does the same thing *without* telling the server.
    /// Prefer this when the session limit per user matters, or when you want a
    /// clean line in the server's log.
    ///
    /// # Errors
    ///
    /// [`Error::Disconnected`] if the session had already stopped — which is
    /// the outcome you asked for, so it is usually safe to ignore.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn run(client: lightstreamer_rs::Client) {
    /// // An already-stopped session is not a failure worth propagating here.
    /// let _ = client.disconnect().await;
    /// # }
    /// ```
    #[tracing::instrument(skip(self))]
    pub async fn disconnect(self) -> Result<()> {
        self.handle
            .destroy(None)
            .await
            .map_err(|_| Error::Disconnected)
    }
}

#[cfg(test)]
mod tests {
    use futures_util::{FutureExt, StreamExt};
    use tokio::sync::{mpsc, oneshot};

    use super::*;
    use crate::client::events::Connected;
    use crate::client::router::Router;
    use crate::protocol::response::Notification;
    use crate::session::{
        BindKind, BoundInfo, SessionClosed as WireClosed, SessionEvent as WireEvent, SessionId,
    };
    use crate::subscription::manager::SubscriptionManager;
    use std::time::Duration;

    /// Everything a router test needs, wired but with no session behind it.
    struct Harness {
        /// Pretends to be the session driver.
        session_tx: mpsc::Sender<WireEvent>,
        commands: mpsc::UnboundedSender<RouterCommand>,
        session_events: SessionEvents,
        ready: oneshot::Receiver<Result<Box<Connected>>>,
        unsubscribed: mpsc::UnboundedReceiver<crate::session::SubscriptionKey>,
    }

    fn harness(session_capacity: usize) -> Harness {
        let (session_tx, session_rx) = mpsc::channel(16);
        let (commands_tx, commands_rx) = mpsc::unbounded_channel();
        let (out_tx, out_rx) = mpsc::channel(session_capacity);
        let (unsub_tx, unsub_rx) = mpsc::unbounded_channel();
        let (ready_tx, ready_rx) = oneshot::channel();

        let router = Router::new(session_rx, commands_rx, out_tx, unsub_tx, ready_tx);
        tokio::spawn(router.run());

        Harness {
            session_tx,
            commands: commands_tx,
            session_events: SessionEvents::new(out_rx),
            ready: ready_rx,
            unsubscribed: unsub_rx,
        }
    }

    fn bound(kind: BindKind) -> WireEvent {
        WireEvent::Bound(Box::new(BoundInfo {
            session_id: SessionId::new("S1"),
            kind,
            keep_alive: Duration::from_secs(5),
            request_limit_bytes: 50_000,
            control_link: None,
        }))
    }

    #[tokio::test]
    async fn test_router_reports_the_first_bind_to_connect() {
        let harness = harness(8);
        assert!(
            harness
                .session_tx
                .send(bound(BindKind::Created))
                .await
                .is_ok()
        );

        match harness.ready.await {
            Ok(Ok(connected)) => {
                assert_eq!(connected.session_id, "S1");
                assert!(matches!(connected.continuity, Continuity::New));
            }
            other => panic!("expected a successful bind, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_router_reports_a_terminal_failure_to_connect() {
        let harness = harness(8);
        let closed = WireClosed::ByServer {
            cause: crate::session::ServerCause {
                code: 1,
                message: "User/password check failed".to_owned(),
            },
        };
        assert!(
            harness
                .session_tx
                .send(WireEvent::Closed(closed))
                .await
                .is_ok()
        );

        match harness.ready.await {
            Ok(Err(Error::Session(cause))) => assert_eq!(cause.code(), 1),
            other => panic!("expected a session refusal, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_session_events_distinguish_the_three_recovery_outcomes() {
        let harness = harness(8);

        // Continuity preserved by a rebind.
        assert!(
            harness
                .session_tx
                .send(bound(BindKind::Rebound))
                .await
                .is_ok()
        );
        // The session was replaced by a new one.
        assert!(
            harness
                .session_tx
                .send(bound(BindKind::Recreated {
                    previous: Some(SessionId::new("S0"))
                }))
                .await
                .is_ok()
        );
        // The session is definitively gone.
        assert!(
            harness
                .session_tx
                .send(WireEvent::Closed(WireClosed::RetriesExhausted {
                    attempts: 8,
                    last: None
                }))
                .await
                .is_ok()
        );

        let mut events = harness.session_events;
        match events.next().await {
            Some(SessionEvent::Connected(connected)) => {
                assert!(connected.continuity.is_preserved());
                assert!(matches!(connected.continuity, Continuity::Preserved));
            }
            other => panic!("expected a preserved bind, got {other:?}"),
        }
        match events.next().await {
            Some(SessionEvent::Connected(connected)) => {
                assert!(!connected.continuity.is_preserved());
            }
            other => panic!("expected a replaced bind, got {other:?}"),
        }
        match events.next().await {
            Some(SessionEvent::Closed(ClosedReason::ReconnectExhausted { attempts: 8 })) => {}
            other => panic!("expected a definitive loss, got {other:?}"),
        }
        // Terminal: the stream ends.
        assert!(events.next().await.is_none());
    }

    #[tokio::test]
    async fn test_dropping_the_session_event_stream_does_not_stall_the_router() {
        // Capacity of one, and nobody reading: without the drop the router
        // would block on the second event forever.
        let harness = harness(1);
        drop(harness.session_events);

        for _ in 0..16 {
            assert!(
                harness
                    .session_tx
                    .send(bound(BindKind::Rebound))
                    .await
                    .is_ok(),
                "the router stopped consuming session events"
            );
        }
    }

    #[tokio::test]
    async fn test_dropping_an_update_stream_asks_for_an_unsubscription() {
        let mut harness = harness(8);
        let id = SubscriptionId::new(a_key().await);
        let (events_tx, events_rx) = mpsc::channel(4);
        assert!(
            harness
                .commands
                .send(RouterCommand::Register {
                    id,
                    manager: Box::new(SubscriptionManager::new(
                        crate::protocol::request::SubscriptionMode::Merge,
                        false,
                        vec!["item1".to_owned()],
                        vec!["price".to_owned()],
                    )),
                    events: events_tx,
                })
                .is_ok()
        );

        let updates = Updates::new(id, events_rx, harness.commands.clone());
        drop(updates);

        match harness.unsubscribed.recv().await {
            Some(key) => assert_eq!(key.get(), id.get()),
            None => panic!("dropping the stream did not unsubscribe"),
        }
    }

    #[tokio::test]
    async fn test_updates_are_bounded_and_block_rather_than_drop() {
        let harness = harness(8);
        let id = SubscriptionId::new(a_key().await);
        // One slot only: the second update has nowhere to go until the caller
        // reads, and the router must wait rather than discard it.
        let (events_tx, mut events_rx) = mpsc::channel(1);
        assert!(
            harness
                .commands
                .send(RouterCommand::Register {
                    id,
                    manager: Box::new(SubscriptionManager::new(
                        crate::protocol::request::SubscriptionMode::Merge,
                        false,
                        vec!["item1".to_owned()],
                        vec!["price".to_owned()],
                    )),
                    events: events_tx,
                })
                .is_ok()
        );

        assert!(
            harness
                .session_tx
                .send(data(
                    id,
                    Notification::SubscriptionOk {
                        subscription_id: 1,
                        item_count: 1,
                        field_count: 1,
                    }
                ))
                .await
                .is_ok()
        );
        for value in ["1.0", "2.0", "3.0"] {
            assert!(
                harness
                    .session_tx
                    .send(data(
                        id,
                        Notification::Update {
                            subscription_id: 1,
                            item_index: 1,
                            raw_values: value.to_owned(),
                        }
                    ))
                    .await
                    .is_ok()
            );
        }

        // Nothing was dropped: activation, then all three updates, in order.
        assert!(matches!(
            events_rx.recv().await,
            Some(SubscriptionEvent::Activated { field_count: 1, .. })
        ));
        for expected in ["1.0", "2.0", "3.0"] {
            match events_rx.recv().await {
                Some(SubscriptionEvent::Update(update)) => {
                    assert_eq!(
                        update.field(1).and_then(|value| value.text()),
                        Some(expected)
                    );
                }
                other => panic!("expected an update carrying {expected}, got {other:?}"),
            }
        }
    }

    #[tokio::test]
    async fn test_an_undecodable_notification_is_surfaced_not_swallowed() {
        let harness = harness(8);
        let id = SubscriptionId::new(a_key().await);
        let (events_tx, mut events_rx) = mpsc::channel(4);
        assert!(
            harness
                .commands
                .send(RouterCommand::Register {
                    id,
                    manager: Box::new(SubscriptionManager::new(
                        crate::protocol::request::SubscriptionMode::Merge,
                        false,
                        Vec::new(),
                        Vec::new(),
                    )),
                    events: events_tx,
                })
                .is_ok()
        );

        // An update before the subscription was ever activated: the manager
        // has no schema to decode it against.
        assert!(
            harness
                .session_tx
                .send(data(
                    id,
                    Notification::Update {
                        subscription_id: 1,
                        item_index: 1,
                        raw_values: "x".to_owned(),
                    }
                ))
                .await
                .is_ok()
        );

        assert!(matches!(
            events_rx.recv().await,
            Some(SubscriptionEvent::Undecodable { .. })
        ));
    }

    /// A refused control request, aimed at whatever it was acting on.
    fn refused(target: crate::session::ControlTarget, code: i64, message: &str) -> WireEvent {
        WireEvent::ControlResponse {
            request_id: None,
            target,
            outcome: crate::session::ControlOutcome::Rejected {
                cause: crate::session::ServerCause {
                    code,
                    message: message.to_owned(),
                },
            },
        }
    }

    #[tokio::test]
    async fn test_a_session_wide_rejection_reaches_the_session_stream() {
        let mut harness = harness(8);
        assert!(
            harness
                .session_tx
                .send(refused(
                    crate::session::ControlTarget::Session,
                    23,
                    "Bad Field schema name"
                ))
                .await
                .is_ok()
        );

        match harness.session_events.next().await {
            Some(SessionEvent::RequestRejected(error)) => {
                assert_eq!(error.code(), 23);
                assert_eq!(error.message(), "Bad Field schema name");
            }
            other => panic!("expected a rejection, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_a_request_that_never_left_is_not_dressed_as_a_server_error() {
        let mut harness = harness(8);
        assert!(
            harness
                .session_tx
                .send(WireEvent::ControlResponse {
                    request_id: None,
                    target: crate::session::ControlTarget::Session,
                    outcome: crate::session::ControlOutcome::NotSent {
                        reason: "the stream connection was down".to_owned(),
                    },
                })
                .await
                .is_ok()
        );

        // A fabricated code would be indistinguishable from one a Metadata
        // Adapter supplied, which this crate cannot honestly claim.
        match harness.session_events.next().await {
            Some(SessionEvent::RequestNotSent { reason }) => {
                assert_eq!(reason, "the stream connection was down");
            }
            other => panic!("expected a local failure, got {other:?}"),
        }
    }

    /// Registers a subscription stream and hands back its id and receiver.
    async fn register(
        harness: &Harness,
        capacity: usize,
    ) -> (SubscriptionId, mpsc::Receiver<SubscriptionEvent>) {
        let id = SubscriptionId::new(a_key().await);
        let (events_tx, events_rx) = mpsc::channel(capacity);
        assert!(
            harness
                .commands
                .send(RouterCommand::Register {
                    id,
                    manager: Box::new(SubscriptionManager::new(
                        crate::protocol::request::SubscriptionMode::Merge,
                        false,
                        vec!["item1".to_owned()],
                        vec!["price".to_owned()],
                    )),
                    events: events_tx,
                })
                .is_ok()
        );
        (id, events_rx)
    }

    #[tokio::test]
    async fn test_a_refused_subscription_is_reported_on_its_own_stream() {
        // The silent-loss case: without the target the caller's stream would
        // simply go quiet forever.
        let harness = harness(8);
        let (id, mut events) = register(&harness, 4).await;

        assert!(
            harness
                .session_tx
                .send(refused(
                    crate::session::ControlTarget::Subscription {
                        key: id.key(),
                        operation: crate::session::SubscriptionOperation::Subscribe,
                    },
                    21,
                    "Bad Item Group name"
                ))
                .await
                .is_ok()
        );

        match events.recv().await {
            Some(SubscriptionEvent::Rejected(error)) => {
                assert_eq!(error.code(), 21);
                assert_eq!(error.message(), "Bad Item Group name");
            }
            other => panic!("expected a rejection on the subscription stream, got {other:?}"),
        }
        // Terminal: the subscription is gone and the stream ends.
        assert!(events.recv().await.is_none());
    }

    #[tokio::test]
    async fn test_an_unsent_subscription_request_is_not_terminal() {
        // The session layer releases the wire binding and re-issues the
        // subscription at the next bind, so ending the stream here would lose
        // a subscription that is still coming.
        let harness = harness(8);
        let (id, mut events) = register(&harness, 4).await;

        assert!(
            harness
                .session_tx
                .send(WireEvent::ControlResponse {
                    request_id: None,
                    target: crate::session::ControlTarget::Subscription {
                        key: id.key(),
                        operation: crate::session::SubscriptionOperation::Subscribe,
                    },
                    outcome: crate::session::ControlOutcome::NotSent {
                        reason: "stream connection lost".to_owned(),
                    },
                })
                .await
                .is_ok()
        );

        match events.recv().await {
            Some(SubscriptionEvent::Deferred { reason }) => {
                assert_eq!(reason, "stream connection lost");
            }
            other => panic!("expected a non-terminal deferral, got {other:?}"),
        }

        // The registration survived: the retry's `SUBOK` still lands here.
        assert!(
            harness
                .session_tx
                .send(data(
                    id,
                    Notification::SubscriptionOk {
                        subscription_id: 2,
                        item_count: 1,
                        field_count: 1,
                    }
                ))
                .await
                .is_ok()
        );
        assert!(matches!(
            events.recv().await,
            Some(SubscriptionEvent::Activated { .. })
        ));
    }

    #[tokio::test]
    async fn test_an_accepted_control_request_produces_no_event() {
        let mut harness = harness(8);
        let (id, mut events) = register(&harness, 4).await;

        assert!(
            harness
                .session_tx
                .send(WireEvent::ControlResponse {
                    request_id: None,
                    target: crate::session::ControlTarget::Subscription {
                        key: id.key(),
                        operation: crate::session::SubscriptionOperation::Subscribe,
                    },
                    outcome: crate::session::ControlOutcome::Accepted,
                })
                .await
                .is_ok()
        );
        // An acceptance is an acknowledgement, not news: `SUBOK` is what says
        // the subscription is live, and that is what reaches the caller.
        assert!(
            harness
                .session_tx
                .send(data(
                    id,
                    Notification::SubscriptionOk {
                        subscription_id: 1,
                        item_count: 1,
                        field_count: 1,
                    }
                ))
                .await
                .is_ok()
        );
        assert!(matches!(
            events.recv().await,
            Some(SubscriptionEvent::Activated { .. })
        ));
        assert!(harness.session_events.next().now_or_never().is_none());
    }

    #[tokio::test]
    async fn test_a_refused_unsubscription_leaves_the_subscription_alive() {
        // A refused `delete` means the subscription is still active, so
        // reporting it on that stream would read as "your subscription is
        // gone" — the opposite of the truth.
        let mut harness = harness(8);
        let (id, mut events) = register(&harness, 4).await;

        assert!(
            harness
                .session_tx
                .send(refused(
                    crate::session::ControlTarget::Subscription {
                        key: id.key(),
                        operation: crate::session::SubscriptionOperation::Unsubscribe,
                    },
                    19,
                    "Specified subscription not found"
                ))
                .await
                .is_ok()
        );

        match harness.session_events.next().await {
            Some(SessionEvent::RequestRejected(error)) => assert_eq!(error.code(), 19),
            other => panic!("expected a session-level rejection, got {other:?}"),
        }

        // The stream is still live: an update still reaches it.
        assert!(
            harness
                .session_tx
                .send(data(
                    id,
                    Notification::SubscriptionOk {
                        subscription_id: 1,
                        item_count: 1,
                        field_count: 1,
                    }
                ))
                .await
                .is_ok()
        );
        assert!(matches!(
            events.recv().await,
            Some(SubscriptionEvent::Activated { .. })
        ));
    }

    #[tokio::test]
    async fn test_an_unparsable_line_is_surfaced_verbatim() {
        let mut harness = harness(8);
        assert!(
            harness
                .session_tx
                .send(WireEvent::Unparsed {
                    line: "FUTURE,1,2".to_owned(),
                    error: crate::protocol::ProtocolError::UnknownTag {
                        tag: "FUTURE".to_owned(),
                        line: "FUTURE,1,2".to_owned(),
                    },
                })
                .await
                .is_ok()
        );

        match harness.session_events.next().await {
            Some(SessionEvent::Unrecognized { line }) => assert_eq!(line, "FUTURE,1,2"),
            other => panic!("expected the raw line, got {other:?}"),
        }
    }

    fn data(id: SubscriptionId, notification: Notification) -> WireEvent {
        WireEvent::Data {
            progressive: 1,
            subscription: Some(id.key()),
            notification,
        }
    }

    /// A transport that connects to nothing and says nothing.
    ///
    /// The router tests need real [`SubscriptionKey`](crate::session::SubscriptionKey)
    /// values, which only the session layer can mint. A session is therefore
    /// created over this transport and its driver is never run: keys are
    /// allocated by `subscribe` before anything reaches the wire.
    #[derive(Debug)]
    struct NullTransport;

    impl crate::transport::Transport for NullTransport {
        fn properties(&self) -> crate::transport::TransportProperties {
            crate::transport::TransportProperties {
                control_shares_stream: true,
                ends_on_content_length: false,
                is_polling: false,
            }
        }

        fn set_control_link(&mut self, _host: Option<&str>) {}

        async fn open_stream(
            &mut self,
            _request: crate::transport::StreamOpen,
        ) -> std::result::Result<(), crate::transport::TransportError> {
            Ok(())
        }

        async fn next_line(
            &mut self,
        ) -> Option<std::result::Result<String, crate::transport::TransportError>> {
            std::future::pending().await
        }

        async fn send_control(
            &mut self,
            _request: crate::transport::EncodedRequest,
        ) -> std::result::Result<(), crate::transport::TransportError> {
            Ok(())
        }

        async fn close(&mut self) -> std::result::Result<(), crate::transport::TransportError> {
            Ok(())
        }
    }

    /// Mints `count` distinct subscription keys.
    async fn mint_keys(count: usize) -> Vec<crate::session::SubscriptionKey> {
        let options = crate::session::options::SessionOptions::default();
        let Ok((driver, handle, events)) = crate::session::connect(NullTransport, options) else {
            panic!("a default session configuration is valid");
        };
        let mut keys = Vec::with_capacity(count);
        for index in 0..count {
            let spec = crate::session::SubscriptionSpec::new(
                format!("item{index}"),
                "price",
                crate::protocol::request::SubscriptionMode::Merge,
            );
            match handle.subscribe(spec).await {
                Ok(key) => keys.push(key),
                Err(error) => panic!("the driver's command channel is open: {error}"),
            }
        }
        // The driver is deliberately never run; dropping it closes nothing the
        // tests rely on.
        drop((driver, events));
        keys
    }

    /// One subscription key.
    async fn a_key() -> crate::session::SubscriptionKey {
        match mint_keys(1).await.into_iter().next() {
            Some(key) => key,
            None => panic!("one key was requested"),
        }
    }
}
