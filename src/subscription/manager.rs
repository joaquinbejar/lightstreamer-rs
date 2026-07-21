//! Per-subscription state: what the client knows about one subscription, and
//! the events it produces from the notifications the session layer hands it.
//!
//! A [`SubscriptionManager`] is the client-side half of one subscription. It
//! starts out knowing only what the caller asked for — the mode, and whatever
//! item and field names the caller declared — because the protocol does not
//! reveal the subscription's actual shape until the server activates it:
//! `SUBOK` (or `SUBCMD`, in `COMMAND` mode) is what carries the item and field
//! counts [`docs/spec/04-notifications.md` §3.1, §3.2]. Until that line
//! arrives there is no way to size the per-item state, and this module says so
//! in the type system rather than guessing: see [`Activation`].
//!
//! Once activated it holds one [`ItemState`] per item — the field-value
//! baseline that every subsequent `U` is decoded against
//! [`docs/spec/04-notifications.md` §2.3] — plus, per item, the two bits the
//! specification's snapshot decision table needs: whether an update has been
//! seen at all and whether `EOS` has arrived
//! [`docs/spec/04-notifications.md` §2.4, §3.5].
//!
//! # Routing is not this module's job
//!
//! Every subscription notification carries a `<subscription-ID>`
//! [`docs/spec/04-notifications.md` §2.1], and this module **ignores it**. The
//! wire id is per-session and restarts at 1 whenever a lost session is replaced
//! (`src/session/mod.rs`, [`SubscriptionKey`]), so matching an incoming line to
//! the manager that owns it is the session layer's responsibility. A manager
//! interprets whatever it is given.
//!
//! [`SubscriptionKey`]: crate::session::SubscriptionKey
//! [`ItemState`]: crate::subscription::update::ItemState

use std::sync::Arc;

use crate::protocol::ProtocolError;
use crate::protocol::request::SubscriptionMode;
use crate::protocol::response::{FilteringMode, MaxFrequency, Notification};
use crate::subscription::item_update::{ItemUpdate, SubscriptionSchema, UpdateKind};
use crate::subscription::update::ItemState;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// A subscription notification that cannot be reconciled with the state this
/// client holds.
///
/// Every variant describes a server line that contradicts something the server
/// itself already said, so none of them is recoverable by retrying the line.
/// They are reported, never panicked on.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub(crate) enum SubscriptionError {
    /// A notification that needs the subscription's shape arrived before the
    /// `SUBOK`/`SUBCMD` that announces it
    /// [`docs/spec/04-notifications.md` §3.1, §3.2].
    #[error("`{tag}` arrived before the subscription was activated by `SUBOK`/`SUBCMD`")]
    NotActivated {
        /// The tag of the offending notification.
        tag: &'static str,
    },

    /// A notification arrived after `UNSUB`, which promises that "no more
    /// real-time updates related to the subscription items and fields will be
    /// sent" [`docs/spec/04-notifications.md` §3.4].
    #[error("`{tag}` arrived after the subscription was ended by `UNSUB`")]
    Ended {
        /// The tag of the offending notification.
        tag: &'static str,
    },

    /// A notification named an item outside the range `SUBOK`/`SUBCMD`
    /// announced [`docs/spec/04-notifications.md` §3.1].
    #[error("`{tag}` names item {item_index} of a {item_count}-item subscription")]
    ItemOutOfRange {
        /// The tag of the offending notification.
        tag: &'static str,
        /// The 1-based item index it carried.
        item_index: u64,
        /// How many items the subscription actually has.
        item_count: usize,
    },

    /// `SUBCMD` placed the key or the command field outside its own schema
    /// [`docs/spec/04-notifications.md` §3.2].
    #[error(
        "`SUBCMD` places the {role} field at position {position} of a {field_count}-field schema"
    )]
    CommandFieldOutOfRange {
        /// Either `key` or `command`.
        role: &'static str,
        /// The 1-based field position it carried.
        position: u64,
        /// How many fields the schema actually has.
        field_count: usize,
    },

    /// The value list of a `U` could not be decoded against the item's current
    /// state [`docs/spec/04-notifications.md` §2.3].
    #[error(transparent)]
    Value(#[from] ProtocolError),
}

/// Builds an out-of-range item error.
#[cold]
#[inline(never)]
fn item_out_of_range(tag: &'static str, item_index: u32, item_count: usize) -> SubscriptionError {
    SubscriptionError::ItemOutOfRange {
        tag,
        item_index: u64::from(item_index),
        item_count,
    }
}

// ---------------------------------------------------------------------------
// Events
// ---------------------------------------------------------------------------

/// Where the key and the command sit in a `COMMAND`-mode schema
/// [`docs/spec/04-notifications.md` §3.2].
///
/// Both positions are **1-based**, exactly as `SUBCMD` reports them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct CommandFields {
    /// Position of the field that identifies the row.
    pub(crate) key: usize,
    /// Position of the field that says what happened to the row.
    pub(crate) command: usize,
}

/// Everything one subscription can tell its caller.
///
/// A closed enum, per
/// `docs/adr/0003-typed-event-stream-as-delivery-surface.md`: adding a variant
/// is a compile-time event for every consumer. One variant per subscription-
/// related notification the specification defines
/// [`docs/spec/04-notifications.md` §2, §3].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SubscriptionEvent {
    /// `SUBOK` / `SUBCMD` — the subscription is live on the server and its
    /// shape is now known [`docs/spec/04-notifications.md` §3.1, §3.2].
    ///
    /// Real-time updates start after this line. It is also emitted again
    /// whenever the subscription is re-established on a replacement session,
    /// in which case all item state has been rebuilt from scratch.
    Activated {
        /// How many items the subscription resolved to.
        item_count: usize,
        /// How many fields the schema resolved to — the width every `U` value
        /// list is decoded against.
        field_count: usize,
        /// Present exactly for `COMMAND`-mode subscriptions, which are
        /// activated by `SUBCMD` rather than `SUBOK`.
        command_fields: Option<CommandFields>,
    },

    /// `U` — new values for one item [`docs/spec/04-notifications.md` §2].
    Update(ItemUpdate),

    /// `EOS` — the snapshot of one item is complete
    /// [`docs/spec/04-notifications.md` §3.5].
    ///
    /// From here on, updates for that item are classified as real time rather
    /// than snapshot in `DISTINCT` and `COMMAND` modes
    /// [`docs/spec/04-notifications.md` §2.4].
    EndOfSnapshot {
        /// **1-based** index of the item whose snapshot ended.
        item_index: usize,
    },

    /// `CS` — the server cleared the snapshot of one item
    /// [`docs/spec/04-notifications.md` §3.6].
    ///
    /// Sent in `DISTINCT` and `COMMAND` modes "so that it can clear its
    /// representation of the item (typically a list or a history, with these
    /// modes)". The caller should drop the list, history or row set it has
    /// accumulated for this item. What this crate keeps is unaffected; see
    /// [`SubscriptionManager::handle`].
    SnapshotCleared {
        /// **1-based** index of the item whose snapshot was cleared.
        item_index: usize,
    },

    /// `OV` — the server dropped updates for one item because of its own
    /// buffer limits [`docs/spec/04-notifications.md` §3.7].
    ///
    /// The lost updates are gone: nothing in this crate can reconstruct them,
    /// and the item's field values now reflect the surviving updates only.
    /// Surfacing the loss rather than absorbing it is the point
    /// (`docs/adr/0003-typed-event-stream-as-delivery-surface.md`).
    Overflow {
        /// **1-based** index of the item whose updates were dropped.
        item_index: usize,
        /// How many update events were dropped.
        dropped_count: u64,
    },

    /// `CONF` — the subscription's frequency configuration
    /// [`docs/spec/04-notifications.md` §3.8].
    ///
    /// Sent once at the start of a subscription — possibly before the
    /// activation — and again after every successful reconfiguration, even
    /// when the value did not change.
    Reconfigured {
        /// The maximum update frequency now in force for every item of the
        /// subscription, in updates per second, or `unlimited`.
        max_frequency: MaxFrequency,
        /// Whether updates for this subscription may be filtered.
        filtering: FilteringMode,
    },

    /// `UNSUB` — the subscription ended [`docs/spec/04-notifications.md` §3.4].
    ///
    /// Terminal: "after this notification, no more real-time updates related to
    /// the subscription items and fields will be sent".
    Unsubscribed,
}

// ---------------------------------------------------------------------------
// The §2.4 decision table
// ---------------------------------------------------------------------------

/// Decides whether a `U` carries snapshot content or a live change.
///
/// This is the specification's own decision table
/// [`docs/spec/04-notifications.md` §2.4], implemented row for row. Reproduced
/// here so the mapping is checkable against the source:
///
/// | `LS_snapshot` | `LS_mode` | `EOS` already received? | First notification? | The notification is... |
/// |---|---|---|---|---|
/// | `false` or missing | - | (cannot be received) | - | real-time update |
/// | `true` or a number | `RAW` | (cannot be received) | - | real-time update |
/// | " | `MERGE` | (cannot be received) | YES | snapshot |
/// | " | `MERGE` | (cannot be received) | NO | real-time update |
/// | " | `DISTINCT` | YES | - | real-time update |
/// | " | `DISTINCT` | NO | - | snapshot |
/// | " | `COMMAND` | YES | - | real-time update |
/// | " | `COMMAND` | NO | - | snapshot |
///
/// `eos_received` and `first_notification` are **per item**: `EOS` names one
/// item [`docs/spec/04-notifications.md` §3.5], and a `MERGE` snapshot is one
/// `U` per item [ibid.].
#[must_use]
pub(crate) const fn classify_update(
    snapshot_requested: bool,
    mode: SubscriptionMode,
    eos_received: bool,
    first_notification: bool,
) -> UpdateKind {
    // Row 1: `LS_snapshot` false or missing — no snapshot was asked for, so
    // nothing that arrives can be snapshot content, whatever the mode.
    if !snapshot_requested {
        return UpdateKind::RealTime;
    }

    match mode {
        // Row 2: `RAW` — "with RAW mode the snapshot is not supported"
        // [`docs/spec/04-notifications.md` §3.5], so a snapshot request cannot
        // produce snapshot content.
        SubscriptionMode::Raw => UpdateKind::RealTime,

        // Rows 3 and 4: `MERGE` — the snapshot is exactly one `U` per item, so
        // the first notification is it and every later one is live. `EOS` is
        // marked "(cannot be received)" for this mode and is therefore not
        // consulted.
        SubscriptionMode::Merge => {
            if first_notification {
                UpdateKind::Snapshot
            } else {
                UpdateKind::RealTime
            }
        }

        // Rows 5 to 8: `DISTINCT` and `COMMAND` — the snapshot may be several
        // `U` notifications and its end is announced by `EOS`
        // [`docs/spec/04-notifications.md` §3.5], so `EOS` alone decides and
        // "first notification" is not consulted.
        SubscriptionMode::Distinct | SubscriptionMode::Command => {
            if eos_received {
                UpdateKind::RealTime
            } else {
                UpdateKind::Snapshot
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Per-item state
// ---------------------------------------------------------------------------

/// One item's decoding baseline and the two flags the §2.4 table reads.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ItemTracker {
    /// The current value of every field — what unchanged-field markers and
    /// `^`-letter diffs are resolved against
    /// [`docs/spec/04-notifications.md` §2.2, §2.3].
    state: ItemState,
    /// Whether any `U` has been applied to this item yet — the "First
    /// notification?" column of [`docs/spec/04-notifications.md` §2.4].
    seen_update: bool,
    /// Whether `EOS` has arrived for this item — the "EOS notification already
    /// received?" column [`docs/spec/04-notifications.md` §2.4, §3.5].
    end_of_snapshot: bool,
}

impl ItemTracker {
    /// A fresh item whose schema has `field_count` fields.
    #[must_use]
    fn new(field_count: usize) -> Self {
        Self {
            state: ItemState::new(field_count),
            seen_update: false,
            end_of_snapshot: false,
        }
    }
}

/// The subscription's shape and item state, once the server has announced it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Active {
    schema: Arc<SubscriptionSchema>,
    items: Vec<ItemTracker>,
}

/// How far along the subscription is.
///
/// The counts a subscription needs in order to hold any state at all — how many
/// items, how many fields — arrive only with `SUBOK`/`SUBCMD`
/// [`docs/spec/04-notifications.md` §3.1, §3.2]. Before that line there is
/// genuinely nothing to size a [`ItemState`](crate::subscription::update::ItemState)
/// against, so [`Activation::Pending`] holds no item state rather than an empty
/// or guessed one.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Activation {
    /// No `SUBOK`/`SUBCMD` yet: the shape of the subscription is unknown.
    Pending,
    /// Activated: the shape is known and item state exists.
    Active(Active),
    /// `UNSUB` received; nothing further may arrive
    /// [`docs/spec/04-notifications.md` §3.4].
    Ended,
}

// ---------------------------------------------------------------------------
// The manager
// ---------------------------------------------------------------------------

/// The client-side state of one subscription.
///
/// Feed it the notifications the session layer routed to this subscription with
/// [`handle`](Self::handle); it returns the [`SubscriptionEvent`] each one
/// means, or [`None`] for a notification that does not concern a subscription.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SubscriptionManager {
    mode: SubscriptionMode,
    snapshot_requested: bool,
    declared_items: Vec<String>,
    declared_fields: Vec<String>,
    activation: Activation,
}

impl SubscriptionManager {
    /// Creates the state of a subscription the caller has just requested.
    ///
    /// - `mode` is `LS_mode`; it selects the row of the §2.4 decision table
    ///   every update is classified by.
    /// - `snapshot_requested` is whether `LS_snapshot` was sent as `true` or as
    ///   a number — the first column of that table. `false` or omitted is
    ///   `false` here [`docs/spec/04-notifications.md` §2.4].
    /// - `declared_items` and `declared_fields` are the caller's own names for
    ///   the items and fields, in order. TLCP never transmits them
    ///   [`docs/spec/04-notifications.md` §3.1], so they can only come from the
    ///   caller; pass empty slices when the subscription named a server-side
    ///   group and schema, and every position will be reported by its 1-based
    ///   index instead.
    #[must_use]
    pub(crate) fn new(
        mode: SubscriptionMode,
        snapshot_requested: bool,
        declared_items: Vec<String>,
        declared_fields: Vec<String>,
    ) -> Self {
        Self {
            mode,
            snapshot_requested,
            declared_items,
            declared_fields,
            activation: Activation::Pending,
        }
    }

    /// The subscription's mode.
    // This and the three accessors below have no non-test caller: the session
    // layer drives a manager through `handle` and reads the events it returns.
    // They are the inspection surface this module's tests assert against.
    #[allow(dead_code)]
    #[must_use]
    #[inline]
    pub(crate) const fn mode(&self) -> SubscriptionMode {
        self.mode
    }

    /// Whether `SUBOK`/`SUBCMD` has arrived, i.e. whether the subscription's
    /// shape is known [`docs/spec/04-notifications.md` §3.1, §3.2].
    #[allow(dead_code)]
    #[must_use]
    #[inline]
    pub(crate) const fn is_active(&self) -> bool {
        matches!(self.activation, Activation::Active(_))
    }

    /// Whether `UNSUB` has arrived [`docs/spec/04-notifications.md` §3.4].
    #[must_use]
    #[inline]
    pub(crate) const fn is_ended(&self) -> bool {
        matches!(self.activation, Activation::Ended)
    }

    /// How many items the subscription resolved to, once it is active.
    #[allow(dead_code)]
    #[must_use]
    #[inline]
    pub(crate) fn item_count(&self) -> Option<usize> {
        match &self.activation {
            Activation::Active(active) => Some(active.schema.item_count()),
            Activation::Pending | Activation::Ended => None,
        }
    }

    /// How many fields the schema resolved to, once the subscription is active.
    #[allow(dead_code)]
    #[must_use]
    #[inline]
    pub(crate) fn field_count(&self) -> Option<usize> {
        match &self.activation {
            Activation::Active(active) => Some(active.schema.field_count()),
            Activation::Pending | Activation::Ended => None,
        }
    }

    /// Interprets one notification.
    ///
    /// Returns the event it means, or [`None`] if the notification is not one
    /// this subscription cares about — session-level lines, message lines and
    /// anything else the parser produced. The `<subscription-ID>` argument is
    /// **not** checked: routing is the session layer's job, as the module
    /// documentation explains.
    ///
    /// # What each tag does to the state held here
    ///
    /// - `SUBOK` / `SUBCMD` — sizes the item state and records the schema
    ///   [§3.1, §3.2]. Receiving it again rebuilds everything from scratch,
    ///   which is what a subscription re-established on a replacement session
    ///   needs: the server restarts its flow, so the client must restart its
    ///   baseline.
    /// - `U` — applies the value list to the item's baseline [§2.3] and
    ///   classifies the result against the §2.4 table.
    /// - `EOS` — flips the item to real time for `DISTINCT` and `COMMAND`
    ///   [§2.4, §3.5]. Field values are untouched: end of snapshot is a
    ///   boundary, not a reset.
    /// - `CS` — reported to the caller, whose accumulated list, history or row
    ///   set for the item should be dropped [§3.6]. The field-value baseline
    ///   kept here is **not** cleared.
    ///
    ///   SPEC-AMBIGUITY (cs-resets-baseline): "after a `CS`, the spec does not
    ///   state whether the 'previous update of the same field' baseline used by
    ///   unchanged-field markers and `^`-letter diffs is reset for that item"
    ///   [`docs/spec/04-notifications.md` §3.6]. Choice: keep it. Clearing it
    ///   would leave the very next update undecodable — an empty token would
    ///   have no previous value to mean, and a `^`-letter diff no base to apply
    ///   to — so a server that sends `CS` followed by a delta would silently
    ///   produce null fields. Keeping the baseline can at worst report a stale
    ///   value for a field the server never mentions again; clearing it can
    ///   corrupt every field of the next update.
    /// - `OV` — reported; nothing local changes, because the dropped updates
    ///   never reached this client at all [§3.7].
    /// - `CONF` — reported; accepted before activation too, since it "is sent
    ///   at the start of a subscription" [§3.8].
    /// - `UNSUB` — ends the subscription; any later notification for it is an
    ///   error [§3.4].
    ///
    /// # Errors
    ///
    /// [`SubscriptionError`] when a notification contradicts what the server
    /// already said: an update before activation or after `UNSUB`, an item
    /// index outside the announced range, a `SUBCMD` whose key or command
    /// position is outside its own schema, or a value list that does not decode
    /// [`docs/spec/04-notifications.md` §2.3].
    pub(crate) fn handle(
        &mut self,
        notification: &Notification,
    ) -> Result<Option<SubscriptionEvent>, SubscriptionError> {
        match notification {
            // §3.1: `SUBOK,<subscription-ID>,<num-items>,<num-fields>`.
            Notification::SubscriptionOk {
                item_count,
                field_count,
                ..
            } => self.activate(*item_count, *field_count, None).map(Some),

            // §3.2: `SUBCMD` adds the key and command field positions.
            Notification::SubscriptionCommandOk {
                item_count,
                field_count,
                key_field_index,
                command_field_index,
                ..
            } => self
                .activate(
                    *item_count,
                    *field_count,
                    Some((*key_field_index, *command_field_index)),
                )
                .map(Some),

            // §2: `U,<subscription-ID>,<item>,<values>`.
            Notification::Update {
                item_index,
                raw_values,
                ..
            } => self.apply_update(*item_index, raw_values).map(Some),

            // §3.5: `EOS,<subscription-ID>,<item>`.
            Notification::EndOfSnapshot { item_index, .. } => {
                let index = self.item_position("EOS", *item_index)?;
                if let Activation::Active(active) = &mut self.activation
                    && let Some(tracker) = active.items.get_mut(index)
                {
                    tracker.end_of_snapshot = true;
                }
                Ok(Some(SubscriptionEvent::EndOfSnapshot {
                    item_index: to_one_based(index),
                }))
            }

            // §3.6: `CS,<subscription-ID>,<item>`. See the note above on why
            // the field-value baseline is deliberately preserved.
            Notification::ClearSnapshot { item_index, .. } => {
                let index = self.item_position("CS", *item_index)?;
                Ok(Some(SubscriptionEvent::SnapshotCleared {
                    item_index: to_one_based(index),
                }))
            }

            // §3.7: `OV,<subscription-ID>,<item>,<overflow-size>`.
            Notification::Overflow {
                item_index,
                dropped_count,
                ..
            } => {
                let index = self.item_position("OV", *item_index)?;
                Ok(Some(SubscriptionEvent::Overflow {
                    item_index: to_one_based(index),
                    dropped_count: *dropped_count,
                }))
            }

            // §3.8: `CONF,<subscription-ID>,<max-frequency>,<filtered|unfiltered>`.
            // "This notification is sent at the start of a subscription", so it
            // may legitimately precede `SUBOK`/`SUBCMD` and is accepted in any
            // state that is not ended.
            Notification::SubscriptionReconfigured {
                max_frequency,
                filtering,
                ..
            } => {
                if self.is_ended() {
                    return Err(SubscriptionError::Ended { tag: "CONF" });
                }
                Ok(Some(SubscriptionEvent::Reconfigured {
                    max_frequency: max_frequency.clone(),
                    filtering: filtering.clone(),
                }))
            }

            // §3.4: `UNSUB,<subscription-ID>`.
            Notification::Unsubscribed { .. } => {
                self.activation = Activation::Ended;
                Ok(Some(SubscriptionEvent::Unsubscribed))
            }

            // Not a subscription notification. Session and message lines are
            // handled by the session layer [`docs/spec/04-notifications.md`
            // §4, §5].
            _ => Ok(None),
        }
    }

    /// Handles `SUBOK` / `SUBCMD` [`docs/spec/04-notifications.md` §3.1, §3.2].
    fn activate(
        &mut self,
        item_count: u32,
        field_count: u32,
        command_fields: Option<(u32, u32)>,
    ) -> Result<SubscriptionEvent, SubscriptionError> {
        let items = to_usize(item_count);
        let fields = to_usize(field_count);

        // §3.2: the key and command positions are 1-based indices "of the field
        // that contains the key / the command", so they must name a field of
        // the schema the same line just declared.
        let zero_based = match command_fields {
            Some((key, command)) => {
                let key_index = command_field_position("key", key, fields)?;
                let command_index = command_field_position("command", command, fields)?;
                Some((key_index, command_index))
            }
            None => None,
        };

        let schema = Arc::new(SubscriptionSchema::new(
            items,
            fields,
            &self.declared_items,
            &self.declared_fields,
            zero_based,
        ));

        // Rebuilding unconditionally is what a re-established subscription
        // needs: on a replacement session the server restarts the flow from
        // scratch, snapshot included, so the previous baseline is not a
        // baseline for anything that follows.
        let mut trackers = Vec::with_capacity(items);
        for _ in 0..items {
            trackers.push(ItemTracker::new(fields));
        }

        self.activation = Activation::Active(Active {
            schema,
            items: trackers,
        });

        Ok(SubscriptionEvent::Activated {
            item_count: items,
            field_count: fields,
            command_fields: zero_based.map(|(key, command)| CommandFields {
                key: to_one_based(key),
                command: to_one_based(command),
            }),
        })
    }

    /// Handles `U` [`docs/spec/04-notifications.md` §2].
    fn apply_update(
        &mut self,
        item_index: u32,
        raw_values: &str,
    ) -> Result<SubscriptionEvent, SubscriptionError> {
        let snapshot_requested = self.snapshot_requested;
        let mode = self.mode;

        let active = match &mut self.activation {
            Activation::Active(active) => active,
            // §3.1: "After this notification, real-time updates related to the
            // subscription items and fields start to be sent" — so a `U` before
            // it has no schema width to be decoded against.
            Activation::Pending => return Err(SubscriptionError::NotActivated { tag: "U" }),
            Activation::Ended => return Err(SubscriptionError::Ended { tag: "U" }),
        };

        let item_count = active.items.len();
        let index = item_position(item_count, "U", item_index)?;
        let tracker = active
            .items
            .get_mut(index)
            .ok_or_else(|| item_out_of_range("U", item_index, item_count))?;

        // §2.4, "First notification?": read before this update is recorded.
        let first_notification = !tracker.seen_update;
        let kind = classify_update(
            snapshot_requested,
            mode,
            tracker.end_of_snapshot,
            first_notification,
        );

        // §2.3: the normative decoding algorithm, which owns the markers, the
        // unchanged runs and the diff formats. A rejected list leaves the item
        // untouched, so `seen_update` is only set once the update committed.
        let outcome = tracker.state.apply(raw_values)?;
        tracker.seen_update = true;

        let field_count = tracker.state.field_count();
        let mut values = Vec::with_capacity(field_count);
        for position in 0..field_count {
            // `None` is null, `Some(text)` is text — possibly empty, which is a
            // different thing [`docs/spec/04-notifications.md` §2.2].
            values.push(tracker.state.field(position).flatten().map(str::to_owned));
        }

        Ok(SubscriptionEvent::Update(ItemUpdate::new(
            Arc::clone(&active.schema),
            index,
            kind,
            values,
            outcome.changed_fields().to_vec(),
        )))
    }

    /// Validates the `<item>` argument of `EOS`, `CS` or `OV` and returns its
    /// 0-based index [`docs/spec/04-notifications.md` §3.5, §3.6, §3.7].
    fn item_position(
        &self,
        tag: &'static str,
        item_index: u32,
    ) -> Result<usize, SubscriptionError> {
        match &self.activation {
            Activation::Active(active) => item_position(active.items.len(), tag, item_index),
            Activation::Pending => Err(SubscriptionError::NotActivated { tag }),
            Activation::Ended => Err(SubscriptionError::Ended { tag }),
        }
    }
}

/// Converts a 1-based wire item index into a 0-based index, checking it against
/// the item count `SUBOK`/`SUBCMD` announced
/// [`docs/spec/04-notifications.md` §3.1].
fn item_position(
    item_count: usize,
    tag: &'static str,
    item_index: u32,
) -> Result<usize, SubscriptionError> {
    // §2.1: the item index is 1-based, so `0` names no item.
    let index = to_usize(item_index)
        .checked_sub(1)
        .ok_or_else(|| item_out_of_range(tag, item_index, item_count))?;
    if index >= item_count {
        return Err(item_out_of_range(tag, item_index, item_count));
    }
    Ok(index)
}

/// Converts a 1-based `SUBCMD` field position into a 0-based index, checking it
/// against the field count of the same line
/// [`docs/spec/04-notifications.md` §3.2].
fn command_field_position(
    role: &'static str,
    position: u32,
    field_count: usize,
) -> Result<usize, SubscriptionError> {
    let out_of_range = || SubscriptionError::CommandFieldOutOfRange {
        role,
        position: u64::from(position),
        field_count,
    };
    let index = to_usize(position).checked_sub(1).ok_or_else(out_of_range)?;
    if index >= field_count {
        return Err(out_of_range());
    }
    Ok(index)
}

/// Widens a wire count to a [`usize`].
///
/// Saturates rather than failing, which on every platform this crate supports
/// (`usize` at least 32 bits) is the identity. A hypothetical 16-bit target
/// could not hold the count in memory anyway, and the saturated value is then
/// caught by the range checks above.
#[must_use]
#[inline]
fn to_usize(value: u32) -> usize {
    usize::try_from(value).unwrap_or(usize::MAX)
}

/// Converts a 0-based index back to the 1-based numbering the specification
/// and this crate's public surface both use
/// [`docs/spec/04-notifications.md` §2.1].
#[must_use]
#[inline]
const fn to_one_based(index: usize) -> usize {
    // Cannot overflow: every caller passes an index into a `Vec`.
    index + 1
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::subscription::item_update::{FieldValue, ItemCommand};

    // -- Fixtures ------------------------------------------------------------

    fn names(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    /// `SUBOK,3,<items>,<fields>` [`docs/spec/04-notifications.md` §3.1].
    fn subok(item_count: u32, field_count: u32) -> Notification {
        Notification::SubscriptionOk {
            subscription_id: 3,
            item_count,
            field_count,
        }
    }

    /// `SUBCMD,3,<items>,<fields>,<key>,<command>`
    /// [`docs/spec/04-notifications.md` §3.2].
    fn subcmd(item_count: u32, field_count: u32, key: u32, command: u32) -> Notification {
        Notification::SubscriptionCommandOk {
            subscription_id: 3,
            item_count,
            field_count,
            key_field_index: key,
            command_field_index: command,
        }
    }

    /// `U,3,<item>,<values>` [`docs/spec/04-notifications.md` §2.1].
    fn update(item_index: u32, raw_values: &str) -> Notification {
        Notification::Update {
            subscription_id: 3,
            item_index,
            raw_values: raw_values.to_owned(),
        }
    }

    fn eos(item_index: u32) -> Notification {
        Notification::EndOfSnapshot {
            subscription_id: 3,
            item_index,
        }
    }

    fn clear_snapshot(item_index: u32) -> Notification {
        Notification::ClearSnapshot {
            subscription_id: 3,
            item_index,
        }
    }

    fn overflow(item_index: u32, dropped_count: u64) -> Notification {
        Notification::Overflow {
            subscription_id: 3,
            item_index,
            dropped_count,
        }
    }

    fn conf() -> Notification {
        Notification::SubscriptionReconfigured {
            subscription_id: 3,
            max_frequency: MaxFrequency::Limited {
                updates_per_second: "3.0".to_owned(),
            },
            filtering: FilteringMode::Filtered,
        }
    }

    fn unsub() -> Notification {
        Notification::Unsubscribed { subscription_id: 3 }
    }

    /// Drives a notification and unwraps the [`ItemUpdate`] it must produce.
    fn expect_update(manager: &mut SubscriptionManager, notification: &Notification) -> ItemUpdate {
        match manager.handle(notification) {
            Ok(Some(SubscriptionEvent::Update(update))) => update,
            other => panic!("expected an update event, got {other:?}"),
        }
    }

    /// A `MERGE` subscription over the §2.5 quote schema, already activated.
    fn quote_manager(snapshot_requested: bool) -> SubscriptionManager {
        let mut manager = SubscriptionManager::new(
            SubscriptionMode::Merge,
            snapshot_requested,
            names(&["item1"]),
            names(&[
                "timestamp",
                "price",
                "change",
                "minimum",
                "maximum",
                "bid",
                "ask",
                "open",
                "close",
                "status",
            ]),
        );
        assert!(manager.handle(&subok(1, 10)).is_ok());
        manager
    }

    // -- §2.4 decision table, every row --------------------------------------

    #[test]
    fn test_classify_row_1_no_snapshot_requested_is_always_real_time_s2_4() {
        // Row 1: `LS_snapshot` false or missing, any mode.
        for mode in [
            SubscriptionMode::Raw,
            SubscriptionMode::Merge,
            SubscriptionMode::Distinct,
            SubscriptionMode::Command,
        ] {
            for eos_received in [false, true] {
                for first in [false, true] {
                    assert_eq!(
                        classify_update(false, mode, eos_received, first),
                        UpdateKind::RealTime,
                        "mode {mode:?}, eos {eos_received}, first {first}"
                    );
                }
            }
        }
    }

    #[test]
    fn test_classify_row_2_raw_is_always_real_time_s2_4() {
        // Row 2: snapshot requested, `RAW`.
        for first in [false, true] {
            assert_eq!(
                classify_update(true, SubscriptionMode::Raw, false, first),
                UpdateKind::RealTime
            );
        }
    }

    #[test]
    fn test_classify_row_3_merge_first_notification_is_snapshot_s2_4() {
        assert_eq!(
            classify_update(true, SubscriptionMode::Merge, false, true),
            UpdateKind::Snapshot
        );
    }

    #[test]
    fn test_classify_row_4_merge_later_notification_is_real_time_s2_4() {
        assert_eq!(
            classify_update(true, SubscriptionMode::Merge, false, false),
            UpdateKind::RealTime
        );
    }

    #[test]
    fn test_classify_row_5_distinct_after_eos_is_real_time_s2_4() {
        for first in [false, true] {
            assert_eq!(
                classify_update(true, SubscriptionMode::Distinct, true, first),
                UpdateKind::RealTime
            );
        }
    }

    #[test]
    fn test_classify_row_6_distinct_before_eos_is_snapshot_s2_4() {
        for first in [false, true] {
            assert_eq!(
                classify_update(true, SubscriptionMode::Distinct, false, first),
                UpdateKind::Snapshot
            );
        }
    }

    #[test]
    fn test_classify_row_7_command_after_eos_is_real_time_s2_4() {
        for first in [false, true] {
            assert_eq!(
                classify_update(true, SubscriptionMode::Command, true, first),
                UpdateKind::RealTime
            );
        }
    }

    #[test]
    fn test_classify_row_8_command_before_eos_is_snapshot_s2_4() {
        for first in [false, true] {
            assert_eq!(
                classify_update(true, SubscriptionMode::Command, false, first),
                UpdateKind::Snapshot
            );
        }
    }

    // -- Snapshot then real time, per mode -----------------------------------

    #[test]
    fn test_merge_first_update_is_snapshot_then_real_time() {
        let mut manager = quote_manager(true);
        let first = expect_update(
            &mut manager,
            &update(1, "20:00:33|3.04|0.0|2.41|3.67|3.03|3.04|#|#|$"),
        );
        assert!(first.is_snapshot());

        let second = expect_update(
            &mut manager,
            &update(1, "20:00:54|3.07|0.98|||3.06|3.07|||Suspended"),
        );
        assert_eq!(second.kind(), UpdateKind::RealTime);
    }

    #[test]
    fn test_merge_without_snapshot_request_is_never_snapshot() {
        let mut manager = quote_manager(false);
        let first = expect_update(
            &mut manager,
            &update(1, "20:00:33|3.04|0.0|2.41|3.67|3.03|3.04|#|#|$"),
        );
        assert_eq!(first.kind(), UpdateKind::RealTime);
    }

    #[test]
    fn test_merge_first_notification_is_tracked_per_item() {
        let mut manager = SubscriptionManager::new(
            SubscriptionMode::Merge,
            true,
            names(&["a", "b"]),
            names(&["v"]),
        );
        assert!(manager.handle(&subok(2, 1)).is_ok());

        assert!(expect_update(&mut manager, &update(1, "1")).is_snapshot());
        // The second item has still had no update of its own, so its first one
        // is its snapshot [`docs/spec/04-notifications.md` §2.4].
        assert!(expect_update(&mut manager, &update(2, "2")).is_snapshot());
        assert!(!expect_update(&mut manager, &update(1, "3")).is_snapshot());
    }

    #[test]
    fn test_distinct_snapshot_ends_at_eos() {
        let mut manager = SubscriptionManager::new(
            SubscriptionMode::Distinct,
            true,
            names(&["chat"]),
            names(&["line"]),
        );
        assert!(manager.handle(&subok(1, 1)).is_ok());

        // A `DISTINCT` snapshot may be several `U` notifications
        // [`docs/spec/04-notifications.md` §3.5].
        assert!(expect_update(&mut manager, &update(1, "first")).is_snapshot());
        assert!(expect_update(&mut manager, &update(1, "second")).is_snapshot());

        assert_eq!(
            manager.handle(&eos(1)),
            Ok(Some(SubscriptionEvent::EndOfSnapshot { item_index: 1 }))
        );
        assert!(!expect_update(&mut manager, &update(1, "third")).is_snapshot());
    }

    #[test]
    fn test_eos_is_tracked_per_item() {
        let mut manager = SubscriptionManager::new(
            SubscriptionMode::Distinct,
            true,
            names(&["a", "b"]),
            names(&["v"]),
        );
        assert!(manager.handle(&subok(2, 1)).is_ok());
        assert!(manager.handle(&eos(1)).is_ok());

        assert!(!expect_update(&mut manager, &update(1, "x")).is_snapshot());
        // `EOS` names one item [`docs/spec/04-notifications.md` §3.5]; the
        // other is still in its snapshot.
        assert!(expect_update(&mut manager, &update(2, "y")).is_snapshot());
    }

    #[test]
    fn test_raw_updates_are_real_time_even_before_any_eos() {
        let mut manager =
            SubscriptionManager::new(SubscriptionMode::Raw, true, names(&["tick"]), names(&["v"]));
        assert!(manager.handle(&subok(1, 1)).is_ok());
        assert!(!expect_update(&mut manager, &update(1, "x")).is_snapshot());
    }

    // -- Activation ----------------------------------------------------------

    #[test]
    fn test_subok_announces_the_shape_s3_1() {
        let mut manager =
            SubscriptionManager::new(SubscriptionMode::Merge, false, Vec::new(), Vec::new());
        assert!(!manager.is_active());
        assert_eq!(manager.field_count(), None);

        // The spec's own example: `SUBOK,3,1,10`
        // [`docs/spec/04-notifications.md` §3.1].
        assert_eq!(
            manager.handle(&subok(1, 10)),
            Ok(Some(SubscriptionEvent::Activated {
                item_count: 1,
                field_count: 10,
                command_fields: None,
            }))
        );
        assert!(manager.is_active());
        assert_eq!(manager.item_count(), Some(1));
        assert_eq!(manager.field_count(), Some(10));
        assert_eq!(manager.mode(), SubscriptionMode::Merge);
    }

    #[test]
    fn test_subcmd_announces_key_and_command_positions_s3_2() {
        let mut manager =
            SubscriptionManager::new(SubscriptionMode::Command, true, Vec::new(), Vec::new());
        // The spec's own example: `SUBCMD,3,1,10,1,2`
        // [`docs/spec/04-notifications.md` §3.2].
        assert_eq!(
            manager.handle(&subcmd(1, 10, 1, 2)),
            Ok(Some(SubscriptionEvent::Activated {
                item_count: 1,
                field_count: 10,
                command_fields: Some(CommandFields { key: 1, command: 2 }),
            }))
        );
    }

    #[test]
    fn test_update_before_activation_is_an_error() {
        let mut manager =
            SubscriptionManager::new(SubscriptionMode::Merge, false, Vec::new(), Vec::new());
        // The schema width is unknown until `SUBOK`
        // [`docs/spec/04-notifications.md` §3.1], so the value list cannot be
        // decoded.
        assert_eq!(
            manager.handle(&update(1, "a|b")),
            Err(SubscriptionError::NotActivated { tag: "U" })
        );
        assert_eq!(
            manager.handle(&eos(1)),
            Err(SubscriptionError::NotActivated { tag: "EOS" })
        );
    }

    #[test]
    fn test_reactivation_rebuilds_the_item_state() {
        let mut manager = quote_manager(true);
        let _ = expect_update(
            &mut manager,
            &update(1, "20:00:33|3.04|0.0|2.41|3.67|3.03|3.04|#|#|$"),
        );

        // A subscription re-established on a replacement session gets a fresh
        // `SUBOK`; the server restarts its flow, so the baseline restarts too.
        assert!(manager.handle(&subok(1, 10)).is_ok());
        let after = expect_update(
            &mut manager,
            &update(1, "20:00:34|3.05|0.1|2.41|3.67|3.03|3.04|#|#|$"),
        );
        // First notification again, hence snapshot again
        // [`docs/spec/04-notifications.md` §2.4].
        assert!(after.is_snapshot());
        assert_eq!(after.changed_count(), 10);
    }

    #[test]
    fn test_subcmd_with_out_of_range_key_is_an_error() {
        let mut manager =
            SubscriptionManager::new(SubscriptionMode::Command, true, Vec::new(), Vec::new());
        assert_eq!(
            manager.handle(&subcmd(1, 3, 4, 2)),
            Err(SubscriptionError::CommandFieldOutOfRange {
                role: "key",
                position: 4,
                field_count: 3,
            })
        );
        assert_eq!(
            manager.handle(&subcmd(1, 3, 0, 2)),
            Err(SubscriptionError::CommandFieldOutOfRange {
                role: "key",
                position: 0,
                field_count: 3,
            })
        );
        assert_eq!(
            manager.handle(&subcmd(1, 3, 1, 9)),
            Err(SubscriptionError::CommandFieldOutOfRange {
                role: "command",
                position: 9,
                field_count: 3,
            })
        );
    }

    #[test]
    fn test_item_index_outside_the_announced_range_is_an_error() {
        let mut manager = quote_manager(true);
        // §2.1: the item index is 1-based, so `0` names no item.
        assert!(matches!(
            manager.handle(&update(0, "a")),
            Err(SubscriptionError::ItemOutOfRange { .. })
        ));
        assert!(matches!(
            manager.handle(&update(2, "a")),
            Err(SubscriptionError::ItemOutOfRange { .. })
        ));
        assert!(matches!(
            manager.handle(&eos(2)),
            Err(SubscriptionError::ItemOutOfRange { tag: "EOS", .. })
        ));
        assert!(matches!(
            manager.handle(&clear_snapshot(7)),
            Err(SubscriptionError::ItemOutOfRange { tag: "CS", .. })
        ));
        assert!(matches!(
            manager.handle(&overflow(7, 1)),
            Err(SubscriptionError::ItemOutOfRange { tag: "OV", .. })
        ));
    }

    // -- Unchanged fields ----------------------------------------------------

    #[test]
    fn test_unchanged_fields_are_absent_from_changed_fields_s2_5() {
        let mut manager = quote_manager(true);
        let _ = expect_update(
            &mut manager,
            &update(1, "20:00:33|3.04|0.0|2.41|3.67|3.03|3.04|#|#|$"),
        );

        // §2.5 example 2: "This update contains 4 unchanged fields: minimum,
        // maximum, open and close."
        let second = expect_update(
            &mut manager,
            &update(1, "20:00:54|3.07|0.98|||3.06|3.07|||Suspended"),
        );
        let changed: Vec<&str> = second.changed_fields().map(|field| field.name()).collect();
        assert_eq!(
            changed,
            vec!["timestamp", "price", "change", "bid", "ask", "status"]
        );
        assert!(!second.is_field_changed_by_name("minimum"));
        // Unchanged does not mean unreadable: the previous value is still there.
        assert_eq!(
            second.field_by_name("minimum"),
            Some(FieldValue::Text("2.41"))
        );
    }

    #[test]
    fn test_caret_run_of_unchanged_fields_s2_5() {
        let mut manager = quote_manager(true);
        let _ = expect_update(
            &mut manager,
            &update(1, "20:00:33|3.04|0.0|2.41|3.67|3.03|3.04|#|#|$"),
        );
        // §2.5 example 4: `^4` covers `price`, `change`, `minimum`, `maximum`.
        let fourth = expect_update(&mut manager, &update(1, "20:04:40|^4|3.02|3.03|||"));
        let changed: Vec<usize> = fourth
            .changed_fields()
            .map(|field| field.position())
            .collect();
        assert_eq!(changed, vec![1, 6, 7]);
        assert_eq!(
            fourth.field_by_name("price"),
            Some(FieldValue::Text("3.04"))
        );
    }

    #[test]
    fn test_null_and_empty_survive_to_the_caller() {
        let mut manager = quote_manager(true);
        // §2.5 example 1: `open` and `close` are null (`#`), `status` is the
        // empty string (`$`).
        let first = expect_update(
            &mut manager,
            &update(1, "20:00:33|3.04|0.0|2.41|3.67|3.03|3.04|#|#|$"),
        );
        assert_eq!(first.field_by_name("open"), Some(FieldValue::Null));
        assert_eq!(first.field_by_name("close"), Some(FieldValue::Null));
        assert_eq!(first.field_by_name("status"), Some(FieldValue::Text("")));
        assert_ne!(first.field_by_name("open"), first.field_by_name("status"));
        // Both were carried, so both count as changed
        // [`docs/spec/04-notifications.md` §2.5].
        assert!(first.is_field_changed_by_name("open"));
        assert!(first.is_field_changed_by_name("status"));
        // A name nobody declared resolves to nothing.
        assert_eq!(first.field_by_name("volume"), None);
    }

    #[test]
    fn test_a_malformed_value_list_is_reported_and_changes_nothing() {
        let mut manager = quote_manager(true);
        let _ = expect_update(
            &mut manager,
            &update(1, "20:00:33|3.04|0.0|2.41|3.67|3.03|3.04|#|#|$"),
        );
        // Fewer tokens than the schema has fields
        // [`docs/spec/04-notifications.md` §2.3].
        assert!(matches!(
            manager.handle(&update(1, "a|b")),
            Err(SubscriptionError::Value(ProtocolError::FieldValue { .. }))
        ));
        // The item is intact, and still counts as having seen an update.
        let next = expect_update(&mut manager, &update(1, "^10"));
        assert_eq!(next.field_by_name("price"), Some(FieldValue::Text("3.04")));
        assert!(!next.is_snapshot());
    }

    // -- COMMAND mode --------------------------------------------------------

    /// A one-item `COMMAND` subscription: key, command, quantity.
    fn portfolio_manager() -> SubscriptionManager {
        let mut manager = SubscriptionManager::new(
            SubscriptionMode::Command,
            true,
            names(&["portfolio1"]),
            names(&["key", "command", "qty"]),
        );
        // `SUBCMD,3,1,3,1,2` [`docs/spec/04-notifications.md` §3.2].
        assert!(manager.handle(&subcmd(1, 3, 1, 2)).is_ok());
        manager
    }

    #[test]
    fn test_command_mode_add_update_delete() {
        let mut manager = portfolio_manager();

        let added = expect_update(&mut manager, &update(1, "AAPL|ADD|100"));
        assert!(added.is_command_mode());
        assert!(
            added.is_snapshot(),
            "no `EOS` yet, so still snapshot [§2.4]"
        );
        assert_eq!(added.key(), Some(FieldValue::Text("AAPL")));
        assert_eq!(added.command(), Some(ItemCommand::Add));
        assert_eq!(added.field_by_name("qty"), Some(FieldValue::Text("100")));

        assert!(manager.handle(&eos(1)).is_ok());

        // An `UPDATE` that leaves the key unchanged: the empty token resolves
        // against the previous update of the same item
        // [`docs/spec/04-notifications.md` §2.2, §3.3].
        let changed = expect_update(&mut manager, &update(1, "|UPDATE|120"));
        assert!(!changed.is_snapshot());
        assert_eq!(changed.key(), Some(FieldValue::Text("AAPL")));
        assert_eq!(changed.command(), Some(ItemCommand::Update));
        assert!(!changed.is_field_changed_by_name("key"));
        assert_eq!(changed.field_by_name("qty"), Some(FieldValue::Text("120")));

        let deleted = expect_update(&mut manager, &update(1, "|DELETE|"));
        assert_eq!(deleted.command(), Some(ItemCommand::Delete));
        assert_eq!(deleted.key(), Some(FieldValue::Text("AAPL")));
        // SPEC-AMBIGUITY (command-delta-baseline): a DELETE tells the caller to
        // drop its row; the field baseline kept here is untouched, so the next
        // update's unchanged markers still resolve.
        let after = expect_update(&mut manager, &update(1, "MSFT|ADD|5"));
        assert_eq!(after.key(), Some(FieldValue::Text("MSFT")));
        assert_eq!(after.field_by_name("qty"), Some(FieldValue::Text("5")));
    }

    #[test]
    fn test_command_mode_null_command_field() {
        let mut manager = portfolio_manager();
        let update = expect_update(&mut manager, &update(1, "AAPL|#|100"));
        // A null command names no operation
        // [`docs/spec/04-notifications.md` §3.3].
        assert_eq!(update.command(), None);
        assert_eq!(update.key(), Some(FieldValue::Text("AAPL")));
    }

    // -- EOS, CS, OV, CONF, UNSUB --------------------------------------------

    #[test]
    fn test_eos_does_not_disturb_field_values_s3_5() {
        let mut manager = quote_manager(true);
        let _ = expect_update(
            &mut manager,
            &update(1, "20:00:33|3.04|0.0|2.41|3.67|3.03|3.04|#|#|$"),
        );
        assert_eq!(
            manager.handle(&eos(1)),
            Ok(Some(SubscriptionEvent::EndOfSnapshot { item_index: 1 }))
        );
        let next = expect_update(&mut manager, &update(1, "^10"));
        assert_eq!(next.field_by_name("price"), Some(FieldValue::Text("3.04")));
    }

    #[test]
    fn test_clear_snapshot_is_reported_and_keeps_the_decoding_baseline_s3_6() {
        let mut manager = SubscriptionManager::new(
            SubscriptionMode::Distinct,
            true,
            names(&["chat"]),
            names(&["line", "author"]),
        );
        assert!(manager.handle(&subok(1, 2)).is_ok());
        let _ = expect_update(&mut manager, &update(1, "hello|ana"));

        // `CS,3,1` [`docs/spec/04-notifications.md` §3.6].
        assert_eq!(
            manager.handle(&clear_snapshot(1)),
            Ok(Some(SubscriptionEvent::SnapshotCleared { item_index: 1 }))
        );

        // SPEC-AMBIGUITY (cs-resets-baseline): the baseline is deliberately
        // kept, so an unchanged token after a `CS` still resolves.
        let next = expect_update(&mut manager, &update(1, "bye|"));
        assert_eq!(next.field_by_name("author"), Some(FieldValue::Text("ana")));
        assert!(!next.is_field_changed_by_name("author"));
    }

    #[test]
    fn test_overflow_is_reported_verbatim_s3_7() {
        let mut manager = quote_manager(false);
        // `OV,3,1,5` [`docs/spec/04-notifications.md` §3.7].
        assert_eq!(
            manager.handle(&overflow(1, 5)),
            Ok(Some(SubscriptionEvent::Overflow {
                item_index: 1,
                dropped_count: 5,
            }))
        );
    }

    #[test]
    fn test_overflow_does_not_touch_item_state_s3_7() {
        let mut manager = quote_manager(true);
        let _ = expect_update(
            &mut manager,
            &update(1, "20:00:33|3.04|0.0|2.41|3.67|3.03|3.04|#|#|$"),
        );
        assert!(manager.handle(&overflow(1, 3)).is_ok());
        // The dropped updates never reached this client; what did survive is
        // still the baseline.
        let next = expect_update(&mut manager, &update(1, "^10"));
        assert_eq!(next.field_by_name("price"), Some(FieldValue::Text("3.04")));
        assert!(next.changed_fields().next().is_none());
    }

    #[test]
    fn test_conf_is_accepted_before_activation_s3_8() {
        let mut manager =
            SubscriptionManager::new(SubscriptionMode::Merge, false, Vec::new(), Vec::new());
        // "This notification is sent at the start of a subscription"
        // [`docs/spec/04-notifications.md` §3.8], which may precede `SUBOK`.
        assert_eq!(
            manager.handle(&conf()),
            Ok(Some(SubscriptionEvent::Reconfigured {
                max_frequency: MaxFrequency::Limited {
                    updates_per_second: "3.0".to_owned(),
                },
                filtering: FilteringMode::Filtered,
            }))
        );
        assert!(!manager.is_active());
    }

    #[test]
    fn test_unsub_ends_the_subscription_s3_4() {
        let mut manager = quote_manager(true);
        assert_eq!(
            manager.handle(&unsub()),
            Ok(Some(SubscriptionEvent::Unsubscribed))
        );
        assert!(manager.is_ended());
        // "After this notification, no more real-time updates related to the
        // subscription items and fields will be sent"
        // [`docs/spec/04-notifications.md` §3.4].
        assert_eq!(
            manager.handle(&update(1, "a")),
            Err(SubscriptionError::Ended { tag: "U" })
        );
        assert_eq!(
            manager.handle(&conf()),
            Err(SubscriptionError::Ended { tag: "CONF" })
        );
    }

    #[test]
    fn test_non_subscription_notifications_are_ignored() {
        let mut manager = quote_manager(true);
        assert_eq!(manager.handle(&Notification::Probe), Ok(None));
        assert_eq!(
            manager.handle(&Notification::Sync {
                elapsed_seconds: 120
            }),
            Ok(None)
        );
        assert_eq!(
            manager.handle(&Notification::ServerName {
                name: "Lightstreamer HTTP Server".to_owned()
            }),
            Ok(None)
        );
    }

    // -- Names ---------------------------------------------------------------

    #[test]
    fn test_undeclared_names_report_positions() {
        let mut manager =
            SubscriptionManager::new(SubscriptionMode::Merge, false, Vec::new(), Vec::new());
        assert!(manager.handle(&subok(2, 2)).is_ok());
        let update = expect_update(&mut manager, &update(2, "a|b"));
        assert_eq!(update.item_index(), 2);
        assert_eq!(update.item_name(), "2");
        assert_eq!(update.declared_item_name(), None);
        assert_eq!(update.field_name(1), Some("1"));
    }

    #[test]
    fn test_declared_names_reach_the_update() {
        let mut manager = quote_manager(true);
        let first = expect_update(
            &mut manager,
            &update(1, "20:00:33|3.04|0.0|2.41|3.67|3.03|3.04|#|#|$"),
        );
        assert_eq!(first.item_name(), "item1");
        assert_eq!(first.field_name(10), Some("status"));
        assert_eq!(first.field_position("status"), Some(10));
        assert_eq!(first.field_position("nope"), None);
    }
}
