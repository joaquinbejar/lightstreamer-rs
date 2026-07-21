//! What the caller receives for every real-time update: [`ItemUpdate`].
//!
//! A TLCP subscription is a table. Its **items** are the rows the server was
//! asked for and its **fields** are the columns of the schema; both are ordered
//! by the Metadata Adapter and both are numbered from 1 on the wire
//! [`docs/spec/04-notifications.md` §2.1]. Every `U` notification reports new
//! values for one item, and [`ItemUpdate`] is that notification after the
//! second-level value syntax has been decoded against the item's previous
//! state [`docs/spec/04-notifications.md` §2.3].
//!
//! # Null is not empty
//!
//! This is the single most important thing to get right about a field value.
//! TLCP has two distinct markers and they mean different things
//! [`docs/spec/04-notifications.md` §2.2]:
//!
//! | Wire token | Meaning | This crate |
//! |---|---|---|
//! | `#` | the field is **null** — it has no value | [`FieldValue::Null`] |
//! | `$` | the field is the **empty string** | [`FieldValue::Text`] wrapping `""` |
//! | *(empty token)* | the field is **unchanged** since the last update | the previous [`FieldValue`], and the field is absent from [`ItemUpdate::changed_fields`] |
//!
//! A field value is therefore [`FieldValue`], never a bare `&str` and never an
//! `Option<&str>` that a caller might flatten by accident. `FieldValue::Null`
//! and `FieldValue::Text("")` compare unequal, print differently, and are
//! produced by different server intentions. Code that treats a missing quote as
//! an empty quote is wrong in a way no test of its own will catch, so this
//! crate refuses to make the two spellable as one.
//!
//! # Values are text
//!
//! A field value is exactly the UTF-8 string the server sent, after
//! percent-decoding and after any diff was applied
//! [`docs/spec/04-notifications.md` §2.2, §2.3]. This crate never parses one
//! into a number, a date or a boolean: what a value means is the Metadata
//! Adapter's business and the caller's, not the protocol's.
//!
//! # Snapshot or real time
//!
//! [`ItemUpdate::kind`] answers it, computed from the specification's own
//! decision table [`docs/spec/04-notifications.md` §2.4]; the table is
//! implemented row for row by `classify_update` in
//! `src/subscription/manager.rs`.

use std::fmt;
use std::ops::Range;
use std::sync::Arc;

// ---------------------------------------------------------------------------
// Field values
// ---------------------------------------------------------------------------

/// The value of one field of one item.
///
/// TLCP distinguishes **null** from the **empty string**, and so does this
/// type: `#` on the wire produces [`FieldValue::Null`] and `$` produces
/// [`FieldValue::Text`] wrapping `""` [`docs/spec/04-notifications.md` §2.2].
/// They are not interchangeable. A quote adapter that has no bid to report
/// sends `#`; one whose status line is deliberately blank sends `$`.
///
/// # Examples
///
/// ```ignore
/// match update.field_by_name("bid") {
///     Some(FieldValue::Text(bid)) => println!("bid is {bid}"),
///     Some(FieldValue::Null) => println!("there is no bid"),
///     None => println!("this subscription has no `bid` field"),
/// }
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FieldValue<'a> {
    /// The field has no value — the `#` marker
    /// [`docs/spec/04-notifications.md` §2.2].
    ///
    /// Also what a field reads as before any update has ever set it, which is
    /// only observable between the subscription being activated and its first
    /// `U`: the first update of a subscription carries a token for every field
    /// [`docs/spec/04-notifications.md` §2.3].
    Null,

    /// The field holds this text, which may be the **empty string** — the `$`
    /// marker sets it to exactly that
    /// [`docs/spec/04-notifications.md` §2.2].
    Text(&'a str),
}

impl<'a> FieldValue<'a> {
    /// Whether the field is null, i.e. has no value at all.
    ///
    /// A field holding the empty string is **not** null.
    #[must_use]
    #[inline]
    pub const fn is_null(self) -> bool {
        matches!(self, Self::Null)
    }

    /// Whether the field holds the empty string.
    ///
    /// A null field is **not** empty text: it has no text.
    #[must_use]
    #[inline]
    pub fn is_empty_text(self) -> bool {
        matches!(self, Self::Text(text) if text.is_empty())
    }

    /// The text, or [`None`] if the field is null.
    ///
    /// Collapsing the distinction is a deliberate act here, which is the point:
    /// `text()` reads as "I am choosing to treat null as absent", whereas an
    /// accessor that returned `&str` directly would make the same choice
    /// silently.
    #[must_use]
    #[inline]
    pub const fn text(self) -> Option<&'a str> {
        match self {
            Self::Null => None,
            Self::Text(text) => Some(text),
        }
    }

    /// The text, substituting `default` if the field is null.
    #[must_use]
    #[inline]
    pub fn text_or(self, default: &'a str) -> &'a str {
        self.text().unwrap_or(default)
    }
}

impl fmt::Display for FieldValue<'_> {
    /// Renders null as an empty string.
    ///
    /// This is a display convenience and is intentionally lossy — it is the one
    /// place the null/empty distinction is collapsed, because a terminal has no
    /// way to show it. Match on the variant when the difference matters.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Null => Ok(()),
            Self::Text(text) => f.write_str(text),
        }
    }
}

// ---------------------------------------------------------------------------
// COMMAND-mode command field
// ---------------------------------------------------------------------------

/// What a `COMMAND`-mode update did to the row named by its key.
///
/// In `COMMAND` mode the subscription is a dynamic table: "each real-time
/// update may add a new row, delete an existing row or change the content of a
/// specific row" [`docs/spec/04-notifications.md` §3.3]. Which of the three
/// happened is carried in the field whose position `SUBCMD` reported
/// [`docs/spec/04-notifications.md` §3.2], and the row it applies to is named
/// by the value of the key field.
///
/// SPEC-AMBIGUITY (command-literals): the specification names the commands
/// `ADD`, `DELETE` and `UPDATE` but "nowhere states the literal wire values
/// carried in the command field of a `U` notification, nor their casing"
/// [`docs/spec/04-notifications.md` §3.3]. This crate matches the three names
/// **case-insensitively** — the most permissive reading that cannot admit a
/// fourth command by accident — and preserves anything else as
/// [`ItemCommand::Unrecognized`] rather than rejecting the update, so a server
/// that grows a command cannot break a client that does not know it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ItemCommand {
    /// A row entered the table. Its fields are the ones this update carries.
    Add,

    /// An existing row changed. The fields this update did not carry keep the
    /// values they already had.
    Update,

    /// A row left the table. The caller should drop its own representation of
    /// the row named by [`ItemUpdate::key`].
    ///
    /// Note what this does **not** mean for the state this crate keeps: see
    /// the `DELETE` discussion on [`ItemUpdate::key`].
    Delete,

    /// The command field held something else. The value is preserved verbatim.
    Unrecognized(
        /// The command exactly as the server sent it, after decoding.
        String,
    ),
}

impl ItemCommand {
    /// Classifies a decoded command-field value.
    #[must_use]
    fn from_wire(value: &str) -> Self {
        // SPEC-AMBIGUITY (command-literals): casing is unspecified
        // [`docs/spec/04-notifications.md` §3.3], so the comparison ignores it.
        if value.eq_ignore_ascii_case("ADD") {
            Self::Add
        } else if value.eq_ignore_ascii_case("UPDATE") {
            Self::Update
        } else if value.eq_ignore_ascii_case("DELETE") {
            Self::Delete
        } else {
            Self::Unrecognized(value.to_owned())
        }
    }

    /// The command as an uppercase name, or the preserved literal for
    /// [`ItemCommand::Unrecognized`].
    #[must_use]
    #[inline]
    pub fn as_str(&self) -> &str {
        match self {
            Self::Add => "ADD",
            Self::Update => "UPDATE",
            Self::Delete => "DELETE",
            Self::Unrecognized(literal) => literal,
        }
    }
}

impl fmt::Display for ItemCommand {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

// ---------------------------------------------------------------------------
// Snapshot classification
// ---------------------------------------------------------------------------

/// Whether an update carries part of an item's initial state or a live change.
///
/// `U` carries both: "the notification is also used to send the item's
/// snapshot" [`docs/spec/04-notifications.md` §2.1], and telling them apart is
/// a function of four inputs, tabulated in
/// [`docs/spec/04-notifications.md` §2.4].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UpdateKind {
    /// Part of the item's **initial state**, delivered because the
    /// subscription asked for a snapshot.
    ///
    /// A snapshot may be one `U` (`MERGE`) or many (`DISTINCT`, `COMMAND`),
    /// and in the latter case its end is announced by an end-of-snapshot event
    /// [`docs/spec/04-notifications.md` §3.5]. `RAW` has no snapshot at all.
    Snapshot,

    /// A **live change** to the item, produced after its initial state.
    RealTime,
}

impl UpdateKind {
    /// Whether this is snapshot content.
    #[must_use]
    #[inline]
    pub const fn is_snapshot(self) -> bool {
        matches!(self, Self::Snapshot)
    }
}

// ---------------------------------------------------------------------------
// Schema
// ---------------------------------------------------------------------------

/// One item or field name, remembering whether the caller declared it.
///
/// The protocol never transmits item or field names: `SUBOK` reports only how
/// many of each there are, and their order "is determined by the Metadata
/// Adapter" [`docs/spec/04-notifications.md` §3.1]. Names therefore come from
/// the caller, who knows them whenever it spelled out an item list and a field
/// list rather than naming a server-side group and schema. When a name was not
/// declared, this holds the position rendered in decimal so that
/// [`ItemUpdate::item_name`] can always return something printable.
#[derive(Debug, Clone, PartialEq, Eq)]
struct SchemaName {
    text: String,
    declared: bool,
}

impl SchemaName {
    /// A name the caller supplied.
    #[must_use]
    fn declared(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            declared: true,
        }
    }

    /// A stand-in for a name the caller did not supply: the 1-based position.
    #[must_use]
    fn positional(position: usize) -> Self {
        Self {
            text: position.to_string(),
            declared: false,
        }
    }

    /// The printable name.
    #[must_use]
    #[inline]
    fn as_str(&self) -> &str {
        &self.text
    }

    /// The name, only if the caller declared it.
    #[must_use]
    #[inline]
    fn declared_str(&self) -> Option<&str> {
        if self.declared {
            Some(&self.text)
        } else {
            None
        }
    }
}

/// The shape of one activated subscription: how many items and fields it has,
/// what they are called, and — in `COMMAND` mode — where the key and command
/// fields sit.
///
/// Shared by [`Arc`] across every [`ItemUpdate`] of the subscription, because
/// it is fixed from the moment `SUBOK`/`SUBCMD` arrives
/// [`docs/spec/04-notifications.md` §3.1, §3.2] and copying it per update would
/// be pure waste.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SubscriptionSchema {
    items: Vec<SchemaName>,
    fields: Vec<SchemaName>,
    key_field: Option<usize>,
    command_field: Option<usize>,
}

impl SubscriptionSchema {
    /// Builds the schema announced by `SUBOK` or `SUBCMD`.
    ///
    /// `declared_items` and `declared_fields` are the caller's own names, taken
    /// positionally. They may be shorter than the server's counts (or empty, if
    /// the caller subscribed to a server-side group and schema); every position
    /// they do not cover falls back to its 1-based index
    /// [`docs/spec/04-notifications.md` §3.1].
    ///
    /// `command_fields` carries the 0-based key and command positions of a
    /// `COMMAND`-mode subscription [`docs/spec/04-notifications.md` §3.2].
    #[must_use]
    pub(crate) fn new(
        item_count: usize,
        field_count: usize,
        declared_items: &[String],
        declared_fields: &[String],
        command_fields: Option<(usize, usize)>,
    ) -> Self {
        let (key_field, command_field) = match command_fields {
            Some((key, command)) => (Some(key), Some(command)),
            None => (None, None),
        };
        Self {
            items: Self::name_positions(item_count, declared_items),
            fields: Self::name_positions(field_count, declared_fields),
            key_field,
            command_field,
        }
    }

    /// Builds a schema from a name per position, [`None`] meaning the caller
    /// declared none for that position.
    ///
    /// [`new`](Self::new) can only take declared names as a *prefix*, which is
    /// what a subscription's item and field lists actually are. The
    /// `test-util` builder ([`crate::test_util`]) needs to name one item at an
    /// arbitrary position, so it supplies the names slot by slot instead.
    #[cfg(feature = "test-util")]
    #[must_use]
    pub(crate) fn from_optional_names(
        items: Vec<Option<String>>,
        fields: Vec<Option<String>>,
        command_fields: Option<(usize, usize)>,
    ) -> Self {
        /// Maps each slot to a declared name or to its 1-based position.
        fn named(slots: Vec<Option<String>>) -> Vec<SchemaName> {
            slots
                .into_iter()
                .enumerate()
                .map(|(index, name)| match name {
                    Some(text) => SchemaName::declared(text),
                    // Cannot overflow: `index` is an index into a `Vec`.
                    None => SchemaName::positional(index + 1),
                })
                .collect()
        }

        let (key_field, command_field) = match command_fields {
            Some((key, command)) => (Some(key), Some(command)),
            None => (None, None),
        };
        Self {
            items: named(items),
            fields: named(fields),
            key_field,
            command_field,
        }
    }

    /// Zips `count` positions against the names the caller declared.
    #[must_use]
    fn name_positions(count: usize, declared: &[String]) -> Vec<SchemaName> {
        let mut names = Vec::with_capacity(count);
        for index in 0..count {
            match declared.get(index) {
                Some(name) => names.push(SchemaName::declared(name.clone())),
                // Cannot overflow: `index < count <= usize::MAX`.
                None => names.push(SchemaName::positional(index + 1)),
            }
        }
        names
    }

    /// How many items the subscription resolved to.
    #[must_use]
    #[inline]
    pub(crate) fn item_count(&self) -> usize {
        self.items.len()
    }

    /// How many fields the schema resolved to — the width every `U` value list
    /// is decoded against [`docs/spec/04-notifications.md` §3.1].
    #[must_use]
    #[inline]
    pub(crate) fn field_count(&self) -> usize {
        self.fields.len()
    }

    /// Whether this subscription carries a key and a command field, i.e. was
    /// activated by `SUBCMD` [`docs/spec/04-notifications.md` §3.2].
    #[must_use]
    #[inline]
    pub(crate) fn is_command_mode(&self) -> bool {
        self.key_field.is_some() && self.command_field.is_some()
    }
}

// ---------------------------------------------------------------------------
// ItemUpdate
// ---------------------------------------------------------------------------

/// One real-time update, decoded: the new state of one item of one
/// subscription, plus which of its fields this update actually changed.
///
/// This is what a caller consumes from the subscription's event stream
/// (`docs/adr/0003-typed-event-stream-as-delivery-surface.md`):
///
/// ```ignore
/// while let Some(update) = updates.next().await {
///     println!("{}: {:?}", update.item_name(), update.changed_fields());
/// }
/// ```
///
/// # Every field is readable, not just the changed ones
///
/// An update on the wire is a delta: a field the server did not mention is
/// "unchanged compared to the previous update of the same field"
/// [`docs/spec/04-notifications.md` §2.2]. This type carries the item's
/// **complete** state after the delta was applied, so
/// [`field`](Self::field) and [`field_by_name`](Self::field_by_name) always
/// answer, whether or not this particular update touched the field. Use
/// [`changed_fields`](Self::changed_fields) or
/// [`is_field_changed`](Self::is_field_changed) to ask what moved.
///
/// # Positions are 1-based
///
/// Item and field positions match the specification's own numbering — items
/// and fields are numbered from 1 [`docs/spec/04-notifications.md` §2.1,
/// §3.2] — so a position printed in a log lines up with a position in a
/// packet capture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemUpdate {
    schema: Arc<SubscriptionSchema>,
    /// 0-based index of the item within the subscription.
    item: usize,
    kind: UpdateKind,
    /// The complete state of the item after this update, one entry per field
    /// of the schema. `None` is null; `Some(text)` is text, possibly empty.
    values: Vec<Option<String>>,
    /// 0-based positions of the fields this update carried a token for,
    /// ascending.
    changed: Vec<usize>,
}

impl ItemUpdate {
    /// Assembles an update from the decoded state of an item.
    ///
    /// `item` and `changed` are 0-based; `values` has one entry per field of
    /// `schema`, `None` meaning null.
    #[must_use]
    pub(crate) fn new(
        schema: Arc<SubscriptionSchema>,
        item: usize,
        kind: UpdateKind,
        values: Vec<Option<String>>,
        changed: Vec<usize>,
    ) -> Self {
        Self {
            schema,
            item,
            kind,
            values,
            changed,
        }
    }

    // -- The item -----------------------------------------------------------

    /// The item's **1-based** position within the subscription.
    ///
    /// This is the `<item>` argument of the `U` notification verbatim; the
    /// order of items "is determined by the Metadata Adapter"
    /// [`docs/spec/04-notifications.md` §2.1].
    #[must_use]
    #[inline]
    pub const fn item_index(&self) -> usize {
        // Cannot overflow: `item` is an index into a `Vec`.
        self.item + 1
    }

    /// The item's name.
    ///
    /// TLCP never sends item names — `SUBOK` reports only a count
    /// [`docs/spec/04-notifications.md` §3.1] — so this is the name the caller
    /// declared for this position when it subscribed. If the caller subscribed
    /// to a server-side group and did not declare a name for this position,
    /// this returns the item's 1-based index rendered in decimal, so that it is
    /// always printable. Use [`declared_item_name`](Self::declared_item_name)
    /// when the difference matters.
    #[must_use]
    #[inline]
    pub fn item_name(&self) -> &str {
        self.schema
            .items
            .get(self.item)
            .map_or("", SchemaName::as_str)
    }

    /// The item's name, or [`None`] if the caller never declared one.
    ///
    /// Unlike [`item_name`](Self::item_name) this never invents a stand-in.
    #[must_use]
    #[inline]
    pub fn declared_item_name(&self) -> Option<&str> {
        self.schema
            .items
            .get(self.item)
            .and_then(SchemaName::declared_str)
    }

    // -- Snapshot or real time ----------------------------------------------

    /// Whether this update is snapshot content or a live change
    /// [`docs/spec/04-notifications.md` §2.4].
    #[must_use]
    #[inline]
    pub const fn kind(&self) -> UpdateKind {
        self.kind
    }

    /// Shorthand for `self.kind().is_snapshot()`.
    #[must_use]
    #[inline]
    pub const fn is_snapshot(&self) -> bool {
        self.kind.is_snapshot()
    }

    // -- Fields --------------------------------------------------------------

    /// How many fields the subscription's schema has
    /// [`docs/spec/04-notifications.md` §3.1].
    #[must_use]
    #[inline]
    pub fn field_count(&self) -> usize {
        self.schema.field_count()
    }

    /// The name of the field at `position` (**1-based**), or [`None`] if the
    /// schema has no such field.
    ///
    /// As with [`item_name`](Self::item_name), the protocol does not transmit
    /// field names; an undeclared position renders as its 1-based index.
    #[must_use]
    #[inline]
    pub fn field_name(&self, position: usize) -> Option<&str> {
        self.schema
            .fields
            .get(position.checked_sub(1)?)
            .map(SchemaName::as_str)
    }

    /// The value of the field at `position` (**1-based**), or [`None`] if the
    /// schema has no such field.
    ///
    /// Note the two levels: [`None`] means *there is no such field*, whereas
    /// `Some(`[`FieldValue::Null`]`)` means *the field exists and is null*.
    /// See the module documentation on null versus empty.
    #[must_use]
    pub fn field(&self, position: usize) -> Option<FieldValue<'_>> {
        let index = position.checked_sub(1)?;
        Some(match self.values.get(index)? {
            None => FieldValue::Null,
            Some(text) => FieldValue::Text(text),
        })
    }

    /// The value of the field the caller declared as `name`, or [`None`] if no
    /// declared field has that name.
    ///
    /// Only names the caller declared are matched: the decimal stand-ins that
    /// [`field_name`](Self::field_name) returns for undeclared positions are
    /// **not** lookup keys, so `field_by_name("3")` cannot accidentally resolve
    /// to the third field of a subscription whose fields were never named.
    /// Names are compared exactly, and if the caller declared the same name
    /// twice the first position wins.
    #[must_use]
    pub fn field_by_name(&self, name: &str) -> Option<FieldValue<'_>> {
        self.field(self.field_position(name)?)
    }

    /// The **1-based** position of the field the caller declared as `name`, or
    /// [`None`] if no declared field has that name.
    #[must_use]
    pub fn field_position(&self, name: &str) -> Option<usize> {
        self.schema
            .fields
            .iter()
            .position(|field| field.declared_str() == Some(name))
            // Cannot overflow: `position` is an index into a `Vec`.
            .map(|index| index + 1)
    }

    /// Every field of the schema, in order, changed or not.
    #[must_use]
    #[inline]
    pub fn fields(&self) -> Fields<'_> {
        Fields {
            update: self,
            positions: 0..self.values.len(),
        }
    }

    // -- What changed --------------------------------------------------------

    /// The fields this update changed, in ascending position order.
    ///
    /// "Changed" means **the update carried an explicit token for the field**,
    /// which is exactly what the specification's own worked examples mark
    /// *Changed* [`docs/spec/04-notifications.md` §2.5]. It is not a comparison
    /// of values: an update that sets a field to the value it already had is
    /// still a change, and the spec's third example does precisely that to its
    /// `status` field. Fields left out by an empty token or skipped by a `^N`
    /// run are absent, since both spellings mean "unchanged"
    /// [`docs/spec/04-notifications.md` §2.2].
    #[must_use]
    #[inline]
    pub fn changed_fields(&self) -> ChangedFields<'_> {
        ChangedFields {
            update: self,
            positions: self.changed.iter(),
        }
    }

    /// How many fields this update changed.
    #[must_use]
    #[inline]
    pub fn changed_count(&self) -> usize {
        self.changed.len()
    }

    /// Whether this update carried a token for the field at `position`
    /// (**1-based**).
    #[must_use]
    pub fn is_field_changed(&self, position: usize) -> bool {
        position
            .checked_sub(1)
            .is_some_and(|index| self.changed.contains(&index))
    }

    /// Whether this update carried a token for the field the caller declared as
    /// `name`. `false` if there is no such declared field.
    #[must_use]
    pub fn is_field_changed_by_name(&self, name: &str) -> bool {
        self.field_position(name)
            .is_some_and(|position| self.is_field_changed(position))
    }

    // -- COMMAND mode --------------------------------------------------------

    /// Whether this update belongs to a `COMMAND`-mode subscription, i.e. one
    /// activated by `SUBCMD` [`docs/spec/04-notifications.md` §3.2].
    #[must_use]
    #[inline]
    pub fn is_command_mode(&self) -> bool {
        self.schema.is_command_mode()
    }

    /// The row this update is about, for a `COMMAND`-mode subscription.
    ///
    /// [`None`] if the subscription is not in `COMMAND` mode. Otherwise the
    /// value of the field `SUBCMD` designated as the key
    /// [`docs/spec/04-notifications.md` §3.2] — an ordinary field, decoded like
    /// any other, so it can legitimately be [`FieldValue::Null`] and that is
    /// reported rather than hidden.
    ///
    /// # What a `DELETE` does, and does not, do here
    ///
    /// [`ItemCommand::Delete`] tells the caller to drop **its own**
    /// representation of the row. It does not erase anything inside this crate:
    /// the field values this crate keeps per item are the baseline that the
    /// next update's unchanged-field markers are resolved against
    /// [`docs/spec/04-notifications.md` §2.2], and discarding that baseline
    /// would make the very next `U` undecodable.
    ///
    /// SPEC-AMBIGUITY (command-delta-baseline): the specification "does not
    /// state ... whether unchanged-field markers in a `U` are resolved against
    /// the previous update of the same key or of the same item", noting that
    /// the decoding algorithm "speaks only of 'the previous update of the same
    /// field', without qualification by key"
    /// [`docs/spec/04-notifications.md` §3.3]. This crate takes the algorithm
    /// literally and keeps **one baseline per item**, because that is the only
    /// reading the spec actually states; a per-key baseline would be an
    /// invention, and it would have no defined value for a key seen for the
    /// first time.
    #[must_use]
    pub fn key(&self) -> Option<FieldValue<'_>> {
        let index = self.schema.key_field?;
        // 1-based for `field`.
        self.field(index.checked_add(1)?)
    }

    /// What this update did to the row named by [`key`](Self::key), for a
    /// `COMMAND`-mode subscription.
    ///
    /// [`None`] if the subscription is not in `COMMAND` mode, or if the command
    /// field is null — a null command names no operation, so this crate reports
    /// its absence rather than guessing one.
    #[must_use]
    pub fn command(&self) -> Option<ItemCommand> {
        let index = self.schema.command_field?;
        let value = self.field(index.checked_add(1)?)?;
        value.text().map(ItemCommand::from_wire)
    }
}

// ---------------------------------------------------------------------------
// Field views and iterators
// ---------------------------------------------------------------------------

/// One field of an [`ItemUpdate`]: where it sits, what it is called, what it
/// holds, and whether this update changed it.
#[derive(Clone, Copy)]
pub struct Field<'a> {
    update: &'a ItemUpdate,
    /// 0-based.
    index: usize,
}

impl<'a> Field<'a> {
    /// The field's **1-based** position in the schema.
    #[must_use]
    #[inline]
    pub const fn position(&self) -> usize {
        // Cannot overflow: `index` is an index into a `Vec`.
        self.index + 1
    }

    /// The field's name, or its 1-based position in decimal if the caller
    /// declared no name for it — see [`ItemUpdate::field_name`].
    #[must_use]
    #[inline]
    pub fn name(&self) -> &'a str {
        self.update
            .schema
            .fields
            .get(self.index)
            .map_or("", SchemaName::as_str)
    }

    /// The field's name, or [`None`] if the caller declared none.
    #[must_use]
    #[inline]
    pub fn declared_name(&self) -> Option<&'a str> {
        self.update
            .schema
            .fields
            .get(self.index)
            .and_then(SchemaName::declared_str)
    }

    /// The field's value after this update. Null and empty stay distinct; see
    /// [`FieldValue`].
    #[must_use]
    #[inline]
    pub fn value(&self) -> FieldValue<'a> {
        match self.update.values.get(self.index) {
            Some(Some(text)) => FieldValue::Text(text),
            Some(None) | None => FieldValue::Null,
        }
    }

    /// Whether this update carried a token for the field
    /// [`docs/spec/04-notifications.md` §2.5].
    #[must_use]
    #[inline]
    pub fn is_changed(&self) -> bool {
        self.update.changed.contains(&self.index)
    }
}

impl fmt::Debug for Field<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Field")
            .field("position", &self.position())
            .field("name", &self.name())
            .field("value", &self.value())
            .field("changed", &self.is_changed())
            .finish()
    }
}

/// Iterator over every field of an [`ItemUpdate`]; see
/// [`ItemUpdate::fields`].
#[derive(Clone)]
pub struct Fields<'a> {
    update: &'a ItemUpdate,
    positions: Range<usize>,
}

impl<'a> Iterator for Fields<'a> {
    type Item = Field<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        let index = self.positions.next()?;
        Some(Field {
            update: self.update,
            index,
        })
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.positions.size_hint()
    }
}

impl ExactSizeIterator for Fields<'_> {}

impl fmt::Debug for Fields<'_> {
    /// Renders as a map from field name to value, which is what makes
    /// `println!("{:?}", update.fields())` readable.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_map()
            .entries(self.clone().map(|field| (field.name(), field.value())))
            .finish()
    }
}

/// Iterator over the fields an [`ItemUpdate`] changed; see
/// [`ItemUpdate::changed_fields`].
#[derive(Clone)]
pub struct ChangedFields<'a> {
    update: &'a ItemUpdate,
    positions: std::slice::Iter<'a, usize>,
}

impl<'a> Iterator for ChangedFields<'a> {
    type Item = Field<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        let index = *self.positions.next()?;
        Some(Field {
            update: self.update,
            index,
        })
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.positions.size_hint()
    }
}

impl ExactSizeIterator for ChangedFields<'_> {}

impl fmt::Debug for ChangedFields<'_> {
    /// Renders as a map from field name to value, so the ADR-0003 usage
    /// `println!("{}: {:?}", update.item_name(), update.changed_fields())`
    /// prints something a human can read.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_map()
            .entries(self.clone().map(|field| (field.name(), field.value())))
            .finish()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn names(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    fn quote_schema() -> Arc<SubscriptionSchema> {
        Arc::new(SubscriptionSchema::new(
            2,
            3,
            &names(&["item1", "item2"]),
            &names(&["last_price", "time", "status"]),
            None,
        ))
    }

    fn update_with(values: Vec<Option<String>>, changed: Vec<usize>) -> ItemUpdate {
        ItemUpdate::new(quote_schema(), 0, UpdateKind::RealTime, values, changed)
    }

    // -- Null vs empty -------------------------------------------------------

    #[test]
    fn test_field_value_null_and_empty_text_are_distinct() {
        // `#` is null and `$` is the empty string
        // [`docs/spec/04-notifications.md` §2.2].
        assert_ne!(FieldValue::Null, FieldValue::Text(""));
        assert!(FieldValue::Null.is_null());
        assert!(!FieldValue::Null.is_empty_text());
        assert!(FieldValue::Text("").is_empty_text());
        assert!(!FieldValue::Text("").is_null());
        assert_eq!(FieldValue::Null.text(), None);
        assert_eq!(FieldValue::Text("").text(), Some(""));
    }

    #[test]
    fn test_field_value_text_or_substitutes_only_for_null() {
        assert_eq!(FieldValue::Null.text_or("-"), "-");
        assert_eq!(FieldValue::Text("").text_or("-"), "");
    }

    #[test]
    fn test_null_and_empty_reach_the_caller_distinctly() {
        let update = update_with(
            vec![None, Some(String::new()), Some("live".to_owned())],
            vec![0, 1, 2],
        );
        assert_eq!(update.field(1), Some(FieldValue::Null));
        assert_eq!(update.field(2), Some(FieldValue::Text("")));
        assert_eq!(update.field_by_name("last_price"), Some(FieldValue::Null));
        assert_eq!(update.field_by_name("time"), Some(FieldValue::Text("")));
        assert_ne!(update.field(1), update.field(2));
    }

    #[test]
    fn test_field_display_collapses_null_to_empty_deliberately() {
        assert_eq!(FieldValue::Null.to_string(), "");
        assert_eq!(FieldValue::Text("").to_string(), "");
        assert_eq!(FieldValue::Text("x").to_string(), "x");
    }

    // -- Positions and names -------------------------------------------------

    #[test]
    fn test_positions_are_one_based() {
        let update = update_with(vec![Some("a".to_owned()), None, None], vec![0]);
        // Items and fields are numbered from 1
        // [`docs/spec/04-notifications.md` §2.1].
        assert_eq!(update.item_index(), 1);
        assert_eq!(update.field(1), Some(FieldValue::Text("a")));
        assert_eq!(update.field(0), None);
        assert_eq!(update.field(4), None);
        assert_eq!(update.field_name(1), Some("last_price"));
        assert_eq!(update.field_name(0), None);
    }

    #[test]
    fn test_names_come_from_the_caller() {
        let update = update_with(vec![None, None, None], vec![]);
        assert_eq!(update.item_name(), "item1");
        assert_eq!(update.declared_item_name(), Some("item1"));
        assert_eq!(update.field_position("status"), Some(3));
    }

    #[test]
    fn test_undeclared_names_fall_back_to_the_position() {
        // `SUBOK` reports counts only [`docs/spec/04-notifications.md` §3.1].
        let schema = Arc::new(SubscriptionSchema::new(1, 2, &[], &[], None));
        let update = ItemUpdate::new(
            schema,
            0,
            UpdateKind::RealTime,
            vec![Some("a".to_owned()), None],
            vec![0],
        );
        assert_eq!(update.item_name(), "1");
        assert_eq!(update.declared_item_name(), None);
        assert_eq!(update.field_name(2), Some("2"));
        // The stand-in is not a lookup key.
        assert_eq!(update.field_by_name("2"), None);
        assert_eq!(update.field_position("1"), None);
    }

    #[test]
    fn test_partially_declared_names_are_taken_positionally() {
        let schema = Arc::new(SubscriptionSchema::new(
            1,
            3,
            &names(&["only_item"]),
            &names(&["first"]),
            None,
        ));
        let update = ItemUpdate::new(schema, 0, UpdateKind::RealTime, vec![None; 3], vec![]);
        assert_eq!(update.field_name(1), Some("first"));
        assert_eq!(update.field_name(2), Some("2"));
        assert_eq!(update.field_position("first"), Some(1));
    }

    #[test]
    fn test_unknown_field_name_is_none_everywhere() {
        let update = update_with(vec![Some("a".to_owned()), None, None], vec![0]);
        assert_eq!(update.field_by_name("no_such_field"), None);
        assert_eq!(update.field_position("no_such_field"), None);
        assert!(!update.is_field_changed_by_name("no_such_field"));
    }

    #[test]
    fn test_duplicate_declared_names_resolve_to_the_first() {
        let schema = Arc::new(SubscriptionSchema::new(
            1,
            2,
            &names(&["i"]),
            &names(&["dup", "dup"]),
            None,
        ));
        let update = ItemUpdate::new(
            schema,
            0,
            UpdateKind::RealTime,
            vec![Some("first".to_owned()), Some("second".to_owned())],
            vec![0, 1],
        );
        assert_eq!(update.field_position("dup"), Some(1));
        assert_eq!(update.field_by_name("dup"), Some(FieldValue::Text("first")));
    }

    // -- Changed fields ------------------------------------------------------

    #[test]
    fn test_changed_fields_reports_only_carried_tokens() {
        let update = update_with(
            vec![
                Some("3.04".to_owned()),
                Some("20:00:33".to_owned()),
                Some(String::new()),
            ],
            vec![0, 2],
        );
        let changed: Vec<(usize, &str)> = update
            .changed_fields()
            .map(|field| (field.position(), field.name()))
            .collect();
        assert_eq!(changed, vec![(1, "last_price"), (3, "status")]);
        assert_eq!(update.changed_count(), 2);
        assert!(update.is_field_changed(1));
        assert!(!update.is_field_changed(2));
        assert!(!update.is_field_changed(0));
        assert!(update.is_field_changed_by_name("status"));
        assert!(!update.is_field_changed_by_name("time"));
    }

    #[test]
    fn test_fields_iterates_the_whole_schema_with_changed_flags() {
        let update = update_with(
            vec![Some("a".to_owned()), None, Some(String::new())],
            vec![0, 2],
        );
        let seen: Vec<(usize, bool)> = update
            .fields()
            .map(|field| (field.position(), field.is_changed()))
            .collect();
        assert_eq!(seen, vec![(1, true), (2, false), (3, true)]);
        assert_eq!(update.fields().len(), 3);
        assert_eq!(update.changed_fields().len(), 2);
    }

    #[test]
    fn test_changed_fields_debug_is_a_readable_map() {
        // The ADR-0003 usage prints this with `{:?}`.
        let update = update_with(
            vec![Some("3.04".to_owned()), None, Some(String::new())],
            vec![0, 1],
        );
        let rendered = format!("{:?}", update.changed_fields());
        assert!(rendered.contains("last_price"), "{rendered}");
        assert!(rendered.contains("Text(\"3.04\")"), "{rendered}");
        assert!(rendered.contains("Null"), "{rendered}");
    }

    #[test]
    fn test_adr_0003_usage_compiles_and_prints() {
        // `docs/adr/0003-typed-event-stream-as-delivery-surface.md` pins this
        // exact pair of accessors.
        let update = update_with(vec![Some("3.04".to_owned()), None, None], vec![0]);
        let line = format!("{}: {:?}", update.item_name(), update.changed_fields());
        assert!(line.starts_with("item1: "), "{line}");
    }

    // -- COMMAND mode --------------------------------------------------------

    fn command_update(key: &str, command: &str) -> ItemUpdate {
        // `SUBCMD,3,1,3,1,2` places the key first and the command second
        // [`docs/spec/04-notifications.md` §3.2].
        let schema = Arc::new(SubscriptionSchema::new(
            1,
            3,
            &names(&["portfolio"]),
            &names(&["key", "command", "qty"]),
            Some((0, 1)),
        ));
        ItemUpdate::new(
            schema,
            0,
            UpdateKind::RealTime,
            vec![
                Some(key.to_owned()),
                Some(command.to_owned()),
                Some("10".to_owned()),
            ],
            vec![0, 1, 2],
        )
    }

    #[test]
    fn test_command_mode_exposes_key_and_command() {
        let update = command_update("AAPL", "ADD");
        assert!(update.is_command_mode());
        assert_eq!(update.key(), Some(FieldValue::Text("AAPL")));
        assert_eq!(update.command(), Some(ItemCommand::Add));
    }

    #[test]
    fn test_command_literals_are_case_insensitive() {
        // SPEC-AMBIGUITY (command-literals): casing is unspecified
        // [`docs/spec/04-notifications.md` §3.3].
        assert_eq!(command_update("k", "add").command(), Some(ItemCommand::Add));
        assert_eq!(
            command_update("k", "Update").command(),
            Some(ItemCommand::Update)
        );
        assert_eq!(
            command_update("k", "DELETE").command(),
            Some(ItemCommand::Delete)
        );
    }

    #[test]
    fn test_unrecognized_command_is_preserved_not_rejected() {
        let update = command_update("k", "MERGE_ROW");
        assert_eq!(
            update.command(),
            Some(ItemCommand::Unrecognized("MERGE_ROW".to_owned()))
        );
        assert_eq!(update.command().unwrap().as_str(), "MERGE_ROW");
    }

    #[test]
    fn test_command_names_render_uppercase() {
        assert_eq!(ItemCommand::Add.to_string(), "ADD");
        assert_eq!(ItemCommand::Update.as_str(), "UPDATE");
        assert_eq!(ItemCommand::Delete.as_str(), "DELETE");
    }

    #[test]
    fn test_non_command_subscription_has_no_key_or_command() {
        let update = update_with(vec![Some("a".to_owned()), None, None], vec![0]);
        assert!(!update.is_command_mode());
        assert_eq!(update.key(), None);
        assert_eq!(update.command(), None);
    }

    #[test]
    fn test_null_key_is_reported_as_null_not_missing() {
        let schema = Arc::new(SubscriptionSchema::new(
            1,
            2,
            &[],
            &names(&["key", "command"]),
            Some((0, 1)),
        ));
        let update = ItemUpdate::new(
            schema,
            0,
            UpdateKind::RealTime,
            vec![None, None],
            vec![0, 1],
        );
        // The key field is an ordinary field and may be null; that is different
        // from the subscription having no key field at all.
        assert_eq!(update.key(), Some(FieldValue::Null));
        // A null command names no operation.
        assert_eq!(update.command(), None);
    }

    // -- Snapshot classification --------------------------------------------

    #[test]
    fn test_update_kind_accessors() {
        let schema = quote_schema();
        let snapshot = ItemUpdate::new(
            Arc::clone(&schema),
            0,
            UpdateKind::Snapshot,
            vec![None; 3],
            vec![],
        );
        let live = ItemUpdate::new(schema, 1, UpdateKind::RealTime, vec![None; 3], vec![]);
        assert!(snapshot.is_snapshot());
        assert_eq!(snapshot.kind(), UpdateKind::Snapshot);
        assert!(!live.is_snapshot());
        assert_eq!(live.item_index(), 2);
        assert_eq!(live.item_name(), "item2");
    }
}
