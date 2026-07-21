//! What one subscription tells you, and the stream that delivers it.

use std::pin::Pin;
use std::task::{Context, Poll};

use futures_util::Stream;
use tokio::sync::mpsc;

use crate::client::events::SubscriptionId;
use crate::client::router::RouterCommand;
use crate::error::ServerError;
use crate::protocol::response::{FilteringMode, MaxFrequency as WireFrequency};
use crate::subscription::item_update::ItemUpdate;
use crate::subscription::manager::{CommandFields as WireCommandFields, SubscriptionEvent as Wire};

/// Where the key and the command sit in a `COMMAND`-mode field schema
/// [`docs/spec/04-notifications.md` §3.2].
///
/// Both positions are **1-based**, matching the numbering
/// [`ItemUpdate`] uses everywhere.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub struct CommandFields {
    /// Position of the field that identifies the row.
    pub key: usize,
    /// Position of the field that says what happened to the row — `ADD`,
    /// `UPDATE` or `DELETE`.
    pub command: usize,
}

#[cfg(feature = "test-util")]
impl CommandFields {
    /// Assembles one field by field, for [`crate::test_util`].
    #[must_use]
    pub(crate) const fn from_parts(key: usize, command: usize) -> Self {
        Self { key, command }
    }
}

impl From<WireCommandFields> for CommandFields {
    fn from(fields: WireCommandFields) -> Self {
        Self {
            key: fields.key,
            command: fields.command,
        }
    }
}

/// The update rate the server has settled on for a subscription
/// [`docs/spec/04-notifications.md` §3.8].
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum UpdateFrequency {
    /// No limit is applied.
    Unlimited,
    /// At most this many updates per second, per item.
    Limited {
        /// The limit exactly as the server wrote it. Kept as text because the
        /// protocol carries it as text and this crate never reinterprets a
        /// server-supplied number on the caller's behalf.
        updates_per_second: String,
    },
    /// **The rate could not be read**, so do not treat it as a limit.
    ///
    /// The server sent something that is neither `unlimited` nor a decimal
    /// number [`docs/spec/04-notifications.md` §3.8]. It is reported rather
    /// than dropped, and reported apart from
    /// [`Limited`](UpdateFrequency::Limited) rather than inside it, because a
    /// caller that reads `Limited { updates_per_second: "" }` and divides by it
    /// is acting on a number nobody sent. Log it, or treat the subscription's
    /// rate as unknown — but do not compute with it.
    Unrecognized {
        /// The value as it arrived.
        literal: String,
    },
}

impl From<WireFrequency> for UpdateFrequency {
    fn from(frequency: WireFrequency) -> Self {
        match frequency {
            WireFrequency::Unlimited => Self::Unlimited,
            WireFrequency::Limited { updates_per_second } => Self::Limited { updates_per_second },
            WireFrequency::Unrecognized { literal } => Self::Unrecognized { literal },
        }
    }
}

/// Whether the server may drop or merge updates for a subscription to keep up
/// [`docs/spec/04-notifications.md` §3.8].
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum Filtering {
    /// The server may filter: it merges or discards updates rather than fall
    /// behind, and you see fewer events than the adapter produced.
    Filtered,
    /// The server may not filter: every update is delivered. If it cannot keep
    /// up it says so with a [`SubscriptionEvent::Overflow`] rather than
    /// dropping silently.
    Unfiltered,
    /// The server named a mode this version does not know.
    ///
    /// Preserved rather than rejected, so a newer server cannot break an older
    /// client over one word of an otherwise well-formed line.
    Unrecognized {
        /// The word as it arrived.
        literal: String,
    },
}

impl From<FilteringMode> for Filtering {
    fn from(mode: FilteringMode) -> Self {
        match mode {
            FilteringMode::Filtered => Self::Filtered,
            FilteringMode::Unfiltered => Self::Unfiltered,
            FilteringMode::Unrecognized { literal } => Self::Unrecognized { literal },
        }
    }
}

/// Everything one subscription can tell you.
///
/// Delivered by [`Updates`]. `#[non_exhaustive]`, so match with a trailing `_`
/// arm and a later version adding a variant will not break your build.
///
/// The ordinary shape of a subscription's life is
/// [`Activated`](SubscriptionEvent::Activated), then a run of
/// [`Update`](SubscriptionEvent::Update)s — snapshot first if one was asked
/// for, real-time after — and finally either
/// [`Unsubscribed`](SubscriptionEvent::Unsubscribed) or
/// [`Rejected`](SubscriptionEvent::Rejected). After a session is replaced, a
/// second `Activated` arrives and the run starts over.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum SubscriptionEvent {
    /// The subscription is live on the server and its shape is now known
    /// [`docs/spec/04-notifications.md` §3.1, §3.2].
    ///
    /// Arrives again whenever the subscription is re-created on a session that
    /// replaced a lost one, in which case every item's state has been rebuilt
    /// from nothing.
    Activated {
        /// How many items the server resolved the item group into.
        item_count: usize,
        /// How many fields the server resolved the field schema into. Every
        /// update carries exactly this many values.
        field_count: usize,
        /// Present exactly for [`SubscriptionMode::Command`](crate::SubscriptionMode)
        /// subscriptions, which the server activates differently because it
        /// must also say where the key and command fields are.
        command_fields: Option<CommandFields>,
    },

    /// New values for one item [`docs/spec/04-notifications.md` §2].
    ///
    /// The update carries the item's **complete** state, not only what
    /// changed, so every field is readable; ask
    /// [`ItemUpdate::changed_fields`] what actually moved.
    Update(Box<ItemUpdate>),

    /// The snapshot of one item is complete; everything after this is a live
    /// change [`docs/spec/04-notifications.md` §3.5].
    ///
    /// Sent in `DISTINCT` and `COMMAND` modes. A `MERGE` snapshot is a single
    /// update and gets no boundary marker; `RAW` has no snapshot at all.
    EndOfSnapshot {
        /// **1-based** index of the item whose snapshot ended.
        item_index: usize,
    },

    /// The server cleared one item's snapshot
    /// [`docs/spec/04-notifications.md` §3.6].
    ///
    /// Drop the list, history or row set you accumulated for this item. The
    /// field values this crate tracks are deliberately **not** cleared, so
    /// that a delta arriving straight afterwards still decodes.
    SnapshotCleared {
        /// **1-based** index of the item whose snapshot was cleared.
        item_index: usize,
    },

    /// The **server** dropped updates for one item, because of its own buffer
    /// limits [`docs/spec/04-notifications.md` §3.7].
    ///
    /// Those updates are gone; nothing can reconstruct them, and the item's
    /// values now reflect only what survived. This is the protocol's own way
    /// of surfacing loss rather than hiding it, and this crate passes it
    /// straight through.
    ///
    /// It can only happen in `RAW` mode, for `ADD`/`DELETE` events in
    /// `COMMAND` mode, and for subscriptions that asked for
    /// [`MaxFrequency::Unfiltered`](crate::MaxFrequency).
    Overflow {
        /// **1-based** index of the item whose updates were dropped.
        item_index: usize,
        /// How many update events were dropped.
        dropped_count: u64,
    },

    /// The subscription's frequency configuration
    /// [`docs/spec/04-notifications.md` §3.8].
    ///
    /// Arrives once when the subscription starts — possibly before
    /// [`Activated`](SubscriptionEvent::Activated) — and again after every
    /// reconfiguration, even one that changed nothing.
    Reconfigured {
        /// The rate now in force.
        max_frequency: UpdateFrequency,
        /// Whether the server may filter.
        filtering: Filtering,
    },

    /// The subscription request could not be sent, and will be retried when
    /// the session next binds.
    ///
    /// **Not terminal, and not a refusal.** The server never saw the request —
    /// the connection was gone, or the transport rejected it — so it has no
    /// opinion on your subscription yet. This crate keeps the subscription in
    /// its desired set and re-issues it on the next connection, after which
    /// [`Activated`](SubscriptionEvent::Activated) arrives as usual.
    ///
    /// You do not have to do anything. It is surfaced because it means a real
    /// delay before data starts, and a subscription that seems slow to start
    /// should not be a mystery.
    Deferred {
        /// What stopped it. Diagnostic text; never a credential.
        reason: String,
    },

    /// The server refused the subscription, with a code from **Appendix B**
    /// [`docs/spec/05-error-codes.md` §2].
    ///
    /// Terminal: nothing else arrives on this stream, and the subscription is
    /// not retried on a later session. Common causes are a bad item group
    /// (code 21), a bad field schema (23), a mode the item does not allow
    /// (24), and — in `COMMAND` mode — a schema with no key or no command
    /// field (15, 16).
    ///
    /// Contrast [`Deferred`](SubscriptionEvent::Deferred), where the request
    /// never reached the server and is still coming.
    Rejected(ServerError),

    /// The subscription ended [`docs/spec/04-notifications.md` §3.4].
    ///
    /// Terminal: no further update for these items and fields will be sent.
    Unsubscribed,

    /// A notification for this subscription could not be interpreted.
    ///
    /// Never fatal and never silent: the stream keeps running, and the item
    /// state it refers to is whatever the last decodable update left. Seeing
    /// this repeatedly means this client and the server disagree about the
    /// subscription's shape, which is worth reporting as a bug.
    Undecodable {
        /// What went wrong, in terms a bug report can use.
        detail: String,
    },
}

impl SubscriptionEvent {
    /// Whether nothing further will arrive on this subscription's stream.
    #[must_use]
    #[inline]
    pub const fn is_terminal(&self) -> bool {
        matches!(self, Self::Unsubscribed | Self::Rejected(_))
    }

    /// Translates the subscription layer's event into the public one.
    pub(crate) fn from_wire(event: Wire) -> Self {
        match event {
            Wire::Activated {
                item_count,
                field_count,
                command_fields,
            } => Self::Activated {
                item_count,
                field_count,
                command_fields: command_fields.map(Into::into),
            },
            Wire::Update(update) => Self::Update(Box::new(update)),
            Wire::EndOfSnapshot { item_index } => Self::EndOfSnapshot { item_index },
            Wire::SnapshotCleared { item_index } => Self::SnapshotCleared { item_index },
            Wire::Overflow {
                item_index,
                dropped_count,
            } => Self::Overflow {
                item_index,
                dropped_count,
            },
            Wire::Reconfigured {
                max_frequency,
                filtering,
            } => Self::Reconfigured {
                max_frequency: max_frequency.into(),
                filtering: filtering.into(),
            },
            Wire::Unsubscribed => Self::Unsubscribed,
        }
    }
}

/// The stream of one subscription's events.
///
/// Returned by [`Client::subscribe`](crate::Client::subscribe). It implements
/// [`futures_util::Stream`], so the usual combinators work and the plain shape
/// is a `while let` loop:
///
/// ```no_run
/// use futures_util::StreamExt;
/// use lightstreamer_rs::SubscriptionEvent;
///
/// # async fn run(mut updates: lightstreamer_rs::Updates) {
/// while let Some(event) = updates.next().await {
///     if let SubscriptionEvent::Update(update) = event {
///         println!("{}: {:?}", update.item_name(), update.changed_fields());
///     }
/// }
/// # }
/// ```
///
/// # Dropping this stream unsubscribes
///
/// There is no deregistration dance: drop the `Updates` and the client sends
/// the unsubscription for you. The request goes out asynchronously — the drop
/// itself does not block and cannot fail — so a `UNSUB` may still be in flight
/// when the value is gone. If you need to observe the unsubscription
/// completing, call [`Client::unsubscribe`](crate::Client::unsubscribe)
/// instead and keep reading until [`SubscriptionEvent::Unsubscribed`].
///
/// # Backpressure: bounded, and nothing is dropped
///
/// The stream is fed by a **bounded** channel whose capacity is
/// [`ConnectionOptions::with_update_capacity`](crate::ConnectionOptions::with_update_capacity)
/// — 1024 events by default. When it fills, the client **blocks** rather than
/// discarding anything, and that backpressure propagates through the session
/// down to the socket. Concretely, a consumer that stops polling this stream
/// will, once the buffer fills, stall **the whole client**: other
/// subscriptions and the session's own liveness included.
///
/// That is a deliberate choice, and the alternatives are worse. Dropping
/// updates locally would corrupt the notification count that makes recovery
/// after a broken connection correct
/// [`docs/spec/02-session-lifecycle.md` §5.2], and it would hide loss the
/// protocol goes out of its way to report — TLCP has its own overflow
/// notification precisely so that a client is *told* when data was discarded
/// [`docs/spec/04-notifications.md` §3.7].
///
/// # What a long stall actually does
///
/// "Stalls the client" is literal, and it has a consequence worth planning
/// for. While the client is blocked on a full stream it is not reading from
/// the socket either, so its **liveness timers do not advance**. If you stop
/// consuming for longer than the keepalive interval the server negotiated —
/// reported to you as [`Connected::keepalive`](crate::Connected::keepalive),
/// plus [`ConnectionOptions::with_keepalive_slack`](crate::ConnectionOptions::with_keepalive_slack)
/// — then the moment you start reading again the client will conclude the
/// connection went silent, tear it down, and recover
/// [`docs/spec/02-session-lifecycle.md` §8.1]. You will see a
/// [`SessionEvent::Disconnected`](crate::SessionEvent::Disconnected) carrying
/// [`DisconnectReason::Stalled`](crate::DisconnectReason::Stalled), followed
/// by a reconnection.
///
/// Nothing is lost when that happens — recovery is what the notification count
/// exists for — but it is real work, and it is avoidable. A consumer that may
/// pause for seconds at a time should either buffer on its own side or, better,
/// ask the server to send less.
///
/// If you cannot keep up, say so to the server, which is where the protocol
/// puts that decision: cap the rate with
/// [`Subscription::with_max_frequency`](crate::Subscription::with_max_frequency),
/// or bound what the server buffers with
/// [`Subscription::with_buffer_size`](crate::Subscription::with_buffer_size).
/// The server will then merge or drop on its side, and tell you it did with a
/// [`SubscriptionEvent::Overflow`].
///
/// # The one exception: shutdown
///
/// "Nothing is dropped" holds **while the client is running**. It does not
/// survive an ordered shutdown, and it deliberately does not:
/// [`Client::disconnect`](crate::Client::disconnect) and dropping the
/// [`Client`](crate::Client) are signalled out of band, every blocked delivery
/// gives way to them, and whatever had not yet been delivered is discarded
/// along with this stream.
///
/// Without that exception a consumer that walked away could make the client
/// impossible to stop — no disconnect, no drop, and a task and a socket alive
/// until the process ended. Nothing observable is lost by it: a caller still
/// reading frees room and the delivery completes, and a caller that has asked
/// to disconnect has said it wants no more. See
/// `docs/adr/0003-typed-event-stream-as-delivery-surface.md`.
#[derive(Debug)]
pub struct Updates {
    id: SubscriptionId,
    events: mpsc::Receiver<SubscriptionEvent>,
    /// Unbounded on purpose: [`Drop`] cannot await, and this channel carries
    /// one small message per subscribe and unsubscribe — user actions, not
    /// data volume — so it cannot grow with the update flow.
    control: mpsc::UnboundedSender<RouterCommand>,
}

impl Updates {
    /// Wraps the receiving half the router just registered.
    pub(crate) const fn new(
        id: SubscriptionId,
        events: mpsc::Receiver<SubscriptionEvent>,
        control: mpsc::UnboundedSender<RouterCommand>,
    ) -> Self {
        Self {
            id,
            events,
            control,
        }
    }

    /// The handle identifying this subscription.
    ///
    /// Stable for the life of the stream, including across a session that was
    /// lost and replaced. Use it to correlate with
    /// [`SessionEvent::Resubscribed`](crate::SessionEvent::Resubscribed).
    #[must_use]
    #[inline]
    pub const fn id(&self) -> SubscriptionId {
        self.id
    }
}

impl Stream for Updates {
    type Item = SubscriptionEvent;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.events.poll_recv(cx)
    }
}

impl Drop for Updates {
    /// Unsubscribes.
    ///
    /// Best effort by necessity: a `Drop` cannot await and cannot report a
    /// failure. The command channel is unbounded, so the send only fails when
    /// the client has already stopped — in which case the subscription is gone
    /// anyway.
    fn drop(&mut self) {
        if self
            .control
            .send(RouterCommand::StreamDropped { id: self.id })
            .is_err()
        {
            tracing::debug!(
                id = self.id.get(),
                "subscription stream dropped after the client"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_events_that_end_a_subscription_are_marked_terminal() {
        assert!(SubscriptionEvent::Unsubscribed.is_terminal());
        assert!(SubscriptionEvent::Rejected(ServerError::new(19, "not found")).is_terminal());
        assert!(
            !SubscriptionEvent::EndOfSnapshot { item_index: 1 }.is_terminal(),
            "end of snapshot is a boundary, not an ending"
        );
        assert!(
            !SubscriptionEvent::Overflow {
                item_index: 1,
                dropped_count: 4
            }
            .is_terminal()
        );
    }

    #[test]
    fn test_wire_events_translate_without_losing_anything() {
        let activated = SubscriptionEvent::from_wire(Wire::Activated {
            item_count: 3,
            field_count: 4,
            command_fields: Some(WireCommandFields { key: 1, command: 2 }),
        });
        assert!(matches!(
            activated,
            SubscriptionEvent::Activated {
                item_count: 3,
                field_count: 4,
                command_fields: Some(CommandFields { key: 1, command: 2 })
            }
        ));

        let overflow = SubscriptionEvent::from_wire(Wire::Overflow {
            item_index: 2,
            dropped_count: 17,
        });
        assert!(matches!(
            overflow,
            SubscriptionEvent::Overflow {
                item_index: 2,
                dropped_count: 17
            }
        ));
    }

    #[test]
    fn test_reconfiguration_keeps_the_servers_own_text() {
        let event = SubscriptionEvent::from_wire(Wire::Reconfigured {
            max_frequency: WireFrequency::Limited {
                updates_per_second: "3.0".to_owned(),
            },
            filtering: FilteringMode::Unrecognized {
                literal: "novel".to_owned(),
            },
        });
        assert!(matches!(
            event,
            SubscriptionEvent::Reconfigured {
                max_frequency: UpdateFrequency::Limited { updates_per_second },
                filtering: Filtering::Unrecognized { literal },
            } if updates_per_second == "3.0" && literal == "novel"
        ));
    }

    #[test]
    fn test_an_unreadable_frequency_is_not_delivered_as_a_limit() {
        // A `CONF` whose max-frequency argument is neither `unlimited` nor a
        // decimal number [`docs/spec/04-notifications.md` §3.8] must not reach
        // the caller inside `Limited`, where it would look like a rate.
        let event = SubscriptionEvent::from_wire(Wire::Reconfigured {
            max_frequency: WireFrequency::Unrecognized {
                literal: "banana".to_owned(),
            },
            filtering: FilteringMode::Filtered,
        });
        assert!(matches!(
            event,
            SubscriptionEvent::Reconfigured {
                max_frequency: UpdateFrequency::Unrecognized { literal },
                filtering: Filtering::Filtered,
            } if literal == "banana"
        ));
    }
}
