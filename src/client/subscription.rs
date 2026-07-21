//! What to subscribe to, and how.
//!
//! A **subscription** binds a set of *items* to a set of *fields* and a
//! *mode*. The server's Metadata Adapter resolves the item group and the field
//! schema into the actual items and fields, so what a group name means is
//! decided by the server, not by this crate
//! [`docs/spec/03-requests.md` §6.1].

use std::num::NonZeroU32;

use crate::config::ConfigError;
use crate::protocol::request::{
    DecimalNumber, RequestedBufferSize, RequestedMaxFrequency, Snapshot as WireSnapshot,
    SubscriptionMode as WireMode,
};
use crate::session::SubscriptionSpec;

/// How the server treats the items of a subscription
/// [`docs/spec/03-requests.md` §6.1].
///
/// The mode is not a preference — it changes what the data *means*, and an
/// item may only be subscribed in the modes its Data Adapter supports.
///
/// | Mode | The server keeps | A snapshot is | Typical use |
/// |---|---|---|---|
/// | [`Merge`](SubscriptionMode::Merge) | the current value of every field | the current state | a quote: price, bid, ask |
/// | [`Distinct`](SubscriptionMode::Distinct) | recent events, unmerged | the most recent events | a news feed, a chat |
/// | [`Command`](SubscriptionMode::Command) | a keyed table | the rows of the table | an order book, a portfolio |
/// | [`Raw`](SubscriptionMode::Raw) | nothing | not available | a firehose you filter yourself |
///
/// In every mode but `Raw` the server may filter, buffer and limit the flow;
/// `Raw` forwards each update as it is and is the one mode with no server-side
/// bookkeeping at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum SubscriptionMode {
    /// Every update is forwarded as-is. No item state is kept, no snapshot is
    /// available, and no filtering is applied.
    Raw,
    /// The server keeps the current value of each field and merges each update
    /// into it, so an update carries only what changed and a snapshot is the
    /// current state.
    Merge,
    /// Each update is a distinct event that does not replace the previous one.
    /// A snapshot is the most recent events, up to a length the server or the
    /// subscription chooses.
    Distinct,
    /// Updates carry `ADD`, `UPDATE` and `DELETE` commands against a key
    /// field, forming a table. The field schema **must** contain a `key` field
    /// and a `command` field, or the server rejects the subscription with
    /// Appendix B code 15 or 16 [`docs/spec/05-error-codes.md` §2].
    Command,
}

impl SubscriptionMode {
    /// The mode as the protocol spells it: `RAW`, `MERGE`, `DISTINCT` or
    /// `COMMAND`.
    #[must_use]
    #[inline]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Raw => "RAW",
            Self::Merge => "MERGE",
            Self::Distinct => "DISTINCT",
            Self::Command => "COMMAND",
        }
    }

    /// The wire enum this crate encodes with.
    pub(crate) const fn to_wire(self) -> WireMode {
        match self {
            Self::Raw => WireMode::Raw,
            Self::Merge => WireMode::Merge,
            Self::Distinct => WireMode::Distinct,
            Self::Command => WireMode::Command,
        }
    }
}

/// Which items a subscription covers.
///
/// The protocol sends a single `LS_group` string and lets the server's
/// Metadata Adapter decide what it names
/// [`docs/spec/03-requests.md` §6.1]. In practice adapters use one of two
/// conventions, and this type supports both:
///
/// - a **group name** the adapter knows, resolved server-side into a list;
/// - a **literal list** of item names separated by spaces, which is what the
///   stock adapters and the public demo server accept.
///
/// # Examples
///
/// ```
/// use lightstreamer_rs::ItemGroup;
///
/// let listed = ItemGroup::from_items(["item1", "item2", "item3"])?;
/// assert_eq!(listed.as_str(), "item1 item2 item3");
///
/// let named = ItemGroup::try_new("portfolio1")?;
/// assert_eq!(named.as_str(), "portfolio1");
/// # Ok::<(), lightstreamer_rs::ConfigError>(())
/// ```
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ItemGroup {
    text: String,
    /// Whether the caller spelled the items out. When it did, this crate knows
    /// each position's name and can report it on every update; when the group
    /// is a server-side name, nothing in TLCP ever reveals the item names
    /// [`docs/spec/04-notifications.md` §3.1] and positions are reported by
    /// index instead.
    listed: bool,
}

impl ItemGroup {
    /// Wraps a group name the server's Metadata Adapter will resolve.
    ///
    /// # Errors
    ///
    /// - [`ConfigError::EmptyItemGroup`] if the name is empty or only
    ///   whitespace.
    /// - [`ConfigError::ItemGroupControlCharacter`] if it contains an ASCII
    ///   control character. A guard of this crate: the specification gives no
    ///   grammar for the name, but a control character in one is always a
    ///   mistake.
    #[must_use = "a checked item group does nothing unless it is used"]
    pub fn try_new(group: impl Into<String>) -> Result<Self, ConfigError> {
        let group = group.into();
        if group.trim().is_empty() {
            return Err(ConfigError::EmptyItemGroup);
        }
        if group.chars().any(char::is_control) {
            return Err(ConfigError::ItemGroupControlCharacter);
        }
        Ok(Self {
            text: group,
            listed: false,
        })
    }

    /// Builds the space-separated list convention from individual item names.
    ///
    /// # Errors
    ///
    /// - [`ConfigError::EmptyItemGroup`] if the iterator is empty.
    /// - [`ConfigError::ItemNameHasWhitespace`] if any name contains
    ///   whitespace, which would make it two items once joined.
    /// - [`ConfigError::ItemGroupControlCharacter`] if any name contains an
    ///   ASCII control character.
    ///
    /// # Examples
    ///
    /// ```
    /// use lightstreamer_rs::{ConfigError, ItemGroup};
    ///
    /// assert!(matches!(
    ///     ItemGroup::from_items(["two words"]),
    ///     Err(ConfigError::ItemNameHasWhitespace { .. })
    /// ));
    /// ```
    #[must_use = "a checked item group does nothing unless it is used"]
    pub fn from_items<I, S>(items: I) -> Result<Self, ConfigError>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let names: Vec<String> = items.into_iter().map(Into::into).collect();
        if names.is_empty() {
            return Err(ConfigError::EmptyItemGroup);
        }
        for name in &names {
            if name.is_empty() || name.chars().any(char::is_whitespace) {
                return Err(ConfigError::ItemNameHasWhitespace { name: name.clone() });
            }
            if name.chars().any(char::is_control) {
                return Err(ConfigError::ItemGroupControlCharacter);
            }
        }
        Ok(Self {
            text: names.join(" "),
            listed: true,
        })
    }

    /// The group as it goes on the wire, in `LS_group`.
    #[must_use]
    #[inline]
    pub fn as_str(&self) -> &str {
        &self.text
    }

    /// The item names, when the caller spelled them out.
    ///
    /// Empty for a group *name* the server resolves: TLCP never transmits item
    /// names, so this crate cannot know them and reports each item by its
    /// 1-based position instead [`docs/spec/04-notifications.md` §3.1].
    ///
    /// # Examples
    ///
    /// ```
    /// use lightstreamer_rs::ItemGroup;
    ///
    /// assert_eq!(ItemGroup::from_items(["a", "b"])?.names(), vec!["a", "b"]);
    /// assert!(ItemGroup::try_new("portfolio1")?.names().is_empty());
    /// # Ok::<(), lightstreamer_rs::ConfigError>(())
    /// ```
    #[must_use]
    pub fn names(&self) -> Vec<&str> {
        if self.listed {
            self.text.split_whitespace().collect()
        } else {
            Vec::new()
        }
    }
}

/// Which fields a subscription asks for.
///
/// The mirror of [`ItemGroup`] on the field side: `LS_schema` is one string
/// the Metadata Adapter resolves, either a schema name it knows or a
/// space-separated list of field names [`docs/spec/03-requests.md` §6.1].
///
/// The **order** matters: it is the order in which values arrive in every
/// update, and the order the field indices of an update refer to.
///
/// # Examples
///
/// ```
/// use lightstreamer_rs::FieldSchema;
///
/// let schema = FieldSchema::from_fields(["last_price", "time", "pct_change"])?;
/// assert_eq!(schema.as_str(), "last_price time pct_change");
/// # Ok::<(), lightstreamer_rs::ConfigError>(())
/// ```
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FieldSchema {
    text: String,
    /// Whether the caller spelled the fields out. See [`ItemGroup::listed`].
    listed: bool,
}

impl FieldSchema {
    /// Wraps a schema name the server's Metadata Adapter will resolve.
    ///
    /// # Errors
    ///
    /// - [`ConfigError::EmptyFieldSchema`] if the name is empty or only
    ///   whitespace.
    /// - [`ConfigError::FieldSchemaControlCharacter`] if it contains an ASCII
    ///   control character.
    #[must_use = "a checked field schema does nothing unless it is used"]
    pub fn try_new(schema: impl Into<String>) -> Result<Self, ConfigError> {
        let schema = schema.into();
        if schema.trim().is_empty() {
            return Err(ConfigError::EmptyFieldSchema);
        }
        if schema.chars().any(char::is_control) {
            return Err(ConfigError::FieldSchemaControlCharacter);
        }
        Ok(Self {
            text: schema,
            listed: false,
        })
    }

    /// Builds the space-separated list convention from individual field names.
    ///
    /// # Errors
    ///
    /// - [`ConfigError::EmptyFieldSchema`] if the iterator is empty.
    /// - [`ConfigError::FieldNameHasWhitespace`] if any name contains
    ///   whitespace.
    /// - [`ConfigError::FieldSchemaControlCharacter`] if any name contains an
    ///   ASCII control character.
    #[must_use = "a checked field schema does nothing unless it is used"]
    pub fn from_fields<I, S>(fields: I) -> Result<Self, ConfigError>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let names: Vec<String> = fields.into_iter().map(Into::into).collect();
        if names.is_empty() {
            return Err(ConfigError::EmptyFieldSchema);
        }
        for name in &names {
            if name.is_empty() || name.chars().any(char::is_whitespace) {
                return Err(ConfigError::FieldNameHasWhitespace { name: name.clone() });
            }
            if name.chars().any(char::is_control) {
                return Err(ConfigError::FieldSchemaControlCharacter);
            }
        }
        Ok(Self {
            text: names.join(" "),
            listed: true,
        })
    }

    /// The schema as it goes on the wire, in `LS_schema`.
    #[must_use]
    #[inline]
    pub fn as_str(&self) -> &str {
        &self.text
    }

    /// The field names, when the caller spelled them out.
    ///
    /// Empty for a schema *name* the server resolves: TLCP announces only how
    /// many fields a subscription has, never what they are called
    /// [`docs/spec/04-notifications.md` §3.1], so in that case every field is
    /// reported by its 1-based position instead.
    #[must_use]
    pub fn names(&self) -> Vec<&str> {
        if self.listed {
            self.text.split_whitespace().collect()
        } else {
            Vec::new()
        }
    }
}

/// Whether the server should send the current state before the live flow
/// [`docs/spec/03-requests.md` §6.1].
///
/// A snapshot lets an application start from a complete picture instead of
/// waiting for every field to tick. Its meaning depends on the mode: in
/// [`SubscriptionMode::Merge`] it is the current value of every field, in
/// [`SubscriptionMode::Distinct`] the most recent events, in
/// [`SubscriptionMode::Command`] the rows of the table. It is not available in
/// [`SubscriptionMode::Raw`] at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
#[repr(u8)]
pub enum Snapshot {
    /// Do not send a snapshot; start from the live flow. The server's default.
    #[default]
    Off,
    /// Send the snapshot, which may legitimately be empty.
    On,
    /// Send at most this many snapshot events.
    ///
    /// Accepted **only** with [`SubscriptionMode::Distinct`]; the server
    /// rejects it for the other modes. Fewer events may arrive, if the server
    /// has fewer or imposes a stricter limit.
    Length(NonZeroU32),
}

impl Snapshot {
    /// Whether this asks the server for a snapshot at all.
    #[must_use]
    #[inline]
    pub const fn is_requested(self) -> bool {
        !matches!(self, Self::Off)
    }

    /// The wire enum this crate encodes with.
    const fn to_wire(self) -> WireSnapshot {
        match self {
            Self::Off => WireSnapshot::Off,
            Self::On => WireSnapshot::On,
            Self::Length(n) => WireSnapshot::Length(n),
        }
    }
}

/// A ceiling on how often the server may send updates for an item
/// [`docs/spec/03-requests.md` §6.1].
///
/// Frequency limiting happens **on the server**, before the data is sent, so
/// it is the effective way to protect a slow consumer: this crate never drops
/// an update it has received (see [`Updates`](crate::Updates)).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum MaxFrequency {
    /// No limit: each update is sent as soon as possible. The server's
    /// default.
    #[default]
    Unlimited,
    /// No limit **and no losses**: the server never merges or discards an
    /// update to keep up. If it cannot, it says so with an overflow
    /// notification rather than dropping silently
    /// [`docs/spec/04-notifications.md` §3]. Not compatible with a buffer size
    /// request, which the server ignores in this case.
    Unfiltered,
    /// At most this many updates per second, per item.
    Limited(FrequencyLimit),
}

impl MaxFrequency {
    /// A ceiling in updates per second, written as a decimal number.
    ///
    /// # Errors
    ///
    /// [`ConfigError::InvalidFrequency`] if the text is not a plain decimal
    /// number with a dot separator — `2`, `0.5` and `12.75` are accepted; a
    /// sign, an exponent, a comma separator and a bare `.5` are not.
    ///
    /// # Examples
    ///
    /// ```
    /// use lightstreamer_rs::{ConfigError, MaxFrequency};
    ///
    /// let twice_a_second = MaxFrequency::limited("2.0")?;
    /// assert!(matches!(MaxFrequency::limited("2,0"), Err(ConfigError::InvalidFrequency { .. })));
    /// # Ok::<(), ConfigError>(())
    /// ```
    pub fn limited(updates_per_second: impl Into<String>) -> Result<Self, ConfigError> {
        let text = updates_per_second.into();
        DecimalNumber::try_new(text.clone())
            .map(|number| Self::Limited(FrequencyLimit(number)))
            .map_err(|_| ConfigError::InvalidFrequency { value: text })
    }

    /// The wire enum this crate encodes with.
    fn to_wire(&self) -> RequestedMaxFrequency {
        match self {
            Self::Unlimited => RequestedMaxFrequency::Unlimited,
            Self::Unfiltered => RequestedMaxFrequency::Unfiltered,
            Self::Limited(limit) => RequestedMaxFrequency::Limited(limit.0.clone()),
        }
    }
}

/// A checked decimal frequency, in updates per second.
///
/// Held as text rather than as a number because the protocol carries it as
/// text: `2.0` and `2` are the same rate but not the same bytes, and this
/// crate sends back exactly what the caller wrote. It never does arithmetic on
/// the value.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FrequencyLimit(DecimalNumber);

impl FrequencyLimit {
    /// The limit as it goes on the wire.
    #[must_use]
    #[inline]
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

/// How much the server may buffer per item before it has to drop or merge
/// [`docs/spec/03-requests.md` §6.1].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum BufferSize {
    /// Let the server choose from its own configuration. The default.
    #[default]
    ServerDecides,
    /// Ask for no limit. The server may still impose one.
    Unlimited,
    /// Ask for this many update events.
    Events(NonZeroU32),
}

impl BufferSize {
    /// The wire enum this crate encodes with, or `None` to omit the parameter.
    const fn to_wire(self) -> Option<RequestedBufferSize> {
        match self {
            Self::ServerDecides => None,
            Self::Unlimited => Some(RequestedBufferSize::Unlimited),
            Self::Events(n) => Some(RequestedBufferSize::Events(n)),
        }
    }
}

/// A request to subscribe to a set of items.
///
/// Build one, hand it to [`Client::subscribe`](crate::Client::subscribe), and
/// consume the [`Updates`](crate::Updates) stream you get back. The three
/// values with no default — the mode, the items and the fields — are taken by
/// [`Subscription::new`]; everything else is optional and defaults to letting
/// the server decide.
///
/// # Examples
///
/// ```
/// use lightstreamer_rs::{FieldSchema, ItemGroup, Snapshot, Subscription, SubscriptionMode};
///
/// let subscription = Subscription::new(
///     SubscriptionMode::Merge,
///     ItemGroup::from_items(["item1", "item2"])?,
///     FieldSchema::from_fields(["last_price", "time"])?,
/// )
/// .with_snapshot(Snapshot::On);
///
/// assert_eq!(subscription.mode(), SubscriptionMode::Merge);
/// # Ok::<(), lightstreamer_rs::ConfigError>(())
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Subscription {
    mode: SubscriptionMode,
    items: ItemGroup,
    fields: FieldSchema,
    data_adapter: Option<String>,
    selector: Option<String>,
    snapshot: Snapshot,
    max_frequency: MaxFrequency,
    buffer_size: BufferSize,
}

impl Subscription {
    /// A subscription to a set of items, for a set of fields, in a mode.
    #[must_use = "builders do nothing unless the subscription is used"]
    pub fn new(mode: SubscriptionMode, items: ItemGroup, fields: FieldSchema) -> Self {
        Self {
            mode,
            items,
            fields,
            data_adapter: None,
            selector: None,
            snapshot: Snapshot::default(),
            max_frequency: MaxFrequency::default(),
            buffer_size: BufferSize::default(),
        }
    }

    /// Names the Data Adapter within the session's Adapter Set that provides
    /// these items.
    ///
    /// Left unset, the server uses the Adapter Set's default Data Adapter
    /// [`docs/spec/03-requests.md` §6.1].
    #[must_use = "builders do nothing unless the subscription is used"]
    pub fn with_data_adapter(mut self, name: impl Into<String>) -> Self {
        self.data_adapter = Some(name.into());
        self
    }

    /// Passes a selector name to the Metadata Adapter, which uses it to filter
    /// the updates of every item in the subscription
    /// [`docs/spec/03-requests.md` §6.1].
    ///
    /// What a selector name means is entirely up to the adapter.
    #[must_use = "builders do nothing unless the subscription is used"]
    pub fn with_selector(mut self, name: impl Into<String>) -> Self {
        self.selector = Some(name.into());
        self
    }

    /// Asks for a snapshot before the live flow.
    #[must_use = "builders do nothing unless the subscription is used"]
    pub const fn with_snapshot(mut self, snapshot: Snapshot) -> Self {
        self.snapshot = snapshot;
        self
    }

    /// Caps how often the server sends updates for each item.
    #[must_use = "builders do nothing unless the subscription is used"]
    pub fn with_max_frequency(mut self, frequency: MaxFrequency) -> Self {
        self.max_frequency = frequency;
        self
    }

    /// Asks the server for a particular per-item buffer size.
    #[must_use = "builders do nothing unless the subscription is used"]
    pub const fn with_buffer_size(mut self, size: BufferSize) -> Self {
        self.buffer_size = size;
        self
    }

    /// The mode this subscription was built with.
    #[must_use]
    #[inline]
    pub const fn mode(&self) -> SubscriptionMode {
        self.mode
    }

    /// The items this subscription covers.
    #[must_use]
    #[inline]
    pub const fn items(&self) -> &ItemGroup {
        &self.items
    }

    /// The fields this subscription asks for.
    #[must_use]
    #[inline]
    pub const fn fields(&self) -> &FieldSchema {
        &self.fields
    }

    /// Whether a snapshot was asked for.
    #[must_use]
    #[inline]
    pub const fn snapshot(&self) -> Snapshot {
        self.snapshot
    }

    /// The item names the caller declared, or an empty vector for a
    /// server-resolved group.
    pub(crate) fn declared_items(&self) -> Vec<String> {
        self.items.names().into_iter().map(str::to_owned).collect()
    }

    /// The field names the caller declared, or an empty vector for a
    /// server-resolved schema.
    pub(crate) fn declared_fields(&self) -> Vec<String> {
        self.fields.names().into_iter().map(str::to_owned).collect()
    }

    /// Translates into what the session state machine subscribes with.
    pub(crate) fn into_spec(self) -> SubscriptionSpec {
        SubscriptionSpec {
            group: self.items.text,
            schema: self.fields.text,
            mode: self.mode.to_wire(),
            data_adapter: self.data_adapter,
            selector: self.selector,
            requested_buffer_size: self.buffer_size.to_wire(),
            requested_max_frequency: match self.max_frequency {
                // Omitting the parameter and asking for `unlimited` mean the
                // same thing to the server, and omitting it is the smaller
                // request [`docs/spec/03-requests.md` §6.1].
                MaxFrequency::Unlimited => None,
                ref other => Some(other.to_wire()),
            },
            snapshot: match self.snapshot {
                // Likewise: `false` is the documented default.
                Snapshot::Off => None,
                other => Some(other.to_wire()),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn group() -> ItemGroup {
        match ItemGroup::from_items(["item1", "item2"]) {
            Ok(group) => group,
            Err(error) => unreachable!("the fixture group is valid: {error}"),
        }
    }

    fn schema() -> FieldSchema {
        match FieldSchema::from_fields(["last_price", "time"]) {
            Ok(schema) => schema,
            Err(error) => unreachable!("the fixture schema is valid: {error}"),
        }
    }

    #[test]
    fn test_item_group_joins_a_list_with_spaces() {
        assert!(matches!(
            ItemGroup::from_items(["a", "b", "c"]),
            Ok(group) if group.as_str() == "a b c"
        ));
    }

    #[test]
    fn test_item_group_rejects_an_empty_list() {
        let empty: [&str; 0] = [];
        assert!(matches!(
            ItemGroup::from_items(empty),
            Err(ConfigError::EmptyItemGroup)
        ));
    }

    #[test]
    fn test_item_group_rejects_a_name_with_whitespace() {
        assert!(matches!(
            ItemGroup::from_items(["ok", "not ok"]),
            Err(ConfigError::ItemNameHasWhitespace { name }) if name == "not ok"
        ));
    }

    #[test]
    fn test_item_group_rejects_an_empty_name_in_a_list() {
        assert!(matches!(
            ItemGroup::from_items(["ok", ""]),
            Err(ConfigError::ItemNameHasWhitespace { .. })
        ));
    }

    #[test]
    fn test_item_group_rejects_an_empty_group_name() {
        assert!(matches!(
            ItemGroup::try_new("  "),
            Err(ConfigError::EmptyItemGroup)
        ));
    }

    #[test]
    fn test_item_group_rejects_a_control_character() {
        assert!(matches!(
            ItemGroup::try_new("a\rb"),
            Err(ConfigError::ItemGroupControlCharacter)
        ));
    }

    #[test]
    fn test_field_schema_rejects_every_bad_shape() {
        let empty: [&str; 0] = [];
        assert!(matches!(
            FieldSchema::from_fields(empty),
            Err(ConfigError::EmptyFieldSchema)
        ));
        assert!(matches!(
            FieldSchema::from_fields(["a b"]),
            Err(ConfigError::FieldNameHasWhitespace { .. })
        ));
        assert!(matches!(
            FieldSchema::try_new(""),
            Err(ConfigError::EmptyFieldSchema)
        ));
        assert!(matches!(
            FieldSchema::try_new("a\nb"),
            Err(ConfigError::FieldSchemaControlCharacter)
        ));
    }

    #[test]
    fn test_field_schema_lists_its_names() {
        assert_eq!(schema().names(), vec!["last_price", "time"]);
    }

    #[test]
    fn test_max_frequency_accepts_a_decimal_number() {
        assert!(MaxFrequency::limited("2").is_ok());
        assert!(MaxFrequency::limited("0.5").is_ok());
    }

    #[test]
    fn test_max_frequency_rejects_anything_that_is_not_one() {
        for bad in ["2,0", ".5", "5.", "1e3", "-1", ""] {
            assert!(
                matches!(
                    MaxFrequency::limited(bad),
                    Err(ConfigError::InvalidFrequency { .. })
                ),
                "{bad} was accepted"
            );
        }
    }

    #[test]
    fn test_snapshot_knows_when_it_asks_for_one() {
        assert!(!Snapshot::Off.is_requested());
        assert!(Snapshot::On.is_requested());
        assert!(Snapshot::Length(NonZeroU32::MIN).is_requested());
    }

    #[test]
    fn test_subscription_defaults_omit_every_optional_parameter() {
        let spec = Subscription::new(SubscriptionMode::Merge, group(), schema()).into_spec();
        assert_eq!(spec.group, "item1 item2");
        assert_eq!(spec.schema, "last_price time");
        assert_eq!(spec.mode, WireMode::Merge);
        assert_eq!(spec.data_adapter, None);
        assert_eq!(spec.selector, None);
        assert_eq!(spec.snapshot, None);
        assert_eq!(spec.requested_max_frequency, None);
        assert_eq!(spec.requested_buffer_size, None);
    }

    #[test]
    fn test_subscription_carries_every_option_it_was_given() {
        let frequency = match MaxFrequency::limited("2.5") {
            Ok(frequency) => frequency,
            Err(error) => unreachable!("the fixture frequency is valid: {error}"),
        };
        let spec = Subscription::new(SubscriptionMode::Distinct, group(), schema())
            .with_data_adapter("QUOTE_ADAPTER")
            .with_selector("only_big")
            .with_snapshot(Snapshot::Length(NonZeroU32::MIN))
            .with_max_frequency(frequency)
            .with_buffer_size(BufferSize::Unlimited)
            .into_spec();

        assert_eq!(spec.mode, WireMode::Distinct);
        assert_eq!(spec.data_adapter.as_deref(), Some("QUOTE_ADAPTER"));
        assert_eq!(spec.selector.as_deref(), Some("only_big"));
        assert_eq!(spec.snapshot, Some(WireSnapshot::Length(NonZeroU32::MIN)));
        assert_eq!(
            spec.requested_buffer_size,
            Some(RequestedBufferSize::Unlimited)
        );
        assert!(matches!(
            spec.requested_max_frequency,
            Some(RequestedMaxFrequency::Limited(_))
        ));
    }

    #[test]
    fn test_subscription_mode_spells_itself_the_way_the_protocol_does() {
        assert_eq!(SubscriptionMode::Raw.as_str(), "RAW");
        assert_eq!(SubscriptionMode::Merge.as_str(), "MERGE");
        assert_eq!(SubscriptionMode::Distinct.as_str(), "DISTINCT");
        assert_eq!(SubscriptionMode::Command.as_str(), "COMMAND");
    }
}
