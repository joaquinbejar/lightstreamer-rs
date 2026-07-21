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
//! // Not interested in session events: `drop` opts out. Binding them to
//! //`_session_events` would keep the stream alive and unread, which stalls
//! // the client once it fills — see `SessionEvents`.
//! let (client, session_events) = Client::connect(config).await?;
//! drop(session_events);
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
    ServerInfo, SessionEvent, SessionEvents, StateValidity, SubscriptionId,
};
pub use message::{Message, MessageOutcome, MessageResult, SequenceName};
pub use subscription::{
    BufferSize, FieldSchema, FrequencyLimit, ItemGroup, MaxFrequency, Snapshot, Subscription,
    SubscriptionMode,
};
pub use updates::{CommandFields, Filtering, SubscriptionEvent, UpdateFrequency, Updates};

use tokio::sync::{mpsc, oneshot};

use crate::config::ClientConfig;
use crate::error::{Error, Result};
use crate::session::{self, SessionHandle};

use self::events::SessionEvents as SessionEventStream;
use self::router::{Router, RouterCommand};

/// Hands out one identity per [`Client`] built in this process.
///
/// Starts above [`ClientId::DETACHED`] so that no real client can ever share
/// an identity with a hand-made [`crate::test_util`] handle.
static NEXT_CLIENT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// Which [`Client`] a [`SubscriptionId`] came from.
///
/// Not exposed: a caller has no use for it, and its whole job is to make a
/// handle from one client unusable on another. See [`SubscriptionId`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct ClientId(u64);

impl ClientId {
    /// The identity of a handle that belongs to no client, which
    /// [`crate::test_util`] mints. It matches no real client, so such a handle
    /// can name a subscription in an assertion but never cancel one.
    #[cfg(feature = "test-util")]
    pub(crate) const DETACHED: Self = Self(0);

    /// The next unused identity.
    ///
    /// Fails rather than wrapping. Reaching this needs 2⁶⁴ clients in one
    /// process, but a wrapped identity would silently make two clients'
    /// handles interchangeable again, which is the whole thing being
    /// prevented.
    fn allocate() -> Result<Self> {
        use std::sync::atomic::Ordering::Relaxed;
        NEXT_CLIENT_ID
            .fetch_update(Relaxed, Relaxed, |current| current.checked_add(1))
            .map(Self)
            .map_err(|_| Error::Internal {
                reason: "this process has run out of client identities".to_owned(),
            })
    }

    /// The identity as a number, for `Display` and for logs.
    pub(crate) const fn get(self) -> u64 {
        self.0
    }
}

/// The channel sizes and budgets a client is built with.
#[derive(Debug, Clone, Copy)]
struct Capacities {
    /// Events buffered per subscription stream.
    update: std::num::NonZeroUsize,
    /// Events buffered on the session stream.
    session_event: std::num::NonZeroUsize,
    /// How long [`Client::disconnect`] waits before forcing a stop.
    shutdown_timeout: std::time::Duration,
}

/// Holds one session event produced before the caller could hold the stream.
///
/// The buffer is capped at the capacity the caller configured, which is the
/// budget it already said it wanted for session events. Past that, further
/// pre-connection events are **discarded with a warning**: they are session
/// events for a session that has not bound yet, whose outcome
/// [`Client::connect`] reports as its own return value, and blocking instead
/// would deadlock the bind against a stream nobody can yet read.
fn stage(staged: &mut std::collections::VecDeque<SessionEvent>, event: SessionEvent, limit: usize) {
    if staged.len() < limit {
        staged.push_back(event);
        return;
    }
    tracing::warn!(
        limit,
        "discarding a session event produced before connect() returned"
    );
}

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
    /// This client's identity, carried by every [`SubscriptionId`] it hands
    /// out so that another client's handle cannot be spent here.
    id: ClientId,
    handle: SessionHandle,
    router: mpsc::UnboundedSender<RouterCommand>,
    update_capacity: std::num::NonZeroUsize,
    /// How long [`Client::disconnect`] waits for the ordered shutdown before
    /// forcing one. Taken from
    /// [`ConnectionOptions::with_open_timeout`](crate::ConnectionOptions::with_open_timeout),
    /// which is already this crate's answer to "how long may one server round
    /// trip take"; a destroy is exactly one.
    shutdown_timeout: std::time::Duration,
    /// The background tasks, so that [`Client::disconnect`] can wait for them
    /// instead of merely asking them to stop.
    tasks: Vec<tokio::task::JoinHandle<()>>,
    /// Whether dropping this client should order an immediate stop.
    ///
    /// Always true except while [`Client::disconnect`] is running its ordered
    /// shutdown, which needs the session to survive long enough to tell the
    /// server.
    stop_on_drop: bool,
    /// Held only to be dropped. Closing it is how the background tasks learn
    /// the client is gone, which is what makes "dropping the client shuts the
    /// session down" true.
    _alive: mpsc::Sender<()>,
}

impl Drop for Client {
    /// Shuts the session down.
    ///
    /// Closing the channels the background tasks watch is not enough on its
    /// own: a task blocked delivering into a stream the caller stopped reading
    /// is not watching anything. The explicit stop is what makes "dropping the
    /// client shuts the session down" true even then — it is exempt from
    /// backpressure by construction, and whatever was still undelivered goes
    /// with the streams it was going to.
    fn drop(&mut self) {
        if self.stop_on_drop {
            self.handle.stop();
        }
    }
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
    ///   marks temporary are retried before this is reported, and the last
    ///   failure is carried in the error rather than reduced to a count.
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
        let capacities = Capacities {
            update: config.options().update_capacity(),
            session_event: config.options().session_event_capacity(),
            shutdown_timeout: config.options().open_timeout(),
        };

        // Which transport carries the session, and how it is built, is the
        // session layer's composition root — not this one. The façade names no
        // adapter, so a second transport is added below without touching the
        // semver-governed surface
        // [`docs/adr/0007-transport-port-shape.md` §3].
        let parts = session::connect_configured(config)?;
        Self::assemble(parts, capacities).await
    }

    /// Wires the driver, the router and the unsubscriber together and waits
    /// for the first bind.
    ///
    /// Generic over the transport so that the whole client — not merely the
    /// session layer — can be exercised over a scripted one, with no socket and
    /// no real timer [`docs/adr/0007-transport-port-shape.md`]. It takes the
    /// session already composed, because choosing and building a transport is
    /// not this layer's business.
    async fn assemble<T>(
        parts: (
            session::SessionDriver<T>,
            SessionHandle,
            mpsc::Receiver<session::SessionEvent>,
        ),
        capacities: Capacities,
    ) -> Result<(Self, SessionEvents)>
    where
        T: session::Transport + Send + 'static,
    {
        let Capacities {
            update: update_capacity,
            session_event: session_capacity,
            shutdown_timeout,
        } = capacities;
        let id = ClientId::allocate()?;
        let (driver, handle, session_events) = parts;

        let (router_tx, router_rx) = mpsc::unbounded_channel();
        let (unsubscribe_tx, unsubscribe_rx) = mpsc::unbounded_channel();
        let (session_out_tx, mut session_out_rx) = mpsc::channel(session_capacity.get());
        let (alive_tx, alive_rx) = mpsc::channel(1);
        let (ready_tx, ready_rx) = oneshot::channel();

        let router = Router::new(
            id,
            session_events,
            router_rx,
            session_out_tx,
            unsubscribe_tx,
            ready_tx,
            handle.stop_signal(),
        );

        let tasks = vec![
            tokio::spawn(async move {
                let closed = driver.run().await;
                tracing::debug!(?closed, "session driver stopped");
            }),
            tokio::spawn(router.run()),
            tokio::spawn(router::issue_unsubscriptions(
                handle.clone(),
                unsubscribe_rx,
                alive_rx,
                handle.stop_signal(),
            )),
        ];

        // The router fires readiness once, on the first bind or the first
        // terminal failure. Waiting for it *while draining* the event channel
        // is not an optimisation: nobody holds the event stream yet, so a
        // startup that retries can fill the channel, and a router blocked on a
        // full channel never processes the bind that readiness is waiting for.
        // Draining here is what stops `connect` waiting on itself.
        let mut staged = std::collections::VecDeque::new();
        tokio::pin!(ready_rx);
        let outcome = loop {
            tokio::select! {
                biased;
                result = &mut ready_rx => break result,
                event = session_out_rx.recv() => match event {
                    Some(event) => stage(&mut staged, event, session_capacity.get()),
                    // The router stopped without saying anything, which can
                    // only happen if everything was torn down first.
                    None => break Ok(Err(Error::Disconnected)),
                },
            }
        };
        let connected = match outcome {
            Ok(Ok(connected)) => connected,
            Ok(Err(error)) => return Err(error),
            Err(_) => return Err(Error::Disconnected),
        };
        // Deliberately without the session identifier. `LS_session` is what a
        // control request or a rebind names the session with
        // [`docs/spec/03-requests.md` §5.1], so it is a bearer value, and this
        // crate does not put one in a log line on a caller's behalf. It is on
        // `Connected` for a caller who wants to.
        tracing::info!(
            client = id.get(),
            keepalive = ?connected.keepalive,
            request_limit_bytes = connected.request_limit_bytes,
            "session established"
        );

        Ok((
            Self {
                id,
                handle,
                router: router_tx,
                update_capacity,
                shutdown_timeout,
                tasks,
                stop_on_drop: true,
                _alive: alive_tx,
            },
            SessionEventStream::with_staged(staged, session_out_rx),
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
    /// - [`Error::Config`] if the subscription's options and its mode cannot
    ///   go together — see [`Subscription::validate`], which this calls before
    ///   anything is sent. Checking here rather than at encoding time is what
    ///   makes an impossible subscription a mistake you see at the call site
    ///   instead of a stream that quietly never activates.
    /// - [`Error::Disconnected`] if the session has already stopped.
    /// - [`Error::Internal`] if this client has run out of the identifiers it
    ///   names subscriptions by — which takes 2⁶⁴ of them, and is reported
    ///   rather than wrapped around because a reused identifier would silently
    ///   conflate two subscriptions.
    ///
    /// Everything the *server* has to say about the subscription arrives on
    /// the returned stream instead, where it can be interleaved correctly with
    /// the data.
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
        // Before anything is allocated or registered: a combination the mode
        // does not admit is the caller's mistake, and it is one here rather
        // than a `Deferred` on a stream that never activates.
        subscription.validate()?;

        let (events_tx, events_rx) = mpsc::channel(self.update_capacity.get());
        // The key is allocated, and everything it names registered, *before*
        // the request that could produce a notification for it is queued. The
        // other order leaves a window in which an immediate `SUBOK`, an early
        // update, or a `REQERR` refusing the subscription outright arrives with
        // nowhere to go [`docs/spec/03-requests.md` §13] — and a refusal that
        // lands in that window would leave the caller holding a stream that
        // never receives its own terminal event.
        //
        // The router's channel is unbounded, so this cannot block and cannot
        // fail while the router lives, and the router takes its commands ahead
        // of session events.
        let key = self
            .handle
            .allocate_key()
            .map_err(|error| Error::Internal {
                reason: error.to_string(),
            })?;
        let id = SubscriptionId::new(self.id, key);
        self.router
            .send(RouterCommand::Register {
                id,
                events: events_tx,
            })
            .map_err(|_| Error::Disconnected)?;

        if let Err(error) = self
            .handle
            .subscribe_with_key(key, subscription.into_spec())
            .await
        {
            // Nothing was subscribed, so nothing is unsubscribed: the
            // registration is simply undone.
            tracing::debug!(id = id.get(), %error, "the subscription could not be queued");
            let _ = self.router.send(RouterCommand::Unregister { id });
            return Err(Error::Disconnected);
        }

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
    /// - [`Error::ForeignSubscription`] if the handle was created by a
    ///   different client. Nothing is sent: every client numbers its
    ///   subscriptions from one, so the same number names different data on
    ///   each of them, and acting on it would cancel somebody else's.
    /// - [`Error::Disconnected`] if the session has already stopped, in which
    ///   case the subscription is gone regardless.
    pub async fn unsubscribe(&self, id: SubscriptionId) -> Result<()> {
        if id.client() != self.id {
            return Err(Error::ForeignSubscription { id: id.get() });
        }
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
    /// - [`Error::Config`] if the message's parts cannot be sent together —
    ///   see [`Message::validate`], which this calls first.
    /// - [`Error::Disconnected`] if the session has already stopped.
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
        // The text is the caller's and may be anything, so nothing about the
        // message is logged; what is checked here is only its shape.
        message.validate()?;
        self.handle
            .send_message(message.into_outgoing())
            .await
            .map_err(|_| Error::Disconnected)
    }

    /// Ends the session, telling the server so, and waits until it is over.
    ///
    /// The server closes the session at once instead of holding it open until
    /// its own timeout [`docs/spec/02-session-lifecycle.md` §6.3]. When this
    /// returns, the `destroy` has been written, the socket has been closed, and
    /// every background task this client owns has finished — so a caller may
    /// shut its runtime down immediately afterwards without cutting the
    /// shutdown short. Every stream this client handed out has ended, the last
    /// session event being a [`SessionEvent::Closed`].
    ///
    /// Dropping the client instead ends the session *without* telling the
    /// server, and without waiting. Prefer this call when the session limit per
    /// user matters, or when you want a clean line in the server's log.
    ///
    /// # The wait is bounded
    ///
    /// A server that never answers, or a stream the caller stopped reading,
    /// cannot make this hang: after
    /// [`ConnectionOptions::with_open_timeout`](crate::ConnectionOptions::with_open_timeout)
    /// — the same budget one server round trip is given anywhere else — the
    /// shutdown stops being polite and the tasks are stopped regardless. The
    /// socket is closed on that path too.
    ///
    /// # Errors
    ///
    /// [`Error::Disconnected`] if the session had already stopped — which is
    /// the outcome you asked for, so it is usually safe to ignore.
    /// [`Error::Internal`] if the session did not finish within the budget
    /// above; it has been stopped anyway, and nothing is left running.
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
    pub async fn disconnect(mut self) -> Result<()> {
        // From here on, dropping this value must not cut the session short:
        // the point of the call is to let it finish saying goodbye.
        self.stop_on_drop = false;
        let budget = self.shutdown_timeout;
        let mut tasks = std::mem::take(&mut self.tasks);
        let handle = self.handle.clone();

        // Asking is itself bounded. The command channel is bounded, and a
        // driver blocked delivering into a stream nobody is reading must not
        // be able to swallow the request to end it.
        let asked = match tokio::time::timeout(budget, self.handle.destroy(None)).await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(_)) => Err(Error::Disconnected),
            Err(_) => Err(Error::Internal {
                reason: "the session did not accept the destroy request in time".to_owned(),
            }),
        };

        // Everything that keeps the background tasks alive goes now: the
        // router's command channel, the liveness token, and this client's own
        // session handle. What is left is the ordered shutdown itself.
        drop(self);

        let mut outcome = asked;
        if tokio::time::timeout(budget, join_all(&mut tasks))
            .await
            .is_err()
        {
            tracing::warn!(
                ?budget,
                "the session did not shut down in time; stopping it"
            );
            // Exempt from backpressure by construction: this is what a stream
            // the caller stopped reading cannot hold up.
            handle.stop();
            if tokio::time::timeout(budget, join_all(&mut tasks))
                .await
                .is_err()
            {
                for task in &tasks {
                    task.abort();
                }
                join_all(&mut tasks).await;
            }
            if outcome.is_ok() {
                outcome = Err(Error::Internal {
                    reason: "the session did not shut down within the configured budget".to_owned(),
                });
            }
        }
        drop(handle);
        outcome
    }
}

/// Waits for every task, keeping the ones not yet finished.
///
/// Written to be cancel-safe: a handle is removed only once it has resolved,
/// so a caller that times this out still owns whatever is still running and can
/// escalate rather than detach it.
async fn join_all(tasks: &mut Vec<tokio::task::JoinHandle<()>>) {
    while let Some(task) = tasks.last_mut() {
        if let Err(error) = task.await
            && !error.is_cancelled()
        {
            tracing::warn!(%error, "a client task did not finish cleanly");
        }
        tasks.pop();
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use futures_util::{FutureExt, StreamExt};
    use tokio::sync::{mpsc, oneshot};

    use super::*;
    use crate::client::events::Connected;
    use crate::client::router::Router;
    use crate::session::{
        BindKind, BoundInfo, SessionClosed as WireClosed, SessionEvent as WireEvent, SessionId,
        SubscriptionError, SubscriptionEvent as WireSubscriptionEvent,
    };
    use std::time::Duration;

    /// A fresh client identity, as [`Client::assemble`] would allocate one.
    fn a_client_id() -> ClientId {
        match ClientId::allocate() {
            Ok(id) => id,
            Err(error) => panic!("this process has client identities left: {error}"),
        }
    }

    /// Everything a router test needs, wired but with no session behind it.
    struct Harness {
        /// The identity the router stamps its handles with.
        client: ClientId,
        /// Pretends to be the session driver.
        session_tx: mpsc::Sender<WireEvent>,
        commands: mpsc::UnboundedSender<RouterCommand>,
        session_events: SessionEvents,
        ready: oneshot::Receiver<Result<Box<Connected>>>,
        unsubscribed: mpsc::UnboundedReceiver<crate::session::SubscriptionKey>,
        /// Raises the stop lane, as a client being torn down would.
        stop: tokio::sync::watch::Sender<bool>,
    }

    fn harness(session_capacity: usize) -> Harness {
        let (session_tx, session_rx) = mpsc::channel(16);
        let (commands_tx, commands_rx) = mpsc::unbounded_channel();
        let (out_tx, out_rx) = mpsc::channel(session_capacity);
        let (unsub_tx, unsub_rx) = mpsc::unbounded_channel();
        let (ready_tx, ready_rx) = oneshot::channel();
        let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);

        let client = a_client_id();
        let router = Router::new(
            client,
            session_rx,
            commands_rx,
            out_tx,
            unsub_tx,
            ready_tx,
            stop_rx,
        );
        tokio::spawn(router.run());

        Harness {
            client,
            session_tx,
            commands: commands_tx,
            session_events: SessionEvents::with_staged(std::collections::VecDeque::new(), out_rx),
            ready: ready_rx,
            unsubscribed: unsub_rx,
            stop: stop_tx,
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
                assert_eq!(connected.continuity.state_validity(), StateValidity::Valid);
                assert!(matches!(connected.continuity, Continuity::Preserved));
            }
            other => panic!("expected a preserved bind, got {other:?}"),
        }
        match events.next().await {
            Some(SessionEvent::Connected(connected)) => {
                assert_eq!(
                    connected.continuity.state_validity(),
                    StateValidity::Invalid
                );
            }
            other => panic!("expected a replaced bind, got {other:?}"),
        }
        match events.next().await {
            Some(SessionEvent::Closed(ClosedReason::ReconnectExhausted {
                attempts: 8, ..
            })) => {}
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
        let id = SubscriptionId::new(harness.client, a_key().await);
        let (events_tx, events_rx) = mpsc::channel(4);
        assert!(
            harness
                .commands
                .send(RouterCommand::Register {
                    id,
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
        let id = SubscriptionId::new(harness.client, a_key().await);
        // One slot only: the second update has nowhere to go until the caller
        // reads, and the router must wait rather than discard it.
        let (events_tx, mut events_rx) = mpsc::channel(1);
        assert!(
            harness
                .commands
                .send(RouterCommand::Register {
                    id,
                    events: events_tx,
                })
                .is_ok()
        );

        assert!(
            harness
                .session_tx
                .send(data(
                    id,
                    WireSubscriptionEvent::Activated {
                        item_count: 1,
                        field_count: 1,
                        command_fields: None,
                    }
                ))
                .await
                .is_ok()
        );
        for dropped_count in [1, 2, 3] {
            assert!(
                harness
                    .session_tx
                    .send(data(
                        id,
                        WireSubscriptionEvent::Overflow {
                            item_index: 1,
                            dropped_count,
                        }
                    ))
                    .await
                    .is_ok()
            );
        }

        // Nothing was dropped: activation, then all three events, in order.
        assert!(matches!(
            events_rx.recv().await,
            Some(SubscriptionEvent::Activated { field_count: 1, .. })
        ));
        for expected in [1, 2, 3] {
            match events_rx.recv().await {
                Some(SubscriptionEvent::Overflow { dropped_count, .. }) => {
                    assert_eq!(dropped_count, expected);
                }
                other => panic!("expected an overflow of {expected}, got {other:?}"),
            }
        }
    }

    #[tokio::test]
    async fn test_an_undecodable_notification_is_surfaced_not_swallowed() {
        let harness = harness(8);
        let id = SubscriptionId::new(harness.client, a_key().await);
        let (events_tx, mut events_rx) = mpsc::channel(4);
        assert!(
            harness
                .commands
                .send(RouterCommand::Register {
                    id,
                    events: events_tx,
                })
                .is_ok()
        );

        // An update before the subscription was ever activated: the session
        // had no schema to decode it against, and says so in a typed error.
        assert!(
            harness
                .session_tx
                .send(undecodable(
                    id,
                    SubscriptionError::NotActivated { tag: "U" }
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
        let id = SubscriptionId::new(harness.client, a_key().await);
        let (events_tx, events_rx) = mpsc::channel(capacity);
        assert!(
            harness
                .commands
                .send(RouterCommand::Register {
                    id,
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
                    WireSubscriptionEvent::Activated {
                        item_count: 1,
                        field_count: 1,
                        command_fields: None,
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
                    WireSubscriptionEvent::Activated {
                        item_count: 1,
                        field_count: 1,
                        command_fields: None,
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
                    WireSubscriptionEvent::Activated {
                        item_count: 1,
                        field_count: 1,
                        command_fields: None,
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
                    error: crate::session::ProtocolError::UnknownTag {
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

    /// One subscription event as the session layer would hand it up.
    ///
    /// The session interprets the wire line and routes the *meaning*; these
    /// tests are about the fan-out, so they start where the router does.
    fn data(id: SubscriptionId, event: WireSubscriptionEvent) -> WireEvent {
        WireEvent::Subscription {
            progressive: 1,
            key: id.key(),
            outcome: Box::new(Ok(event)),
        }
    }

    /// A line the session could not reconcile with this subscription's state.
    fn undecodable(id: SubscriptionId, error: SubscriptionError) -> WireEvent {
        WireEvent::Subscription {
            progressive: 1,
            key: id.key(),
            outcome: Box::new(Err(error)),
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

    impl session::Transport for NullTransport {
        fn properties(&self) -> session::TransportProperties {
            session::TransportProperties {
                control_shares_stream: true,
                ends_on_content_length: false,
                is_polling: false,
            }
        }

        fn set_control_link(&mut self, _host: Option<&str>) {}

        async fn open_stream(
            &mut self,
            _request: session::StreamOpen,
        ) -> std::result::Result<(), crate::error::TransportError> {
            Ok(())
        }

        async fn next_line(
            &mut self,
        ) -> Option<std::result::Result<String, crate::error::TransportError>> {
            std::future::pending().await
        }

        async fn send_control(
            &mut self,
            _request: session::EncodedRequest,
        ) -> std::result::Result<(), crate::error::TransportError> {
            Ok(())
        }

        async fn close(&mut self) -> std::result::Result<(), crate::error::TransportError> {
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
                session::SubscriptionMode::Merge,
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

    // -----------------------------------------------------------------------
    // Startup, shutdown and registration ordering
    // -----------------------------------------------------------------------

    /// A transport that replays lines and records what it was asked to do.
    ///
    /// The whole client is exercised over this: driver, router and
    /// unsubscriber, with no socket and no real timer
    /// [`docs/adr/0007-transport-port-shape.md`].
    #[derive(Debug, Clone, Default)]
    struct ScriptLog {
        controls: Vec<String>,
        closes: usize,
    }

    /// One thing a scripted connection does when asked for a line.
    #[derive(Debug, Clone, Copy)]
    enum Say {
        /// Yield this line.
        Line(&'static str),
        /// Yield nothing until this many control requests have been sent, so a
        /// script can answer what the client actually did.
        AwaitControls(usize),
    }

    #[derive(Debug)]
    struct ScriptedTransport {
        /// One script per stream connection; `None` for a connection that
        /// cannot be opened at all.
        scripts: std::collections::VecDeque<Option<Vec<Say>>>,
        current: std::collections::VecDeque<Say>,
        log: std::sync::Arc<std::sync::Mutex<ScriptLog>>,
    }

    impl ScriptedTransport {
        fn new(scripts: Vec<Option<Vec<Say>>>) -> Self {
            Self {
                scripts: scripts.into(),
                current: std::collections::VecDeque::new(),
                log: std::sync::Arc::new(std::sync::Mutex::new(ScriptLog::default())),
            }
        }

        fn log(&self) -> std::sync::Arc<std::sync::Mutex<ScriptLog>> {
            std::sync::Arc::clone(&self.log)
        }
    }

    impl session::Transport for ScriptedTransport {
        fn properties(&self) -> session::TransportProperties {
            session::TransportProperties {
                control_shares_stream: true,
                ends_on_content_length: false,
                is_polling: false,
            }
        }

        fn set_control_link(&mut self, _host: Option<&str>) {}

        async fn open_stream(
            &mut self,
            _request: session::StreamOpen,
        ) -> std::result::Result<(), crate::error::TransportError> {
            match self.scripts.pop_front() {
                Some(Some(script)) => {
                    self.current = script.into();
                    Ok(())
                }
                Some(None) => Err(crate::error::TransportError::ConnectionLost {
                    reason: "scripted failure".to_owned(),
                }),
                None => {
                    self.current.clear();
                    Ok(())
                }
            }
        }

        async fn next_line(
            &mut self,
        ) -> Option<std::result::Result<String, crate::error::TransportError>> {
            loop {
                match self.current.front().copied() {
                    Some(Say::Line(line)) => {
                        self.current.pop_front();
                        return Some(Ok(line.to_owned()));
                    }
                    Some(Say::AwaitControls(wanted)) => {
                        let sent = self.log.lock().map_or(0, |log| log.controls.len());
                        if sent >= wanted {
                            self.current.pop_front();
                            continue;
                        }
                        return std::future::pending().await;
                    }
                    None => return std::future::pending().await,
                }
            }
        }

        async fn send_control(
            &mut self,
            request: session::EncodedRequest,
        ) -> std::result::Result<(), crate::error::TransportError> {
            if let Ok(mut log) = self.log.lock() {
                log.controls.push(request.parameters);
            }
            Ok(())
        }

        async fn close(&mut self) -> std::result::Result<(), crate::error::TransportError> {
            if let Ok(mut log) = self.log.lock() {
                log.closes = log.closes.saturating_add(1);
            }
            Ok(())
        }
    }

    const CONOK: &str = "CONOK,S1,50000,5000,*";

    fn capacities(session_event: usize) -> Capacities {
        Capacities {
            update: std::num::NonZeroUsize::new(8).unwrap_or(std::num::NonZeroUsize::MIN),
            session_event: std::num::NonZeroUsize::new(session_event)
                .unwrap_or(std::num::NonZeroUsize::MIN),
            shutdown_timeout: Duration::from_secs(5),
        }
    }

    fn session_options() -> crate::session::options::SessionOptions {
        crate::session::options::SessionOptions::default().with_backoff(
            crate::session::backoff::BackoffPolicy {
                initial: Duration::from_millis(10),
                max: Duration::from_millis(40),
                max_attempts: std::num::NonZeroU32::new(6),
            },
        )
    }

    /// C-01. Readiness must never depend on the caller consuming a stream it
    /// cannot hold yet.
    #[tokio::test(start_paused = true)]
    async fn test_connect_completes_when_retries_fill_a_capacity_one_event_stream() {
        // Three failed attempts before the bind, with a single slot for the
        // session events they produce. Nobody can read that slot until
        // `connect` returns, so a router that blocked on it would never
        // deliver the bind that resolves this call.
        let transport = ScriptedTransport::new(vec![
            None,
            None,
            None,
            Some(vec![Say::Line(CONOK), Say::AwaitControls(usize::MAX)]),
        ]);

        let (client, mut events) = tokio::time::timeout(
            Duration::from_secs(30),
            Client::assemble(
                session::connect(transport, session_options()).expect("a valid session"),
                capacities(1),
            ),
        )
        .await
        .expect("connect must not wait on a stream the caller cannot hold yet")
        .expect("the fourth attempt bound");

        // One slot was configured, so one pre-connection event is retained —
        // the first, in the order it happened — and the rest are discarded by
        // the documented policy rather than blocking the bind.
        assert!(
            matches!(events.next().await, Some(SessionEvent::Disconnected { .. })),
            "the retained startup event comes first"
        );
        assert!(matches!(
            events.next().await,
            Some(SessionEvent::Connected(_))
        ));
        drop(client);
    }

    /// C-01, the retention half: with room for them, every startup event is
    /// kept, in the order it happened.
    #[tokio::test(start_paused = true)]
    async fn test_connect_retains_startup_events_in_order() {
        let transport = ScriptedTransport::new(vec![
            None,
            None,
            None,
            Some(vec![Say::Line(CONOK), Say::AwaitControls(usize::MAX)]),
        ]);

        let (client, events) = tokio::time::timeout(
            Duration::from_secs(30),
            Client::assemble(
                session::connect(transport, session_options()).expect("a valid session"),
                capacities(16),
            ),
        )
        .await
        .expect("connect must not wait on a stream the caller cannot hold yet")
        .expect("the fourth attempt bound");

        let seen: Vec<SessionEvent> = events.take(4).collect().await;
        assert!(
            seen.iter()
                .take(3)
                .all(|event| matches!(event, SessionEvent::Disconnected { .. })),
            "the three failed attempts come first, in order: {seen:?}"
        );
        assert!(
            matches!(seen.get(3), Some(SessionEvent::Connected(_))),
            "then the bind: {seen:?}"
        );
        drop(client);
    }

    /// C-01. The same, when the attempts run out instead of succeeding: the
    /// failure has to reach the caller rather than deadlock behind it.
    #[tokio::test(start_paused = true)]
    async fn test_connect_reports_exhaustion_with_a_capacity_one_event_stream() {
        let transport = ScriptedTransport::new(vec![None, None, None, None, None, None, None]);

        let outcome = tokio::time::timeout(
            Duration::from_secs(30),
            Client::assemble(
                session::connect(transport, session_options()).expect("a valid session"),
                capacities(1),
            ),
        )
        .await
        .expect("connect must not hang once the retries are spent");

        // A-04: exhaustion carries the last thing that went wrong. A count on
        // its own reads the same whether the host did not resolve or the
        // server refused the credentials.
        match outcome {
            Err(Error::ReconnectExhausted {
                last: Some(reason), ..
            }) => assert!(
                matches!(*reason, DisconnectReason::ConnectionFailed { .. }),
                "expected the scripted connection failure, got {reason:?}"
            ),
            other => panic!(
                "expected exhaustion with a cause, got {:?}",
                other.map(|_| ())
            ),
        }
    }

    /// C-09. `disconnect()` must mean the session is over, not that a request
    /// to end it was queued.
    #[tokio::test(start_paused = true)]
    async fn test_disconnect_waits_for_the_destroy_the_socket_and_the_tasks() {
        // The server answers the destroy — and only the destroy — with the
        // `END` that T7 promises
        // [`docs/spec/02-session-lifecycle.md` §2.2, T7].
        let transport = ScriptedTransport::new(vec![Some(vec![
            Say::Line(CONOK),
            Say::AwaitControls(1),
            Say::Line("END,31,session destroyed"),
        ])]);
        let log = transport.log();

        let (client, mut events) = Client::assemble(
            session::connect(transport, session_options()).expect("a valid session"),
            capacities(16),
        )
        .await
        .expect("the session bound");

        tokio::time::timeout(Duration::from_secs(10), client.disconnect())
            .await
            .expect("disconnect must not hang")
            .expect("the session was destroyed cleanly");

        let recorded = log.lock().expect("the log is not poisoned").clone();
        assert!(
            recorded
                .controls
                .iter()
                .any(|parameters| parameters.contains("LS_op=destroy")),
            "the destroy reached the wire: {:?}",
            recorded.controls
        );
        assert!(recorded.closes >= 1, "the transport was closed");
        // Every task has finished, so the streams they feed have ended.
        let remaining: Vec<SessionEvent> = std::iter::from_fn(|| events.next().now_or_never())
            .flatten()
            .collect();
        assert!(
            matches!(remaining.last(), Some(SessionEvent::Closed(_))),
            "the last event is terminal: {remaining:?}"
        );
    }

    /// C-02 and C-09 together: a caller that stopped reading must not be able
    /// to make `disconnect` hang.
    #[tokio::test(start_paused = true)]
    async fn test_disconnect_completes_even_with_an_unread_event_stream() {
        // Three session-level notifications and one slot to put them in. The
        // stream is held and never read, so the router wedges on the second and
        // stops taking anything from the driver — which is exactly the state in
        // which a shutdown used to be impossible.
        let transport = ScriptedTransport::new(vec![Some(vec![
            Say::Line(CONOK),
            Say::Line("SERVNAME,Lightstreamer HTTP Server"),
            Say::Line("CLIENTIP,127.0.0.1"),
            Say::Line("CONS,unlimited"),
        ])]);
        let log = transport.log();

        let (client, events) = Client::assemble(
            session::connect(transport, session_options()).expect("a valid session"),
            capacities(1),
        )
        .await
        .expect("the session bound");
        // Let the router reach the wedge before asking it to stop.
        tokio::time::sleep(Duration::from_millis(20)).await;

        let disconnected = tokio::time::timeout(Duration::from_secs(60), client.disconnect()).await;
        assert!(
            disconnected.is_ok(),
            "a stream nobody reads must not hold the shutdown open"
        );
        assert!(
            log.lock().expect("the log is not poisoned").closes >= 1,
            "the socket is closed on the forced path too"
        );
        drop(events);
    }

    /// C-04. A subscription's stream must be registered *before* the request
    /// that could produce an event for it is queued, and the registration must
    /// be undone if that request cannot be queued at all.
    ///
    /// The window is made observable by giving the session a single command
    /// slot, filling it, and never running the driver: the submission is then
    /// stuck, and the registration either already happened or never will.
    #[tokio::test(start_paused = true)]
    async fn test_subscribe_registers_the_stream_before_it_submits_the_request() {
        let options = crate::session::options::SessionOptions {
            command_capacity: std::num::NonZeroUsize::MIN,
            ..Default::default()
        };
        let Ok((driver, handle, session_events)) = crate::session::connect(NullTransport, options)
        else {
            panic!("a default session configuration is valid");
        };

        // Occupy the one command slot. The driver is deliberately never run,
        // so nothing will ever free it.
        let filler = handle
            .allocate_key()
            .expect("the key space is not exhausted");
        assert!(handle.unsubscribe(filler).await.is_ok());

        let (router_tx, mut router_rx) = mpsc::unbounded_channel();
        let (alive_tx, _alive_rx) = mpsc::channel(1);
        let client = Client {
            id: a_client_id(),
            handle: handle.clone(),
            router: router_tx,
            update_capacity: std::num::NonZeroUsize::new(8).unwrap_or(std::num::NonZeroUsize::MIN),
            shutdown_timeout: Duration::from_secs(5),
            tasks: Vec::new(),
            stop_on_drop: false,
            _alive: alive_tx,
        };

        let subscribing = tokio::spawn(async move {
            client
                .subscribe(Subscription::new(
                    SubscriptionMode::Merge,
                    ItemGroup::from_items(["item1"]).expect("a valid group"),
                    FieldSchema::from_fields(["price"]).expect("a valid schema"),
                ))
                .await
                .is_ok()
        });
        tokio::time::sleep(Duration::from_millis(20)).await;

        assert!(
            !subscribing.is_finished(),
            "the submission is blocked, which is the window under test"
        );
        assert!(
            matches!(router_rx.try_recv(), Ok(RouterCommand::Register { .. })),
            "the stream is registered before the request is submitted"
        );

        // Now make the submission fail outright: the registration must be
        // rolled back, and no unsubscription asked for, because nothing was
        // ever subscribed.
        drop((driver, session_events, handle));
        assert!(
            !tokio::time::timeout(Duration::from_secs(5), subscribing)
                .await
                .expect("the submission fails once the driver is gone")
                .expect("the task did not panic"),
            "a subscription that could not be submitted is an error"
        );
        assert!(
            matches!(router_rx.try_recv(), Ok(RouterCommand::Unregister { .. })),
            "the registration is rolled back, not left behind"
        );
    }

    /// A-07. Two clients number their subscriptions from one, so their handles
    /// carry the same number. Spending one on the other must fail with a typed
    /// error and must not reach the wire.
    #[tokio::test(start_paused = true)]
    async fn test_a_handle_from_one_client_cannot_unsubscribe_another() {
        async fn a_client() -> (Client, Updates, std::sync::Arc<std::sync::Mutex<ScriptLog>>) {
            let transport = ScriptedTransport::new(vec![Some(vec![
                Say::Line(CONOK),
                Say::AwaitControls(usize::MAX),
            ])]);
            let log = transport.log();
            let (client, events) = Client::assemble(
                session::connect(transport, session_options()).expect("a valid session"),
                capacities(16),
            )
            .await
            .expect("the session bound");
            drop(events);
            let updates = client
                .subscribe(Subscription::new(
                    SubscriptionMode::Merge,
                    ItemGroup::from_items(["item1"]).expect("a valid group"),
                    FieldSchema::from_fields(["price"]).expect("a valid schema"),
                ))
                .await
                .expect("the subscription was queued");
            (client, updates, log)
        }

        // The `Updates` streams are held, not dropped: dropping one
        // unsubscribes, which is the thing a cross-client unsubscribe must not
        // be confused with.
        let (first, first_updates, _) = a_client().await;
        let (second, second_updates, second_log) = a_client().await;
        let (first_id, second_id) = (first_updates.id(), second_updates.id());

        // The premise: the numbers really do collide.
        assert_eq!(
            first_id.get(),
            second_id.get(),
            "each client numbers its subscriptions from one"
        );
        assert_ne!(first_id, second_id, "the handles are still distinct");

        // The subscription each client did ask for reached the wire, so the
        // absence of a `delete` below is not the transport being idle.
        tokio::time::sleep(Duration::from_millis(20)).await;
        let before = second_log.lock().expect("the log is not poisoned").clone();
        assert!(
            before
                .controls
                .iter()
                .any(|parameters| parameters.contains("LS_op=add")),
            "the second client's own subscription was sent: {:?}",
            before.controls
        );

        match second.unsubscribe(first_id).await {
            Err(Error::ForeignSubscription { id }) => assert_eq!(id, first_id.get()),
            other => panic!("a foreign handle was accepted: {other:?}"),
        }

        tokio::time::sleep(Duration::from_millis(20)).await;
        let after = second_log.lock().expect("the log is not poisoned").clone();
        assert!(
            !after
                .controls
                .iter()
                .any(|parameters| parameters.contains("LS_op=delete")),
            "nothing was sent for a handle this client does not own: {:?}",
            after.controls
        );

        // Its own handle still works.
        assert!(second.unsubscribe(second_id).await.is_ok());
        drop((first, second, first_updates, second_updates));
    }

    /// C-05. A blocked delivery that discovers a dropped receiver must not
    /// swallow the unsubscription the drop itself asked for.
    #[tokio::test]
    async fn test_a_stream_dropped_while_the_router_is_blocked_still_unsubscribes() {
        let mut harness = harness(8);
        let id = SubscriptionId::new(harness.client, a_key().await);
        // One slot: the second event has nowhere to go, so the router blocks
        // inside the send — the state in which the receiver then disappears.
        let (events_tx, events_rx) = mpsc::channel(1);
        assert!(
            harness
                .commands
                .send(RouterCommand::Register {
                    id,
                    events: events_tx,
                })
                .is_ok()
        );
        let updates = Updates::new(id, events_rx, harness.commands.clone());

        assert!(
            harness
                .session_tx
                .send(data(
                    id,
                    WireSubscriptionEvent::Activated {
                        item_count: 1,
                        field_count: 1,
                        command_fields: None,
                    }
                ))
                .await
                .is_ok()
        );
        for dropped_count in [1, 2] {
            assert!(
                harness
                    .session_tx
                    .send(data(
                        id,
                        WireSubscriptionEvent::Overflow {
                            item_index: 1,
                            dropped_count,
                        }
                    ))
                    .await
                    .is_ok()
            );
        }
        // The router is now blocked on a full stream. Dropping it is what the
        // caller does when it walks away.
        tokio::task::yield_now().await;
        drop(updates);

        match tokio::time::timeout(Duration::from_secs(5), harness.unsubscribed.recv()).await {
            Ok(Some(key)) => assert_eq!(key.get(), id.get()),
            other => panic!("the server was left subscribed: {other:?}"),
        }
        // Exactly one: cleanup is not duplicated either.
        assert!(harness.unsubscribed.recv().now_or_never().is_none());
    }

    /// C-02. The router honours a stop while a caller's stream is full, so a
    /// client being torn down is not held open by a consumer that walked away.
    #[tokio::test]
    async fn test_the_router_stops_while_a_callers_stream_is_full() {
        let harness = harness(1);
        let id = SubscriptionId::new(harness.client, a_key().await);
        let (events_tx, _events_rx) = mpsc::channel(1);
        assert!(
            harness
                .commands
                .send(RouterCommand::Register {
                    id,
                    events: events_tx,
                })
                .is_ok()
        );
        for _ in 0..4 {
            let _ = harness
                .session_tx
                .send(data(
                    id,
                    WireSubscriptionEvent::Overflow {
                        item_index: 1,
                        dropped_count: 1,
                    },
                ))
                .await;
        }

        assert!(harness.stop.send(true).is_ok());
        // The router drops every sender as it stops, which is what ends the
        // caller's streams.
        match tokio::time::timeout(Duration::from_secs(5), harness.session_tx.closed()).await {
            Ok(()) => {}
            Err(_) => panic!("the router did not stop while a stream was full"),
        }
    }
}
