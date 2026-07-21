//! Building this crate's event payloads by hand, for the tests of crates that
//! depend on it. Enabled by the off-by-default `test-util` feature.
//!
//! A crate built on this one has its own layer between the events and its
//! domain: a parser that reads [`FieldValue`]s out of an [`ItemUpdate`], a
//! rule that decides whether derived state survived a reconnection, a policy
//! for what to do with a message that came back refused. That layer deserves
//! unit tests, and those tests want nothing to do with a socket, a session or
//! a server.
//!
//! Without this module they cannot have one. The payloads this crate delivers
//! are either assembled from private state ([`ItemUpdate`], from a
//! subscription's schema) or declared `#[non_exhaustive]`, which stops any
//! other crate from constructing them at all. Both are deliberate: the first
//! so that nothing outside this crate can forge an update a real session never
//! produced, the second so that a field can be added without breaking every
//! consumer. Neither is a reason a consumer should be unable to write a test,
//! and a feature that exists only in test builds costs the stable surface
//! nothing:
//!
//! ```toml
//! [dev-dependencies]
//! lightstreamer-rs = { version = "1", features = ["test-util"] }
//! ```
//!
//! Cargo enables the feature only when it builds your tests, so your release
//! build never contains it.
//!
//! # What is here
//!
//! | To build | Use |
//! |---|---|
//! | [`ItemUpdate`] — one item's new state | [`ItemUpdateBuilder`] |
//! | [`Connected`] — a bind and what it means for your state | [`ConnectedBuilder`] |
//! | [`Recovery`] — where a recovered flow resumed | [`recovery`] |
//! | [`Resubscribed`] — one subscription re-created | [`ResubscribedBuilder`] |
//! | [`SubscriptionId`] — an id a test can name | [`subscription_id`] |
//! | [`MessageOutcome`] — one message's fate | [`MessageOutcomeBuilder`] |
//! | [`CommandFields`] — where the key and command sit | [`command_fields`] |
//!
//! Anything not listed is already constructible: the event enums themselves
//! ([`SessionEvent`](crate::SessionEvent),
//! [`SubscriptionEvent`](crate::SubscriptionEvent)) are `#[non_exhaustive]`
//! only for *matching*, so you can build any variant, and
//! [`ServerError::new`](crate::ServerError::new) is public. A builder appears
//! here when a field has a sensible default worth omitting; where every field
//! is required and means something, there is a plain function instead.
//!
//! # Null is still not empty
//!
//! [`ItemUpdateBuilder`] takes a [`FieldValue`], not an `Option<&str>` and not
//! a `&str`, for exactly the reason the rest of this crate does: `#` and `$`
//! are different values [`docs/spec/04-notifications.md` §2.2], and the single
//! most valuable thing a consumer can test is that its own parser tells them
//! apart. Write [`FieldValue::Null`] for a field the server had no value for
//! and [`FieldValue::Text`]`("")` for one it deliberately blanked.
//!
//! # Misuse panics
//!
//! These are builders for tests, so they treat a nonsensical payload as a test
//! author's mistake and fail loudly rather than quietly producing something
//! that cannot come off a wire. Each item documents what it rejects; where a
//! contradiction could be derived away instead of rejected, it is — [`recovery`]
//! computes its own outcome, so a [`Recovery`] that disagrees with its own
//! progressives is not representable.
//!
//! # What is deliberately not here
//!
//! [`Updates`](crate::Updates) and [`SessionEvents`](crate::SessionEvents), the
//! streams themselves. A fake `Updates` would have to fake the control channel
//! that unsubscribes when the stream is dropped, and a half-faithful fake of a
//! lifecycle is worse than none. Take the events rather than the stream:
//!
//! ```
//! use futures_util::{Stream, StreamExt};
//! use lightstreamer_rs::SubscriptionEvent;
//!
//! // Not `async fn consume(updates: Updates)` — this is testable with
//! // `futures_util::stream::iter` and works just as well with the real thing.
//! async fn consume(mut events: impl Stream<Item = SubscriptionEvent> + Unpin) -> usize {
//!     let mut updates = 0;
//!     while let Some(event) = events.next().await {
//!         if matches!(event, SubscriptionEvent::Update(_)) {
//!             updates += 1;
//!         }
//!     }
//!     updates
//! }
//! ```

use std::cmp::Ordering;
use std::sync::Arc;
use std::time::Duration;

use crate::client::{
    CommandFields, Connected, Continuity, MessageOutcome, MessageResult, Recovery, RecoveryOutcome,
    Resubscribed, SubscriptionId,
};
use crate::subscription::item_update::{FieldValue, ItemUpdate, SubscriptionSchema, UpdateKind};

/// Builds an [`ItemUpdate`] with no session behind it.
///
/// The update names one item and one field schema; every field starts null
/// and unchanged, and [`changed`](Self::changed) and
/// [`unchanged`](Self::unchanged) fill in the ones a test cares about.
///
/// # Example: testing your own parser
///
/// ```
/// use lightstreamer_rs::test_util::ItemUpdateBuilder;
/// use lightstreamer_rs::{FieldValue, ItemUpdate, UpdateKind};
///
/// // Your crate's domain type ...
/// #[derive(Debug, PartialEq)]
/// struct Quote {
///     bid: Option<String>,
///     offer: Option<String>,
/// }
///
/// // ... and the parser you actually want to test.
/// fn parse(update: &ItemUpdate) -> Quote {
///     let text = |name| {
///         update
///             .field_by_name(name)
///             .and_then(FieldValue::text)
///             .map(str::to_owned)
///     };
///     Quote {
///         bid: text("BID"),
///         offer: text("OFFER"),
///     }
/// }
///
/// // A payload the server really sent: a new bid, no offer at all.
/// let update = ItemUpdateBuilder::new("CS.D.EURUSD.CFD.IP", ["BID", "OFFER", "STATUS"])
///     .changed("BID", FieldValue::Text("1.0921"))
///     .changed("OFFER", FieldValue::Null)
///     .unchanged("STATUS", FieldValue::Text(""))
///     .build();
///
/// assert_eq!(
///     parse(&update),
///     Quote {
///         bid: Some("1.0921".to_owned()),
///         offer: None,
///     }
/// );
///
/// // Null and empty stay apart, and so do changed and unchanged.
/// assert_eq!(update.field_by_name("OFFER"), Some(FieldValue::Null));
/// assert_eq!(update.field_by_name("STATUS"), Some(FieldValue::Text("")));
/// assert!(update.is_field_changed_by_name("OFFER"));
/// assert!(!update.is_field_changed_by_name("STATUS"));
/// assert_eq!(update.kind(), UpdateKind::RealTime);
/// ```
///
/// # What panics
///
/// | Call | Panics when |
/// |---|---|
/// | [`changed`](Self::changed), [`unchanged`](Self::unchanged) | `field` is not one of the names given to [`new`](Self::new) |
/// | [`command_fields`](Self::command_fields) | either position is `0` or greater than the number of fields |
/// | [`item_index`](Self::item_index) | `position` is `0` — positions are 1-based |
///
/// Everything else is representable and means something:
///
/// - A field never given a value is **null and unchanged** — the state an
///   item's field is in before any update has set it.
/// - Setting the same field twice is allowed; the last call decides both its
///   value and whether it counts as changed.
/// - If the same field name is given twice to [`new`](Self::new), the first
///   position wins for every lookup, matching
///   [`ItemUpdate::field_by_name`].
///
/// # What it cannot build
///
/// Every item and field of the built update carries a **declared** name, so
/// [`ItemUpdate::declared_item_name`] is always `Some` and every field is
/// reachable by name. The stand-in names that a subscription to a server-side
/// group and schema produces [`docs/spec/04-notifications.md` §3.1] are not
/// reproducible here, because a test that looks a field up by name needs the
/// name.
#[derive(Debug, Clone)]
pub struct ItemUpdateBuilder {
    item_name: String,
    /// 1-based.
    item_index: usize,
    field_names: Vec<String>,
    kind: UpdateKind,
    /// 1-based, validated against `field_names` on the way in.
    command_fields: Option<(usize, usize)>,
    /// One entry per field; `None` is null.
    values: Vec<Option<String>>,
    /// One flag per field: whether this update carried a token for it.
    changed: Vec<bool>,
}

impl ItemUpdateBuilder {
    /// A real-time update for `item_name`, over a schema of `field_names` in
    /// wire order.
    ///
    /// Both are the names *you* declared when you subscribed: TLCP transmits
    /// counts, never names [`docs/spec/04-notifications.md` §3.1], so these
    /// are what [`ItemUpdate::item_name`] and [`ItemUpdate::field_name`] will
    /// report. Every field starts [`FieldValue::Null`] and unchanged.
    #[must_use]
    pub fn new(
        item_name: impl Into<String>,
        field_names: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        let field_names: Vec<String> = field_names.into_iter().map(Into::into).collect();
        let field_count = field_names.len();
        Self {
            item_name: item_name.into(),
            item_index: 1,
            field_names,
            kind: UpdateKind::RealTime,
            command_fields: None,
            values: vec![None; field_count],
            changed: vec![false; field_count],
        }
    }

    /// Whether the update is snapshot content or a live change. Defaults to
    /// [`UpdateKind::RealTime`].
    #[must_use]
    pub const fn kind(mut self, kind: UpdateKind) -> Self {
        self.kind = kind;
        self
    }

    /// The item's **1-based** position within the subscription. Defaults to
    /// `1`.
    ///
    /// Set this when the code under test dispatches on
    /// [`ItemUpdate::item_index`] rather than on the item's name. The built
    /// schema grows to `position` items, of which only this one is named.
    ///
    /// # Panics
    ///
    /// If `position` is `0`: items are numbered from 1
    /// [`docs/spec/04-notifications.md` §2.1].
    #[must_use]
    pub fn item_index(mut self, position: usize) -> Self {
        assert!(
            position > 0,
            "item positions are 1-based, so `0` is not one"
        );
        self.item_index = position;
        self
    }

    /// Declares the subscription to be in `COMMAND` mode, with the key at
    /// field position `key` and the command at field position `command`, both
    /// **1-based** [`docs/spec/04-notifications.md` §3.2].
    ///
    /// Without this call [`ItemUpdate::key`] and [`ItemUpdate::command`]
    /// return [`None`] and [`ItemUpdate::is_command_mode`] is `false`, which
    /// is the right shape for a `MERGE`, `DISTINCT` or `RAW` subscription.
    ///
    /// # Panics
    ///
    /// If either position is `0` or names a field the schema does not have —
    /// `SUBCMD` positions must name a field of the schema it declares.
    #[must_use]
    pub fn command_fields(mut self, key: usize, command: usize) -> Self {
        self.check_field_position("key", key);
        self.check_field_position("command", command);
        self.command_fields = Some((key, command));
        self
    }

    /// Sets `field` to `value` **and** marks it as changed by this update.
    ///
    /// "Changed" means the update carried an explicit token for the field, not
    /// that the value differs from the previous one
    /// [`docs/spec/04-notifications.md` §2.5] — so use this even when the
    /// server resent a value the field already held.
    ///
    /// # Panics
    ///
    /// If `field` is not one of the names given to [`new`](Self::new).
    #[must_use]
    pub fn changed(self, field: &str, value: FieldValue<'_>) -> Self {
        self.set(field, value, true)
    }

    /// Sets `field` to `value` **without** marking it changed: the value a
    /// previous update left behind, which this one did not carry a token for
    /// [`docs/spec/04-notifications.md` §2.2].
    ///
    /// # Panics
    ///
    /// If `field` is not one of the names given to [`new`](Self::new).
    #[must_use]
    pub fn unchanged(self, field: &str, value: FieldValue<'_>) -> Self {
        self.set(field, value, false)
    }

    /// The finished update.
    #[must_use]
    pub fn build(self) -> ItemUpdate {
        // The named item sits last, at `item_index`; the positions before it
        // exist only so the index lines up, and go unnamed.
        let mut items = vec![None; self.item_index];
        if let Some(slot) = items.last_mut() {
            *slot = Some(self.item_name);
        }

        let schema = SubscriptionSchema::from_optional_names(
            items,
            self.field_names.into_iter().map(Some).collect(),
            // Validated 1-based by `command_fields`, and the schema wants them
            // 0-based.
            self.command_fields
                .map(|(key, command)| (key.saturating_sub(1), command.saturating_sub(1))),
        );

        ItemUpdate::new(
            Arc::new(schema),
            // Validated 1-based by `new` and `item_index`.
            self.item_index.saturating_sub(1),
            self.kind,
            self.values,
            // `ItemUpdate` wants the changed positions ascending, which
            // walking the flags in order gives for free.
            self.changed
                .iter()
                .enumerate()
                .filter_map(|(index, changed)| changed.then_some(index))
                .collect(),
        )
    }

    /// Shared body of [`changed`](Self::changed) and
    /// [`unchanged`](Self::unchanged).
    #[must_use]
    fn set(mut self, field: &str, value: FieldValue<'_>, changed: bool) -> Self {
        let index = self.field_index(field);
        // Both `get_mut`s hit: `field_index` panicked otherwise.
        if let Some(slot) = self.values.get_mut(index) {
            *slot = value.text().map(str::to_owned);
        }
        if let Some(flag) = self.changed.get_mut(index) {
            *flag = changed;
        }
        self
    }

    /// The 0-based position of the field named `field`.
    ///
    /// # Panics
    ///
    /// If the schema has no such field.
    #[must_use]
    fn field_index(&self, field: &str) -> usize {
        match self.field_names.iter().position(|name| name == field) {
            Some(index) => index,
            None => panic!(
                "`{field}` is not a field of this update; its schema is {:?}",
                self.field_names
            ),
        }
    }

    /// Checks that `position` is a 1-based position of an existing field.
    ///
    /// # Panics
    ///
    /// If it is not.
    fn check_field_position(&self, role: &str, position: usize) {
        let count = self.field_names.len();
        assert!(
            position >= 1 && position <= count,
            "the {role} field is at position {position}, but this update has \
             {count} field(s), numbered from 1"
        );
    }
}

// ---------------------------------------------------------------------------
// Session events
// ---------------------------------------------------------------------------

/// Builds a [`Connected`], the payload of [`SessionEvent::Connected`](crate::SessionEvent::Connected).
///
/// The two fields an application reasons about — the session id and the
/// [`Continuity`] — are required; the two the server negotiates are defaulted,
/// since code deciding whether its derived state survived a reconnection does
/// not usually read them.
///
/// # Example: testing that you discard state when the session was replaced
///
/// ```
/// use lightstreamer_rs::test_util::ConnectedBuilder;
/// use lightstreamer_rs::{Connected, Continuity, SessionEvent, StateValidity};
///
/// // Your crate's rule, as `lib.rs` describes it: keep what you computed only
/// // while the session it was computed from is demonstrably still there.
/// fn keep_derived_state(connected: &Connected) -> bool {
///     connected.continuity.state_validity() == StateValidity::Valid
/// }
///
/// let rebound = ConnectedBuilder::new("S1234abcd", Continuity::Preserved).build();
/// assert!(keep_derived_state(&rebound));
///
/// let replaced = ConnectedBuilder::new(
///     "S5678efgh",
///     Continuity::Replaced {
///         previous_session_id: Some("S1234abcd".to_owned()),
///     },
/// )
/// .build();
/// assert!(!keep_derived_state(&replaced));
///
/// // And the whole event, if that is what your handler takes.
/// let event = SessionEvent::Connected(Box::new(replaced));
/// assert!(matches!(event, SessionEvent::Connected(_)));
/// ```
#[derive(Debug, Clone)]
pub struct ConnectedBuilder {
    session_id: String,
    continuity: Continuity,
    keepalive: Duration,
    request_limit_bytes: u64,
}

impl ConnectedBuilder {
    /// The server's session id and what this bind means for state derived from
    /// an earlier one.
    #[must_use]
    pub fn new(session_id: impl Into<String>, continuity: Continuity) -> Self {
        Self {
            session_id: session_id.into(),
            continuity,
            keepalive: DEFAULT_TEST_KEEPALIVE,
            request_limit_bytes: DEFAULT_TEST_REQUEST_LIMIT_BYTES,
        }
    }

    /// The keepalive interval the server settled on. Defaults to five seconds,
    /// a plausible stand-in with no protocol significance.
    #[must_use]
    pub const fn keepalive(mut self, keepalive: Duration) -> Self {
        self.keepalive = keepalive;
        self
    }

    /// The largest request the server will accept, in bytes. Defaults to
    /// `50_000`, likewise a stand-in.
    #[must_use]
    pub const fn request_limit_bytes(mut self, limit: u64) -> Self {
        self.request_limit_bytes = limit;
        self
    }

    /// The finished payload.
    #[must_use]
    pub fn build(self) -> Connected {
        Connected::from_parts(
            self.session_id,
            self.continuity,
            self.keepalive,
            self.request_limit_bytes,
        )
    }
}

/// [`ConnectedBuilder`]'s default keepalive: plausible, and deliberately not
/// derived from any protocol constant.
const DEFAULT_TEST_KEEPALIVE: Duration = Duration::from_secs(5);

/// [`ConnectedBuilder`]'s default request limit, in bytes.
const DEFAULT_TEST_REQUEST_LIMIT_BYTES: u64 = 50_000;

/// Builds a [`Recovery`], the payload of [`SessionEvent::Recovered`](crate::SessionEvent::Recovered), from the
/// two progressives that define it.
///
/// The [`RecoveryOutcome`] is **derived**, exactly as the session layer derives
/// it from a `PROG` answer [`docs/spec/02-session-lifecycle.md` §5.2, §5.4], so
/// a `Recovery` whose outcome contradicts its own numbers cannot be built:
///
/// | `resumed_at` versus `requested_from` | Outcome |
/// |---|---|
/// | equal | [`RecoveryOutcome::Exact`] |
/// | lower — the server resumed earlier and re-sent | [`RecoveryOutcome::Duplicated`] with the difference as `suppressed` |
/// | higher — the server resumed later and the gap is lost | [`RecoveryOutcome::Gap`] with the difference as `missing` |
///
/// # Example: testing that you rebuild only on a real gap
///
/// ```
/// use lightstreamer_rs::test_util::recovery;
/// use lightstreamer_rs::RecoveryOutcome;
///
/// // The server re-sent 3 notifications this client suppressed: nothing lost.
/// let duplicated = recovery(100, 97);
/// assert_eq!(
///     duplicated.outcome,
///     RecoveryOutcome::Duplicated { suppressed: 3 }
/// );
/// assert!(duplicated.is_lossless());
///
/// // The server resumed 2 past the requested point: those are gone.
/// let gap = recovery(100, 102);
/// assert_eq!(gap.outcome, RecoveryOutcome::Gap { missing: 2 });
/// assert!(!gap.is_lossless());
///
/// assert!(recovery(100, 100).is_lossless());
/// ```
#[must_use]
pub fn recovery(requested_from: u64, resumed_at: u64) -> Recovery {
    // The same three cases the session layer distinguishes when a `PROG`
    // answers a recovery request; see the `PROG` handling in
    // `src/session/mod.rs`.
    let outcome = match resumed_at.cmp(&requested_from) {
        Ordering::Equal => RecoveryOutcome::Exact,
        Ordering::Less => match requested_from.checked_sub(resumed_at) {
            Some(suppressed) => RecoveryOutcome::Duplicated { suppressed },
            // Unreachable: the ordering was established by the match.
            None => RecoveryOutcome::Exact,
        },
        Ordering::Greater => match resumed_at.checked_sub(requested_from) {
            Some(missing) => RecoveryOutcome::Gap { missing },
            // Unreachable: the ordering was established by the match.
            None => RecoveryOutcome::Exact,
        },
    };
    Recovery::from_parts(requested_from, resumed_at, outcome)
}

/// Builds a [`Resubscribed`], one entry of the [`SessionEvent::Resubscribed`](crate::SessionEvent::Resubscribed)
/// list.
///
/// Both flags default to `false`, so each one is set by the name that says
/// what it means rather than by its position in an argument list.
///
/// # Example
///
/// ```
/// use lightstreamer_rs::test_util::{ResubscribedBuilder, subscription_id};
/// use lightstreamer_rs::{Resubscribed, SessionEvent};
///
/// // Your crate's rule: a subscription that does not restart from a snapshot
/// // has to be rebuilt tick by tick, so warn about it.
/// fn needs_manual_rebuild(entry: &Resubscribed) -> bool {
///     entry.was_active && !entry.snapshot_restarting
/// }
///
/// let entry = ResubscribedBuilder::new(subscription_id(7))
///     .was_active(true)
///     .build();
/// assert!(needs_manual_rebuild(&entry));
/// assert_eq!(entry.id.get(), 7);
///
/// let event = SessionEvent::Resubscribed(vec![entry]);
/// assert!(matches!(event, SessionEvent::Resubscribed(_)));
/// ```
#[derive(Debug, Clone, Copy)]
pub struct ResubscribedBuilder {
    id: SubscriptionId,
    was_active: bool,
    snapshot_restarting: bool,
}

impl ResubscribedBuilder {
    /// Which subscription was re-created. Get an id from
    /// [`subscription_id`], or from [`Updates::id`](crate::Updates::id) if the
    /// test has a real one.
    #[must_use]
    pub const fn new(id: SubscriptionId) -> Self {
        Self {
            id,
            was_active: false,
            snapshot_restarting: false,
        }
    }

    /// Whether the server had confirmed the subscription before the session
    /// was lost. Defaults to `false`.
    #[must_use]
    pub const fn was_active(mut self, was_active: bool) -> Self {
        self.was_active = was_active;
        self
    }

    /// Whether the subscription asked for a snapshot, and so restarts from a
    /// complete picture. Defaults to `false`.
    #[must_use]
    pub const fn snapshot_restarting(mut self, snapshot_restarting: bool) -> Self {
        self.snapshot_restarting = snapshot_restarting;
        self
    }

    /// The finished entry.
    #[must_use]
    pub const fn build(self) -> Resubscribed {
        Resubscribed::from_parts(self.id, self.was_active, self.snapshot_restarting)
    }
}

/// A [`SubscriptionId`] with a chosen value.
///
/// Real ids come from [`Updates::id`](crate::Updates::id) and are opaque; this
/// exists so a test can name one, and so that
/// [`ResubscribedBuilder`] has something to point at. The number has no
/// meaning beyond identity — it is **not** the protocol's `LS_subId` — so any
/// value will do as long as a test that compares two ids uses two different
/// ones.
#[must_use]
pub const fn subscription_id(id: u64) -> SubscriptionId {
    SubscriptionId::from_raw(id)
}

// ---------------------------------------------------------------------------
// Messages
// ---------------------------------------------------------------------------

/// Builds a [`MessageOutcome`], the payload of [`SessionEvent::Message`](crate::SessionEvent::Message).
///
/// The fate of the message is required; the sequence and the progressive that
/// identify it default to [`None`], which is what a fire-and-forget send
/// reports.
///
/// # Example: testing your own handling of a refused message
///
/// ```
/// use lightstreamer_rs::test_util::MessageOutcomeBuilder;
/// use lightstreamer_rs::{MessageOutcome, MessageResult, ServerError};
///
/// // Your crate's rule: a send that never left is worth retrying, a refusal
/// // is not.
/// fn should_retry(outcome: &MessageOutcome) -> bool {
///     matches!(outcome.result, MessageResult::NotSent { .. })
/// }
///
/// let refused = MessageOutcomeBuilder::new(MessageResult::Refused(ServerError::new(
///     33,
///     "progressive already used",
/// )))
/// .sequence("ORDERS")
/// .progressive(4)
/// .build();
/// assert!(!should_retry(&refused));
/// assert_eq!(refused.sequence.as_deref(), Some("ORDERS"));
///
/// let stranded = MessageOutcomeBuilder::new(MessageResult::NotSent {
///     reason: "the connection was gone".to_owned(),
/// })
/// .build();
/// assert!(should_retry(&stranded));
/// assert_eq!(stranded.progressive, None);
/// assert!(!stranded.is_delivered());
/// ```
#[derive(Debug, Clone)]
pub struct MessageOutcomeBuilder {
    sequence: Option<String>,
    progressive: Option<u64>,
    result: MessageResult,
}

impl MessageOutcomeBuilder {
    /// What became of the message.
    #[must_use]
    pub const fn new(result: MessageResult) -> Self {
        Self {
            sequence: None,
            progressive: None,
            result,
        }
    }

    /// The sequence the message was sent in. Defaults to none, the
    /// fire-and-forget case.
    #[must_use]
    pub fn sequence(mut self, name: impl Into<String>) -> Self {
        self.sequence = Some(name.into());
        self
    }

    /// The progressive identifying the message within its sequence. Defaults
    /// to none, which is what a fire-and-forget send reports.
    #[must_use]
    pub const fn progressive(mut self, progressive: u64) -> Self {
        self.progressive = Some(progressive);
        self
    }

    /// The finished report.
    #[must_use]
    pub fn build(self) -> MessageOutcome {
        MessageOutcome::from_parts(self.sequence, self.progressive, self.result)
    }
}

// ---------------------------------------------------------------------------
// Subscription events
// ---------------------------------------------------------------------------

/// A [`CommandFields`], the `COMMAND`-mode part of
/// [`SubscriptionEvent::Activated`](crate::SubscriptionEvent::Activated).
///
/// Both positions are **1-based**, matching the numbering everything else in
/// this crate uses [`docs/spec/04-notifications.md` §3.2]. There is no schema
/// here to check them against — that is
/// [`ItemUpdateBuilder::command_fields`]'s job — so the only mistake this can
/// catch is a `0`.
///
/// # Example
///
/// ```
/// use lightstreamer_rs::test_util::command_fields;
/// use lightstreamer_rs::SubscriptionEvent;
///
/// let event = SubscriptionEvent::Activated {
///     item_count: 1,
///     field_count: 3,
///     command_fields: Some(command_fields(1, 2)),
/// };
/// assert!(matches!(
///     event,
///     SubscriptionEvent::Activated {
///         command_fields: Some(fields),
///         ..
///     } if fields.key == 1 && fields.command == 2
/// ));
/// ```
///
/// # Panics
///
/// If either position is `0`.
#[must_use]
pub fn command_fields(key: usize, command: usize) -> CommandFields {
    assert!(
        key >= 1 && command >= 1,
        "field positions are 1-based, so `0` is not one: key {key}, command {command}"
    );
    CommandFields::from_parts(key, command)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ServerError;
    use crate::subscription::item_update::ItemCommand;

    fn quote() -> ItemUpdateBuilder {
        ItemUpdateBuilder::new("CS.D.EURUSD.CFD.IP", ["BID", "OFFER", "STATUS"])
    }

    // -- Round trip ----------------------------------------------------------

    #[test]
    fn test_built_update_answers_every_accessor() {
        let update = quote()
            .kind(UpdateKind::Snapshot)
            .changed("BID", FieldValue::Text("1.0921"))
            .unchanged("OFFER", FieldValue::Text("1.0923"))
            .build();

        assert_eq!(update.item_index(), 1);
        assert_eq!(update.item_name(), "CS.D.EURUSD.CFD.IP");
        assert_eq!(update.declared_item_name(), Some("CS.D.EURUSD.CFD.IP"));
        assert_eq!(update.kind(), UpdateKind::Snapshot);
        assert!(update.is_snapshot());
        assert_eq!(update.field_count(), 3);
        assert_eq!(update.field_name(2), Some("OFFER"));
        assert_eq!(update.field_position("STATUS"), Some(3));
        assert_eq!(update.field(1), Some(FieldValue::Text("1.0921")));
        assert_eq!(
            update.field_by_name("OFFER"),
            Some(FieldValue::Text("1.0923"))
        );
        assert_eq!(update.changed_count(), 1);
        assert!(update.is_field_changed(1));
        assert!(update.is_field_changed_by_name("BID"));
        assert!(!update.is_command_mode());
        assert_eq!(update.key(), None);
        assert_eq!(update.command(), None);

        let fields: Vec<(usize, &str, FieldValue<'_>, bool)> = update
            .fields()
            .map(|field| {
                (
                    field.position(),
                    field.name(),
                    field.value(),
                    field.is_changed(),
                )
            })
            .collect();
        assert_eq!(
            fields,
            vec![
                (1, "BID", FieldValue::Text("1.0921"), true),
                (2, "OFFER", FieldValue::Text("1.0923"), false),
                (3, "STATUS", FieldValue::Null, false),
            ]
        );
    }

    #[test]
    fn test_defaults_are_real_time_item_one_and_all_null() {
        let update = quote().build();
        assert_eq!(update.kind(), UpdateKind::RealTime);
        assert!(!update.is_snapshot());
        assert_eq!(update.item_index(), 1);
        assert_eq!(update.changed_count(), 0);
        for field in update.fields() {
            assert_eq!(field.value(), FieldValue::Null, "{}", field.name());
            assert!(!field.is_changed(), "{}", field.name());
        }
    }

    #[test]
    fn test_item_index_positions_the_item() {
        let update = quote().item_index(4).build();
        assert_eq!(update.item_index(), 4);
        assert_eq!(update.item_name(), "CS.D.EURUSD.CFD.IP");
        assert_eq!(update.declared_item_name(), Some("CS.D.EURUSD.CFD.IP"));
    }

    // -- Null versus empty ---------------------------------------------------

    #[test]
    fn test_null_and_empty_stay_distinct() {
        // `#` is null and `$` is the empty string
        // [`docs/spec/04-notifications.md` §2.2].
        let update = quote()
            .changed("BID", FieldValue::Null)
            .changed("OFFER", FieldValue::Text(""))
            .build();
        assert_eq!(update.field_by_name("BID"), Some(FieldValue::Null));
        assert_eq!(update.field_by_name("OFFER"), Some(FieldValue::Text("")));
        assert_ne!(update.field_by_name("BID"), update.field_by_name("OFFER"));
        assert_eq!(update.field_by_name("BID").and_then(FieldValue::text), None);
        assert_eq!(
            update.field_by_name("OFFER").and_then(FieldValue::text),
            Some("")
        );
    }

    #[test]
    fn test_absent_field_and_null_field_are_different_absences() {
        let update = quote().build();
        assert_eq!(update.field_by_name("BID"), Some(FieldValue::Null));
        assert_eq!(update.field_by_name("NO_SUCH_FIELD"), None);
    }

    // -- Changed versus unchanged -------------------------------------------

    #[test]
    fn test_changed_and_unchanged_are_reflected() {
        let update = quote()
            .unchanged("BID", FieldValue::Text("1.0920"))
            .changed("OFFER", FieldValue::Text("1.0923"))
            .changed("STATUS", FieldValue::Null)
            .build();

        let changed: Vec<(usize, &str)> = update
            .changed_fields()
            .map(|field| (field.position(), field.name()))
            .collect();
        assert_eq!(changed, vec![(2, "OFFER"), (3, "STATUS")]);
        assert_eq!(update.changed_count(), 2);
        assert!(!update.is_field_changed(1));
        assert!(update.is_field_changed(2));
        assert!(update.is_field_changed(3));
        assert!(!update.is_field_changed_by_name("BID"));
        assert!(update.is_field_changed_by_name("OFFER"));
    }

    #[test]
    fn test_changed_fields_come_out_ascending_whatever_the_call_order() {
        let update = quote()
            .changed("STATUS", FieldValue::Text("TRADEABLE"))
            .changed("BID", FieldValue::Text("1.0921"))
            .build();
        let positions: Vec<usize> = update
            .changed_fields()
            .map(|field| field.position())
            .collect();
        assert_eq!(positions, vec![1, 3]);
    }

    #[test]
    fn test_last_call_for_a_field_wins() {
        let update = quote()
            .changed("BID", FieldValue::Text("1.0921"))
            .unchanged("BID", FieldValue::Null)
            .build();
        assert_eq!(update.field_by_name("BID"), Some(FieldValue::Null));
        assert!(!update.is_field_changed_by_name("BID"));
        assert_eq!(update.changed_count(), 0);
    }

    #[test]
    fn test_duplicate_field_names_resolve_to_the_first() {
        let update = ItemUpdateBuilder::new("item1", ["dup", "dup"])
            .changed("dup", FieldValue::Text("first"))
            .build();
        assert_eq!(update.field_position("dup"), Some(1));
        assert_eq!(update.field(1), Some(FieldValue::Text("first")));
        assert_eq!(update.field(2), Some(FieldValue::Null));
    }

    // -- COMMAND mode --------------------------------------------------------

    #[test]
    fn test_command_fields_make_key_and_command_answer() {
        // `SUBCMD` positions are 1-based [`docs/spec/04-notifications.md` §3.2].
        let update = ItemUpdateBuilder::new("PORTFOLIO", ["key", "command", "qty"])
            .command_fields(1, 2)
            .changed("key", FieldValue::Text("AAPL"))
            .changed("command", FieldValue::Text("ADD"))
            .changed("qty", FieldValue::Text("10"))
            .build();
        assert!(update.is_command_mode());
        assert_eq!(update.key(), Some(FieldValue::Text("AAPL")));
        assert_eq!(update.command(), Some(ItemCommand::Add));
    }

    #[test]
    fn test_without_command_fields_key_and_command_are_none() {
        let update = quote().changed("BID", FieldValue::Text("1.0921")).build();
        assert!(!update.is_command_mode());
        assert_eq!(update.key(), None);
        assert_eq!(update.command(), None);
    }

    #[test]
    fn test_a_null_command_field_names_no_operation() {
        let update = ItemUpdateBuilder::new("PORTFOLIO", ["key", "command"])
            .command_fields(1, 2)
            .changed("key", FieldValue::Text("AAPL"))
            .changed("command", FieldValue::Null)
            .build();
        assert!(update.is_command_mode());
        assert_eq!(update.key(), Some(FieldValue::Text("AAPL")));
        assert_eq!(update.command(), None);
    }

    // -- Misuse --------------------------------------------------------------

    #[test]
    #[should_panic(expected = "is not a field of this update")]
    fn test_setting_an_unknown_field_panics() {
        let _ = quote().changed("BIDD", FieldValue::Text("1.0921"));
    }

    #[test]
    #[should_panic(expected = "but this update has 3 field(s)")]
    fn test_command_position_past_the_schema_panics() {
        let _ = quote().command_fields(1, 4);
    }

    #[test]
    #[should_panic(expected = "numbered from 1")]
    fn test_command_position_zero_panics() {
        let _ = quote().command_fields(0, 1);
    }

    #[test]
    #[should_panic(expected = "item positions are 1-based")]
    fn test_item_index_zero_panics() {
        let _ = quote().item_index(0);
    }

    // -- Connected -----------------------------------------------------------

    #[test]
    fn test_connected_carries_the_continuity_it_was_given() {
        let replaced = ConnectedBuilder::new(
            "S5678efgh",
            Continuity::Replaced {
                previous_session_id: Some("S1234abcd".to_owned()),
            },
        )
        .build();
        assert_eq!(replaced.session_id, "S5678efgh");
        assert_eq!(
            replaced.continuity.state_validity(),
            crate::StateValidity::Invalid
        );
        assert_eq!(
            replaced.continuity,
            Continuity::Replaced {
                previous_session_id: Some("S1234abcd".to_owned()),
            }
        );

        let recovered =
            ConnectedBuilder::new("S1234abcd", Continuity::Recovered { requested_from: 42 })
                .build();
        assert_eq!(
            recovered.continuity.state_validity(),
            crate::StateValidity::Pending
        );
    }

    #[test]
    fn test_connected_negotiated_values_default_and_override() {
        let defaulted = ConnectedBuilder::new("S1", Continuity::New).build();
        assert_eq!(defaulted.keepalive, DEFAULT_TEST_KEEPALIVE);
        assert_eq!(
            defaulted.request_limit_bytes,
            DEFAULT_TEST_REQUEST_LIMIT_BYTES
        );

        let chosen = ConnectedBuilder::new("S1", Continuity::New)
            .keepalive(Duration::from_millis(1500))
            .request_limit_bytes(4096)
            .build();
        assert_eq!(chosen.keepalive, Duration::from_millis(1500));
        assert_eq!(chosen.request_limit_bytes, 4096);
    }

    // -- Recovery ------------------------------------------------------------

    #[test]
    fn test_recovery_derives_its_outcome_from_the_progressives() {
        // The three cases the session layer distinguishes when a `PROG`
        // answers [`docs/spec/02-session-lifecycle.md` §5.2, §5.4].
        let exact = recovery(100, 100);
        assert_eq!(exact.outcome, RecoveryOutcome::Exact);
        assert!(exact.is_lossless());

        let duplicated = recovery(100, 97);
        assert_eq!(
            duplicated.outcome,
            RecoveryOutcome::Duplicated { suppressed: 3 }
        );
        assert!(duplicated.is_lossless());

        let gap = recovery(100, 102);
        assert_eq!(gap.outcome, RecoveryOutcome::Gap { missing: 2 });
        assert!(!gap.is_lossless());
    }

    #[test]
    fn test_recovery_keeps_both_progressives() {
        let outcome = recovery(7, 4);
        assert_eq!(outcome.requested_from, 7);
        assert_eq!(outcome.resumed_at, 4);
    }

    // -- Resubscribed and subscription ids -----------------------------------

    #[test]
    fn test_resubscribed_flags_default_to_false_and_set_by_name() {
        let defaulted = ResubscribedBuilder::new(subscription_id(1)).build();
        assert!(!defaulted.was_active);
        assert!(!defaulted.snapshot_restarting);

        let set = ResubscribedBuilder::new(subscription_id(2))
            .was_active(true)
            .snapshot_restarting(true)
            .build();
        assert_eq!(set.id, subscription_id(2));
        assert!(set.was_active);
        assert!(set.snapshot_restarting);
    }

    #[test]
    fn test_subscription_ids_round_trip_and_compare() {
        assert_eq!(subscription_id(7).get(), 7);
        assert_eq!(subscription_id(7), subscription_id(7));
        assert_ne!(subscription_id(7), subscription_id(8));
        // The rendering names the owning client too, because the number alone
        // is only unique within one of them.
        assert!(
            subscription_id(7).to_string().ends_with("subscription#7"),
            "{}",
            subscription_id(7)
        );
    }

    // -- MessageOutcome ------------------------------------------------------

    #[test]
    fn test_message_outcome_defaults_to_a_fire_and_forget_shape() {
        let outcome = MessageOutcomeBuilder::new(MessageResult::NotSent {
            reason: "the connection was gone".to_owned(),
        })
        .build();
        assert_eq!(outcome.sequence, None);
        assert_eq!(outcome.progressive, None);
        assert!(!outcome.is_delivered());
        assert!(matches!(outcome.result, MessageResult::NotSent { .. }));
    }

    #[test]
    fn test_message_outcome_carries_its_sequence_and_progressive() {
        let delivered = MessageOutcomeBuilder::new(MessageResult::Delivered {
            response: "ok".to_owned(),
        })
        .sequence("ORDERS")
        .progressive(4)
        .build();
        assert_eq!(delivered.sequence.as_deref(), Some("ORDERS"));
        assert_eq!(delivered.progressive, Some(4));
        assert!(delivered.is_delivered());
    }

    #[test]
    fn test_message_outcome_preserves_the_server_code() {
        let refused = MessageOutcomeBuilder::new(MessageResult::Refused(ServerError::new(
            33,
            "progressive already used",
        )))
        .build();
        assert!(!refused.is_delivered());
        match refused.result {
            MessageResult::Refused(error) => {
                assert_eq!(error.code(), 33);
                assert_eq!(error.message(), "progressive already used");
            }
            other => panic!("expected a refusal, got {other:?}"),
        }
    }

    // -- CommandFields -------------------------------------------------------

    #[test]
    fn test_command_fields_are_one_based() {
        let fields = command_fields(1, 2);
        assert_eq!(fields.key, 1);
        assert_eq!(fields.command, 2);
    }

    #[test]
    #[should_panic(expected = "field positions are 1-based")]
    fn test_command_fields_reject_zero() {
        let _ = command_fields(0, 1);
    }
}
