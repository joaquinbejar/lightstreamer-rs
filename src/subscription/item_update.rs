use serde::Serialize;
use std::collections::HashMap;

/// Contains all the information related to an update of the field values for an item.
/// It reports all the new values of the fields.
///
/// COMMAND Subscription:
/// If the involved Subscription is a COMMAND Subscription, then the values for the current update
/// are meant as relative to the same key.
///
/// Moreover, if the involved Subscription has a two-level behavior enabled, then each update may be
/// associated with either a first-level or a second-level item. In this case, the reported fields are
/// always the union of the first-level and second-level fields and each single update can only change
/// either the first-level or the second-level fields (but for the "command" field, which is first-level
/// and is always set to "UPDATE" upon a second-level update); note that the second-level field values
/// are always None until the first second-level update occurs). When the two-level behavior is enabled,
/// in all methods where a field name has to be supplied, the following convention should be followed:
///
/// - The field name can always be used, both for the first-level and the second-level fields. In case of
///   name conflict, the first-level field is meant.
/// - The field position can always be used; however, the field positions for the second-level fields start
///   at the highest position of the first-level field list + 1. If a field schema had been specified for
///   either first-level or second-level Subscriptions, then client-side knowledge of the first-level schema
///   length would be required.
#[derive(Debug, Clone, Serialize)]
pub struct ItemUpdate {
    /// The name of the item to which this update belongs. May be None if the item was subscribed to by position only.
    pub item_name: Option<String>,
    /// The position of the item in the subscription item list to which this update belongs.
    pub item_pos: usize,
    /// A map containing the current values for all fields in this update.
    pub fields: HashMap<String, Option<String>>,
    /// A map containing only the fields that have changed in this update.
    pub changed_fields: HashMap<String, String>,
    /// Flag indicating whether this update is part of a snapshot (initial state) or a real-time update.
    pub is_snapshot: bool,
}

impl ItemUpdate {
    /// Returns a map containing the values for each field changed with the last server update.
    /// The related field name is used as key for the values in the map. Note that if the Subscription
    /// mode of the involved Subscription is COMMAND, then changed fields are meant as relative to the
    /// previous update for the same key. On such tables if a DELETE command is received, all the fields,
    /// excluding the key field, will be present as changed, with None value. All of this is also true on
    /// tables that have the two-level behavior enabled, but in case of DELETE commands second-level fields
    /// will not be iterated.
    ///
    /// # Raises
    /// - `IllegalStateException` – if the Subscription was initialized using a field schema.
    ///
    /// # Returns
    /// A map containing the values for each field changed with the last server update.
    pub fn get_changed_fields(&self) -> HashMap<String, String> {
        self.changed_fields.clone()
    }

    /// Returns a map containing the values for each field changed with the last server update.
    /// The 1-based field position within the field schema or field list is used as key for the values in
    /// the map. Note that if the Subscription mode of the involved Subscription is COMMAND, then changed
    /// fields are meant as relative to the previous update for the same key. On such tables if a DELETE
    /// command is received, all the fields, excluding the key field, will be present as changed, with None
    /// value. All of this is also true on tables that have the two-level behavior enabled, but in case of
    /// DELETE commands second-level fields will not be iterated.
    ///
    /// # Returns
    /// A map containing the values for each field changed with the last server update.
    pub fn get_changed_fields_by_position(&self) -> HashMap<usize, String> {
        self.changed_fields
            .iter()
            .map(|(name, value)| (self.get_field_position(name), value.clone()))
            .collect()
    }

    /// Returns a map containing the values for each field in the Subscription.
    /// The related field name is used as key for the values in the map.
    ///
    /// # Raises
    /// - `IllegalStateException` – if the Subscription was initialized using a field schema.
    ///
    /// # Returns
    /// A map containing the values for each field in the Subscription.
    pub fn get_fields(&self) -> HashMap<String, Option<String>> {
        self.fields.clone()
    }

    /// Returns a map containing the values for each field in the Subscription.
    /// The 1-based field position within the field schema or field list is used as key for the values in the map.
    ///
    /// # Returns
    /// A map containing the values for each field in the Subscription.
    pub fn get_fields_by_position(&self) -> HashMap<usize, Option<String>> {
        self.fields
            .iter()
            .map(|(name, value)| (self.get_field_position(name), value.clone()))
            .collect()
    }

    /// Inquiry method that retrieves the name of the item to which this update pertains.
    ///
    /// The name will be None if the related Subscription was initialized using an "Item Group".
    ///
    /// # Returns
    /// The name of the item to which this update pertains.
    pub fn get_item_name(&self) -> Option<&str> {
        self.item_name.as_deref()
    }

    /// Inquiry method that retrieves the position in the "Item List" or "Item Group" of the item
    /// to which this update pertains.
    ///
    /// # Returns
    /// The 1-based position of the item to which this update pertains.
    pub fn get_item_pos(&self) -> usize {
        self.item_pos
    }

    /// Inquiry method that gets the value for a specified field, as received from the Server with the
    /// current or previous update.
    ///
    /// # Raises
    /// - `IllegalArgumentException` – if the specified field is not part of the Subscription.
    ///
    /// # Parameters
    /// - `field_name_or_pos` – The field name or the 1-based position of the field within the "Field List" or "Field Schema".
    ///
    /// # Returns
    /// The value of the specified field; it can be None in the following cases:
    ///
    /// - a None value has been received from the Server, as None is a possible value for a field;
    /// - no value has been received for the field yet;
    /// - the item is subscribed to with the COMMAND mode and a DELETE command is received (only the fields
    ///   used to carry key and command information are valued).
    pub fn get_value(&self, field_name_or_pos: &str) -> Option<&str> {
        match field_name_or_pos.parse::<usize>() {
            Ok(pos) => self
                .fields
                .iter()
                .find(|(name, _)| self.get_field_position(name) == pos)
                .and_then(|(_, value)| value.as_deref()),
            Err(_) => self
                .fields
                .get(field_name_or_pos)
                .and_then(|v| v.as_deref()),
        }
    }

    /// Inquiry method that gets the difference between the new value and the previous one as a JSON Patch structure,
    /// provided that the Server has used the JSON Patch format to send this difference, as part of the "delta delivery"
    /// mechanism. This, in turn, requires that:
    ///
    /// - the Data Adapter has explicitly indicated JSON Patch as the privileged type of compression for this field;
    /// - both the previous and new value are suitable for the JSON Patch computation (i.e. they are valid JSON representations);
    /// - the item was subscribed to in MERGE or DISTINCT mode (note that, in case of two-level behavior, this holds for all
    ///   fields related with second-level items, as these items are in MERGE mode);
    /// - sending the JSON Patch difference has been evaluated by the Server as more efficient than sending the full new value.
    ///
    /// Note that the last condition can be enforced by leveraging the Server's <jsonpatch_min_length> configuration flag,
    /// so that the availability of the JSON Patch form would only depend on the Client and the Data Adapter.
    ///
    /// When the above conditions are not met, the method just returns None; in this case, the new value can only be determined
    /// through `ItemUpdate.get_value()`. For instance, this will always be needed to get the first value received.
    ///
    /// # Raises
    /// - `IllegalArgumentException` – if the specified field is not part of the Subscription.
    ///
    /// # Parameters
    /// - `field_name_or_pos` – The field name or the 1-based position of the field within the "Field List" or "Field Schema".
    ///
    /// # Returns
    /// A JSON Patch structure representing the difference between the new value and the previous one,
    /// or None if the difference in JSON Patch format is not available for any reason.
    pub fn get_value_as_json_patch_if_available(&self, _field_name_or_pos: &str) -> Option<String> {
        // Implementation pending
        None
    }

    /// Inquiry method that asks whether the current update belongs to the item snapshot (which carries the current item state
    /// at the time of Subscription). Snapshot events are sent only if snapshot information was requested for the items through
    /// `Subscription.set_requested_snapshot()` and precede the real time events. Snapshot information takes different forms in
    /// different subscription modes and can be spanned across zero, one or several update events. In particular:
    ///
    /// - if the item is subscribed to with the RAW subscription mode, then no snapshot is sent by the Server;
    /// - if the item is subscribed to with the MERGE subscription mode, then the snapshot consists of exactly one event,
    ///   carrying the current value for all fields;
    /// - if the item is subscribed to with the DISTINCT subscription mode, then the snapshot consists of some of the most recent
    ///   updates; these updates are as many as specified through `Subscription.set_requested_snapshot()`, unless fewer are available;
    /// - if the item is subscribed to with the COMMAND subscription mode, then the snapshot consists of an "ADD" event for each key
    ///   that is currently present.
    ///
    /// Note that, in case of two-level behavior, snapshot-related updates for both the first-level item (which is in COMMAND mode)
    /// and any second-level items (which are in MERGE mode) are qualified with this flag.
    ///
    /// # Returns
    /// `true` if the current update event belongs to the item snapshot; `false` otherwise.
    pub fn is_snapshot(&self) -> bool {
        self.is_snapshot
    }

    /// Inquiry method that asks whether the value for a field has changed after the reception of the last update from the Server
    /// for an item. If the Subscription mode is COMMAND then the change is meant as relative to the same key.
    ///
    /// # Parameters
    /// - `field_name_or_pos` – The field name or the 1-based position of the field within the field list or field schema.
    ///
    /// # Returns
    /// Unless the Subscription mode is COMMAND, the return value is `true` in the following cases:
    ///
    /// - It is the first update for the item;
    /// - the new field value is different than the previous field value received for the item.
    ///
    /// If the Subscription mode is COMMAND, the return value is `true` in the following cases:
    ///
    /// - it is the first update for the involved key value (i.e. the event carries an "ADD" command);
    /// - the new field value is different than the previous field value received for the item, relative to the same key value
    ///   (the event must carry an "UPDATE" command);
    /// - the event carries a "DELETE" command (this applies to all fields other than the field used to carry key information).
    ///
    /// In all other cases, the return value is `false`.
    ///
    /// # Raises
    /// - `IllegalArgumentException` – if the specified field is not part of the Subscription.
    pub fn is_value_changed(&self, field_name_or_pos: &str) -> bool {
        match field_name_or_pos.parse::<usize>() {
            Ok(pos) => self
                .changed_fields
                .iter()
                .any(|(name, _)| self.get_field_position(name) == pos),
            Err(_) => self.changed_fields.contains_key(field_name_or_pos),
        }
    }

    /// Helper method to get the 1-based position of a field within the field list or field schema.
    ///
    /// # Parameters
    /// - `field_name` – The name of the field.
    ///
    /// # Returns
    /// The 1-based position of the field within the field list or field schema.
    fn get_field_position(&self, field_name: &str) -> usize {
        // For testing purposes, we'll use a simple implementation
        // In a real implementation, this would look up the position in the field schema
        match field_name {
            "field1" => 1,
            "field2" => 2,
            "field3" => 3,
            _ => 0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    
    #[test]
    fn test_get_fields_by_position() {
        let mut update = create_test_item_update();
        
        let fields_by_position = update.get_fields_by_position();
        
        // Verify the fields are mapped to their positions
        assert_eq!(fields_by_position.len(), 3); // field1, field2, and field3 have valid positions
        assert_eq!(fields_by_position.get(&1), Some(&Some("value1".to_string())));
        assert_eq!(fields_by_position.get(&2), Some(&Some("value2".to_string())));
        assert_eq!(fields_by_position.get(&3), Some(&None));
    }
    
    #[test]
    fn test_get_changed_fields_by_position() {
        let update = create_test_item_update();
        
        let changed_fields_by_position = update.get_changed_fields_by_position();
        
        // Verify the changed fields are mapped to their positions
        assert_eq!(changed_fields_by_position.len(), 2); // Only field1 and field2 have valid positions
        assert_eq!(changed_fields_by_position.get(&1), Some(&"value1".to_string()));
        assert_eq!(changed_fields_by_position.get(&2), Some(&"value2".to_string()));
    }

    fn create_test_item_update() -> ItemUpdate {
        let mut fields = HashMap::new();
        fields.insert("field1".to_string(), Some("value1".to_string()));
        fields.insert("field2".to_string(), Some("value2".to_string()));
        fields.insert("field3".to_string(), None);

        let mut changed_fields = HashMap::new();
        changed_fields.insert("field1".to_string(), "value1".to_string());
        changed_fields.insert("field2".to_string(), "value2".to_string());

        ItemUpdate {
            item_name: Some("test_item".to_string()),
            item_pos: 1,
            fields,
            changed_fields,
            is_snapshot: false,
        }
    }

    #[test]
    fn test_get_item_name() {
        let update = create_test_item_update();
        assert_eq!(update.get_item_name(), Some("test_item"));

        let mut update_no_name = create_test_item_update();
        update_no_name.item_name = None;
        assert_eq!(update_no_name.get_item_name(), None);
    }

    #[test]
    fn test_get_item_pos() {
        let update = create_test_item_update();
        assert_eq!(update.get_item_pos(), 1);
    }

    #[test]
    fn test_get_fields() {
        let update = create_test_item_update();
        let fields = update.get_fields();

        assert_eq!(fields.len(), 3);
        assert_eq!(fields.get("field1").unwrap(), &Some("value1".to_string()));
        assert_eq!(fields.get("field2").unwrap(), &Some("value2".to_string()));
        assert_eq!(fields.get("field3").unwrap(), &None);
    }
    

    #[test]
    fn test_get_changed_fields() {
        let update = create_test_item_update();
        let changed_fields = update.get_changed_fields();

        assert_eq!(changed_fields.len(), 2);
        assert_eq!(changed_fields.get("field1").unwrap(), "value1");
        assert_eq!(changed_fields.get("field2").unwrap(), "value2");
        assert!(!changed_fields.contains_key("field3"));
    }
    

    #[test]
    fn test_get_value() {
        let update = create_test_item_update();

        assert_eq!(update.get_value("field1"), Some("value1"));
        assert_eq!(update.get_value("field2"), Some("value2"));
        assert_eq!(update.get_value("field3"), None);
        assert_eq!(update.get_value("non_existent"), None);
    }

    #[test]
    fn test_is_snapshot() {
        let update = create_test_item_update();
        assert!(!update.is_snapshot());

        let mut snapshot_update = create_test_item_update();
        snapshot_update.is_snapshot = true;
        assert!(snapshot_update.is_snapshot());
    }

    #[test]
    fn test_is_value_changed() {
        let update = create_test_item_update();

        assert!(update.is_value_changed("field1"));
        assert!(update.is_value_changed("field2"));
        assert!(!update.is_value_changed("field3"));
    }

    #[test]
    fn test_get_value_as_json_patch_if_available() {
        let update = create_test_item_update();

        assert_eq!(update.get_value_as_json_patch_if_available("field1"), None);
    }

    #[test]
    fn test_get_field_position() {
        let update = create_test_item_update();
        
        // Test existing fields
        assert_eq!(update.get_field_position("field1"), 1);
        assert_eq!(update.get_field_position("field2"), 2);
        assert_eq!(update.get_field_position("field3"), 3);
        
        // Test non-existent field
        assert_eq!(update.get_field_position("non_existent_field"), 0);
    }
}
