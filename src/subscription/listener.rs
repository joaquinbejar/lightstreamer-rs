use crate::subscription::ItemUpdate;

/// Interface to be implemented to listen to Subscription events comprehending notifications
/// of subscription/unsubscription, updates, errors and others.
///
/// Events for these listeners are dispatched by a different thread than the one that generates them.
/// This means that, upon reception of an event, it is possible that the internal state of the client
/// has changed. On the other hand, all the notifications for a single LightstreamerClient,
/// including notifications to ClientListener, SubscriptionListener and ClientMessageListener
/// will be dispatched by the same thread.
pub trait SubscriptionListener: Send {
    /// Event handler that is called by Lightstreamer each time a request to clear the snapshot
    /// pertaining to an item in the Subscription has been received from the Server.
    /// More precisely, this kind of request can occur in two cases:
    ///
    /// - For an item delivered in COMMAND mode, to notify that the state of the item becomes empty;
    ///   this is equivalent to receiving an update carrying a DELETE command once for each key
    ///   that is currently active.
    ///
    /// - For an item delivered in DISTINCT mode, to notify that all the previous updates received
    ///   for the item should be considered as obsolete; hence, if the listener were showing a list
    ///   of recent updates for the item, it should clear the list in order to keep a coherent view.
    ///
    /// Note that, if the involved Subscription has a two-level behavior enabled
    /// (see `Subscription::set_command_second_level_fields()` and
    /// `Subscription::set_command_second_level_field_schema()`), the notification refers to the
    /// first-level item (which is in COMMAND mode). This kind of notification is not possible
    /// for second-level items (which are in MERGE mode).
    ///
    /// # Parameters
    ///
    /// - `item_name`: name of the involved item. If the Subscription was initialized using an
    ///   "Item Group" then a `None` value is supplied.
    /// - `item_pos`: 1-based position of the item within the "Item List" or "Item Group".
    fn on_clear_snapshot(&mut self, _item_name: Option<&str>, _item_pos: usize) {
        // Default implementation does nothing.
        unimplemented!("Implement on_clear_snapshot method for SubscriptionListener.");
    }

    /// Event handler that is called by Lightstreamer to notify that, due to internal resource
    /// limitations, Lightstreamer Server dropped one or more updates for an item that was
    /// subscribed to as a second-level subscription. Such notifications are sent only if the
    /// Subscription was configured in unfiltered mode (second-level items are always in "MERGE"
    /// mode and inherit the frequency configuration from the first-level Subscription).
    ///
    /// By implementing this method it is possible to perform recovery actions.
    ///
    /// # Parameters
    ///
    /// - `lost_updates`: The number of consecutive updates dropped for the item.
    /// - `key`: The value of the key that identifies the second-level item.
    ///
    /// # See also
    ///
    /// - `Subscription::set_requested_max_frequency()`
    /// - `Subscription::set_command_second_level_fields()`
    /// - `Subscription::set_command_second_level_field_schema()`
    fn on_command_second_level_item_lost_updates(&mut self, _lost_updates: u32, _key: &str) {
        // Default implementation does nothing.
        unimplemented!(
            "Implement on_command_second_level_item_lost_updates method for SubscriptionListener."
        );
    }

    /// Event handler that is called when the Server notifies an error on a second-level subscription.
    ///
    /// By implementing this method it is possible to perform recovery actions.
    ///
    /// # Parameters
    ///
    /// - `code`: The error code sent by the Server. It can be one of the following:
    ///   - 14 - the key value is not a valid name for the Item to be subscribed; only in this case,
    ///     the error is detected directly by the library before issuing the actual request to the Server
    ///   - 17 - bad Data Adapter name or default Data Adapter not defined for the current Adapter Set
    ///   - 21 - bad Group name
    ///   - 22 - bad Group name for this Schema
    ///   - 23 - bad Schema name
    ///   - 24 - mode not allowed for an Item
    ///   - 26 - unfiltered dispatching not allowed for an Item, because a frequency limit is associated to the item
    ///   - 27 - unfiltered dispatching not supported for an Item, because a frequency prefiltering is applied for the item
    ///   - 28 - unfiltered dispatching is not allowed by the current license terms (for special licenses only)
    ///   - 66 - an unexpected exception was thrown by the Metadata Adapter while authorizing the connection
    ///   - 68 - the Server could not fulfill the request because of an internal error.
    ///   - `<= 0` - the Metadata Adapter has refused the subscription or unsubscription request;
    ///     the code value is dependent on the specific Metadata Adapter implementation
    /// - `message`: The description of the error sent by the Server; it can be `None`.
    /// - `key`: The value of the key that identifies the second-level item.
    ///
    /// # See also
    ///
    /// - `ConnectionDetails::set_adapter_set()`
    /// - `Subscription::set_command_second_level_fields()`
    /// - `Subscription::set_command_second_level_field_schema()`
    fn on_command_second_level_subscription_error(
        &mut self,
        _code: i32,
        _message: Option<&str>,
        _key: &str,
    ) {
        // Default implementation does nothing.
        unimplemented!(
            "Implement on_command_second_level_subscription_error method for SubscriptionListener."
        );
    }

    /// Event handler that is called by Lightstreamer to notify that all snapshot events for an item
    /// in the Subscription have been received, so that real time events are now going to be received.
    /// The received snapshot could be empty. Such notifications are sent only if the items are delivered
    /// in DISTINCT or COMMAND subscription mode and snapshot information was indeed requested for the items.
    /// By implementing this method it is possible to perform actions which require that all the initial
    /// values have been received.
    ///
    /// Note that, if the involved Subscription has a two-level behavior enabled
    /// (see `Subscription::set_command_second_level_fields()` and
    /// `Subscription::set_command_second_level_field_schema()`), the notification refers to the
    /// first-level item (which is in COMMAND mode). Snapshot-related updates for the second-level
    /// items (which are in MERGE mode) can be received both before and after this notification.
    ///
    /// # Parameters
    ///
    /// - `item_name`: name of the involved item. If the Subscription was initialized using an
    ///   "Item Group" then a `None` value is supplied.
    /// - `item_pos`: 1-based position of the item within the "Item List" or "Item Group".
    ///
    /// # See also
    ///
    /// - `Subscription::set_requested_snapshot()`
    /// - `ItemUpdate::is_snapshot()`
    fn on_end_of_snapshot(&mut self, _item_name: Option<&str>, _item_pos: usize) {
        // Default implementation does nothing.
        unimplemented!("Implement on_end_of_snapshot method for SubscriptionListener.");
    }

    /// Event handler that is called by Lightstreamer to notify that, due to internal resource
    /// limitations, Lightstreamer Server dropped one or more updates for an item in the Subscription.
    /// Such notifications are sent only if the items are delivered in an unfiltered mode; this occurs if the subscription mode is:
    ///
    /// - RAW
    /// - MERGE or DISTINCT, with unfiltered dispatching specified
    /// - COMMAND, with unfiltered dispatching specified
    /// - COMMAND, without unfiltered dispatching specified (in this case, notifications apply to ADD and DELETE events only)
    ///
    /// By implementing this method it is possible to perform recovery actions.
    ///
    /// # Parameters
    ///
    /// - `item_name`: name of the involved item. If the Subscription was initialized using an
    ///   "Item Group" then a `None` value is supplied.
    /// - `item_pos`: 1-based position of the item within the "Item List" or "Item Group".
    /// - `lost_updates`: The number of consecutive updates dropped for the item.
    ///
    /// # See also
    ///
    /// - `Subscription::set_requested_max_frequency()`
    fn on_item_lost_updates(
        &mut self,
        _item_name: Option<&str>,
        _item_pos: usize,
        _lost_updates: u32,
    ) {
        // Default implementation does nothing.
        unimplemented!("Implement on_item_lost_updates method for SubscriptionListener.");
    }

    /// Event handler that is called by Lightstreamer each time an update pertaining to an item
    /// in the Subscription has been received from the Server.
    ///
    /// # Parameters
    ///
    /// - `update`: a value object containing the updated values for all the fields, together with
    ///   meta-information about the update itself and some helper methods that can be used to
    ///   iterate through all or new values.
    fn on_item_update(&self, _update: &ItemUpdate) {
        // Default implementation does nothing.
        unimplemented!("Implement on_item_update method for SubscriptionListener.");
    }

    /// Event handler that receives a notification when the `SubscriptionListener` instance is
    /// removed from a `Subscription` through `Subscription::remove_listener()`. This is the last
    /// event to be fired on the listener.
    fn on_listen_end(&mut self) {
        // Default implementation does nothing.
    }

    /// Event handler that receives a notification when the `SubscriptionListener` instance is
    /// added to a `Subscription` through `Subscription::add_listener()`. This is the first event
    /// to be fired on the listener.
    fn on_listen_start(&mut self) {
        // Default implementation does nothing.
    }

    /// Event handler that is called by Lightstreamer to notify the client with the real maximum
    /// update frequency of the Subscription. It is called immediately after the Subscription is
    /// established and in response to a requested change (see `Subscription::set_requested_max_frequency()`).
    /// Since the frequency limit is applied on an item basis and a Subscription can involve multiple
    /// items, this is actually the maximum frequency among all items. For Subscriptions with two-level
    /// behavior (see `Subscription::set_command_second_level_fields()` and
    /// `Subscription::set_command_second_level_field_schema()`), the reported frequency limit applies
    /// to both first-level and second-level items.
    ///
    /// The value may differ from the requested one because of restrictions operated on the server side,
    /// but also because of number rounding.
    ///
    /// Note that a maximum update frequency (that is, a non-unlimited one) may be applied by the Server
    /// even when the subscription mode is RAW or the Subscription was done with unfiltered dispatching.
    ///
    /// # Parameters
    ///
    /// - `frequency`: A decimal number, representing the maximum frequency applied by the Server
    ///   (expressed in updates per second), or the string "unlimited". A `None` value is possible in
    ///   rare cases, when the frequency can no longer be determined.
    fn on_real_max_frequency(&mut self, _frequency: Option<f64>) {
        // Default implementation does nothing.
        unimplemented!("Implement on_real_max_frequency method for SubscriptionListener.");
    }

    /// Event handler that is called by Lightstreamer to notify that a Subscription has been successfully
    /// subscribed to through the Server. This can happen multiple times in the life of a Subscription
    /// instance, in case the Subscription is performed multiple times through `LightstreamerClient::unsubscribe()`
    /// and `LightstreamerClient::subscribe()`. This can also happen multiple times in case of automatic
    /// recovery after a connection restart.
    ///
    /// This notification is always issued before the other ones related to the same subscription.
    /// It invalidates all data that has been received previously.
    ///
    /// Note that two consecutive calls to this method are not possible, as before a second
    /// `on_subscription` event is fired an `on_unsubscription()` event is eventually fired.
    ///
    /// If the involved Subscription has a two-level behavior enabled
    /// (see `Subscription::set_command_second_level_fields()` and
    /// `Subscription::set_command_second_level_field_schema()`), second-level subscriptions are not notified.
    fn on_subscription(&mut self) {
        // Default implementation does nothing.
    }

    /// Event handler that is called when the Server notifies an error on a Subscription.
    /// By implementing this method it is possible to perform recovery actions.
    ///
    /// Note that, in order to perform a new subscription attempt, `LightstreamerClient::unsubscribe()`
    /// and `LightstreamerClient::subscribe()` should be issued again, even if no change to the
    /// Subscription attributes has been applied.
    ///
    /// # Parameters
    ///
    /// - `code`: The error code sent by the Server. It can be one of the following:
    ///   - 15 - "key" field not specified in the schema for a COMMAND mode subscription
    ///   - 16 - "command" field not specified in the schema for a COMMAND mode subscription
    ///   - 17 - bad Data Adapter name or default Data Adapter not defined for the current Adapter Set
    ///   - 21 - bad Group name
    ///   - 22 - bad Group name for this Schema
    ///   - 23 - bad Schema name
    ///   - 24 - mode not allowed for an Item
    ///   - 25 - bad Selector name
    ///   - 26 - unfiltered dispatching not allowed for an Item, because a frequency limit is associated to the item
    ///   - 27 - unfiltered dispatching not supported for an Item, because a frequency prefiltering is applied for the item
    ///   - 28 - unfiltered dispatching is not allowed by the current license terms (for special licenses only)
    ///   - 29 - RAW mode is not allowed by the current license terms (for special licenses only)
    ///   - 30 - subscriptions are not allowed by the current license terms (for special licenses only)
    ///   - 66 - an unexpected exception was thrown by the Metadata Adapter while authorizing the connection
    ///   - 68 - the Server could not fulfill the request because of an internal error.
    ///   - `<= 0` - the Metadata Adapter has refused the subscription or unsubscription request;
    ///     the code value is dependent on the specific Metadata Adapter implementation
    /// - `message`: The description of the error sent by the Server; it can be `None`.
    ///
    /// # See also
    ///
    /// - `ConnectionDetails::set_adapter_set()`
    fn on_subscription_error(&mut self, _code: i32, _message: Option<&str>) {
        // Default implementation does nothing.
        unimplemented!("Implement on_subscription_error method for SubscriptionListener.");
    }

    /// Event handler that is called by Lightstreamer to notify that a Subscription has been successfully
    /// unsubscribed from. This can happen multiple times in the life of a Subscription instance, in case
    /// the Subscription is performed multiple times through `LightstreamerClient::unsubscribe()` and
    /// `LightstreamerClient::subscribe()`. This can also happen multiple times in case of automatic
    /// recovery after a connection restart.
    ///
    /// After this notification no more events can be received until a new `on_subscription` event.
    ///
    /// Note that two consecutive calls to this method are not possible, as before a second
    /// `on_unsubscription` event is fired an `on_subscription()` event is eventually fired.
    ///
    /// If the involved Subscription has a two-level behavior enabled
    /// (see `Subscription::set_command_second_level_fields()` and
    /// `Subscription::set_command_second_level_field_schema()`), second-level unsubscriptions are not notified.
    fn on_unsubscription(&mut self) {
        // Default implementation does nothing.
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    struct TestSubscriptionListener {
        on_clear_snapshot_called: Arc<Mutex<bool>>,
        on_end_of_snapshot_called: Arc<Mutex<bool>>,
        on_item_update_called: Arc<Mutex<bool>>,
        on_subscription_called: Arc<Mutex<bool>>,
        on_unsubscription_called: Arc<Mutex<bool>>,
        on_real_max_frequency_called: Arc<Mutex<bool>>,
        item_name: Arc<Mutex<Option<String>>>,
        item_pos: Arc<Mutex<usize>>,
        max_frequency: Arc<Mutex<Option<f64>>>
    }

    impl TestSubscriptionListener {
        fn new() -> Self {
            TestSubscriptionListener {
                on_clear_snapshot_called: Arc::new(Mutex::new(false)),
                on_end_of_snapshot_called: Arc::new(Mutex::new(false)),
                on_item_update_called: Arc::new(Mutex::new(false)),
                on_subscription_called: Arc::new(Mutex::new(false)),
                on_unsubscription_called: Arc::new(Mutex::new(false)),
                on_real_max_frequency_called: Arc::new(Mutex::new(false)),
                item_name: Arc::new(Mutex::new(None)),
                item_pos: Arc::new(Mutex::new(0)),
                max_frequency: Arc::new(Mutex::new(None))
            }
        }
    }

    impl SubscriptionListener for TestSubscriptionListener {
        fn on_clear_snapshot(&mut self, item_name: Option<&str>, item_pos: usize) {
            *self.on_clear_snapshot_called.lock().unwrap() = true;
            *self.item_name.lock().unwrap() = item_name.map(|s| s.to_string());
            *self.item_pos.lock().unwrap() = item_pos;
        }

        fn on_end_of_snapshot(&mut self, item_name: Option<&str>, item_pos: usize) {
            *self.on_end_of_snapshot_called.lock().unwrap() = true;
            *self.item_name.lock().unwrap() = item_name.map(|s| s.to_string());
            *self.item_pos.lock().unwrap() = item_pos;
        }

        fn on_item_update(&self, _update: &ItemUpdate) {
            *self.on_item_update_called.lock().unwrap() = true;
        }

        fn on_subscription(&mut self) {
            *self.on_subscription_called.lock().unwrap() = true;
        }

        fn on_unsubscription(&mut self) {
            *self.on_unsubscription_called.lock().unwrap() = true;
        }

        fn on_real_max_frequency(&mut self, frequency: Option<f64>) {
            *self.on_real_max_frequency_called.lock().unwrap() = true;
            *self.max_frequency.lock().unwrap() = frequency;
        }
    }

    #[test]
    fn test_on_clear_snapshot() {
        let mut listener = TestSubscriptionListener::new();

        listener.on_clear_snapshot(Some("testItem"), 42);

        assert!(*listener.on_clear_snapshot_called.lock().unwrap());
        assert_eq!(*listener.item_name.lock().unwrap(), Some("testItem".to_string()));
        assert_eq!(*listener.item_pos.lock().unwrap(), 42);
    }

    #[test]
    fn test_on_end_of_snapshot() {
        let mut listener = TestSubscriptionListener::new();

        listener.on_end_of_snapshot(Some("testItem"), 42);

        assert!(*listener.on_end_of_snapshot_called.lock().unwrap());
        assert_eq!(*listener.item_name.lock().unwrap(), Some("testItem".to_string()));
        assert_eq!(*listener.item_pos.lock().unwrap(), 42);
    }

    #[test]
    fn test_on_item_update() {
        let listener = TestSubscriptionListener::new();

        let mut fields = HashMap::new();
        fields.insert("field1".to_string(), Some("value1".to_string()));

        let mut changed_fields = HashMap::new();
        changed_fields.insert("field1".to_string(), "value1".to_string());

        let item_update = ItemUpdate {
            item_name: Some("testItem".to_string()),
            item_pos: 42,
            fields,
            changed_fields,
            is_snapshot: false,
        };

        listener.on_item_update(&item_update);

        assert!(*listener.on_item_update_called.lock().unwrap());
    }

    #[test]
    fn test_on_subscription() {
        let mut listener = TestSubscriptionListener::new();

        listener.on_subscription();

        assert!(*listener.on_subscription_called.lock().unwrap());
    }

    #[test]
    fn test_on_unsubscription() {
        let mut listener = TestSubscriptionListener::new();

        listener.on_unsubscription();

        assert!(*listener.on_unsubscription_called.lock().unwrap());
    }

    #[test]
    fn test_on_real_max_frequency() {
        let mut listener = TestSubscriptionListener::new();

        listener.on_real_max_frequency(Some(10.5));

        assert!(*listener.on_real_max_frequency_called.lock().unwrap());
        assert_eq!(*listener.max_frequency.lock().unwrap(), Some(10.5));

        listener.on_real_max_frequency(None);
        assert_eq!(*listener.max_frequency.lock().unwrap(), None);
    }

    #[test]
    fn test_optional_methods_with_default_implementation() {
        struct MinimalListener;

        impl SubscriptionListener for MinimalListener {
        }

        let _listener = MinimalListener;

    }
}