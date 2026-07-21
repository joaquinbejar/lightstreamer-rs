//! Building an [`ItemUpdate`] by hand, for the tests of crates that depend on
//! this one. Enabled by the off-by-default `test-util` feature.
//!
//! A crate built on this one usually has its own parsing layer: it reads
//! [`FieldValue`]s out of an [`ItemUpdate`] and turns them into whatever its
//! domain calls them. That layer deserves unit tests against captured
//! payloads, and those tests want nothing to do with a socket, a session or a
//! server. Without this module they cannot have one — an [`ItemUpdate`] is
//! assembled from a subscription's schema, and both the constructor and the
//! schema are private, on purpose, because nothing outside this crate should
//! be able to forge an update that a real session produced.
//!
//! [`ItemUpdateBuilder`] is the deliberate exception, kept behind a feature
//! gate so that it exists only where it belongs:
//!
//! ```toml
//! [dev-dependencies]
//! lightstreamer-rs = { version = "1", features = ["test-util"] }
//! ```
//!
//! Cargo enables the feature only when it builds your tests, so your release
//! build never contains it.
//!
//! # Null is still not empty
//!
//! The builder takes a [`FieldValue`], not an `Option<&str>` and not a `&str`,
//! for exactly the reason the rest of this crate does: `#` and `$` are
//! different values [`docs/spec/04-notifications.md` §2.2], and the single
//! most valuable thing a consumer can test is that its own parser tells them
//! apart. Write [`FieldValue::Null`] for a field the server had no value for
//! and [`FieldValue::Text`]`("")` for one it deliberately blanked.
//!
//! # Misuse panics
//!
//! This is a builder for tests, so it treats a nonsensical update as a test
//! author's mistake and fails loudly rather than quietly producing something
//! that cannot come off a wire. See [`ItemUpdateBuilder`] for the complete
//! list of what panics.

use std::sync::Arc;

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
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
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
}
