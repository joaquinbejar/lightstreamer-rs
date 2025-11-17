// Copyright (C) 2024 Joaquín Béjar García
// Portions of this file are derived from lightstreamer-client
// Copyright (C) 2024 Daniel López Azaña
// Original project: https://github.com/daniloaz/lightstreamer-client
//
// This file is part of lightstreamer-rs.
//
// lightstreamer-rs is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// lightstreamer-rs is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with lightstreamer-rs. If not, see <https://www.gnu.org/licenses/>.

use crate::subscription::ItemUpdate;
use tokio::sync::mpsc;

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

/// A subscription listener that forwards item updates to a tokio mpsc channel.
///
/// This listener allows decoupling the reception of updates from their processing,
/// enabling asynchronous consumption of updates by other tasks or components.
///
/// # Examples
///
/// ```ignore
/// use lightstreamer_rs::subscription::{ChannelSubscriptionListener, ItemUpdate};
/// use tokio::sync::mpsc;
///
/// let (tx, mut rx) = mpsc::unbounded_channel();
/// let listener = ChannelSubscriptionListener::new(tx);
///
/// // Add listener to subscription
/// subscription.add_listener(Box::new(listener));
///
/// // Process updates in a separate task
/// tokio::spawn(async move {
///     while let Some(update) = rx.recv().await {
///         println!("Received update: {:?}", update);
///     }
/// });
/// ```
pub struct ChannelSubscriptionListener {
    /// Channel sender for forwarding item updates.
    sender: mpsc::UnboundedSender<ItemUpdate>,
}

impl ChannelSubscriptionListener {
    /// Creates a new `ChannelSubscriptionListener` with the provided sender.
    ///
    /// # Arguments
    ///
    /// * `sender` - An unbounded mpsc sender for forwarding `ItemUpdate` events
    ///
    /// # Returns
    ///
    /// A new instance of `ChannelSubscriptionListener`
    pub fn new(sender: mpsc::UnboundedSender<ItemUpdate>) -> Self {
        Self { sender }
    }

    /// Creates a new channel pair and returns both the listener and receiver.
    ///
    /// This is a convenience method that creates both the channel and the listener
    /// in a single call.
    ///
    /// # Returns
    ///
    /// A tuple containing:
    /// - The `ChannelSubscriptionListener` instance
    /// - The receiver end of the channel for consuming updates
    ///
    /// # Examples
    ///
    /// ```ignore
    /// use lightstreamer_rs::subscription::ChannelSubscriptionListener;
    ///
    /// let (listener, mut rx) = ChannelSubscriptionListener::create_channel();
    /// subscription.add_listener(Box::new(listener));
    ///
    /// tokio::spawn(async move {
    ///     while let Some(update) = rx.recv().await {
    ///         // Process update
    ///     }
    /// });
    /// ```
    pub fn create_channel() -> (Self, mpsc::UnboundedReceiver<ItemUpdate>) {
        let (tx, rx) = mpsc::unbounded_channel();
        (Self::new(tx), rx)
    }
}

impl SubscriptionListener for ChannelSubscriptionListener {
    fn on_item_update(&self, update: &ItemUpdate) {
        // Clone the update and send it through the channel
        // If send fails, the receiver has been dropped, which is acceptable
        let _ = self.sender.send(update.clone());
    }

    fn on_subscription(&mut self) {
        // Optional: Could send a special message or log this event
    }

    fn on_unsubscription(&mut self) {
        // Optional: Could send a special message or log this event
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
        max_frequency: Arc<Mutex<Option<f64>>>,
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
                max_frequency: Arc::new(Mutex::new(None)),
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
        assert_eq!(
            *listener.item_name.lock().unwrap(),
            Some("testItem".to_string())
        );
        assert_eq!(*listener.item_pos.lock().unwrap(), 42);
    }

    #[test]
    fn test_on_end_of_snapshot() {
        let mut listener = TestSubscriptionListener::new();

        listener.on_end_of_snapshot(Some("testItem"), 42);

        assert!(*listener.on_end_of_snapshot_called.lock().unwrap());
        assert_eq!(
            *listener.item_name.lock().unwrap(),
            Some("testItem".to_string())
        );
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

        impl SubscriptionListener for MinimalListener {}

        let _listener = MinimalListener;
    }

    #[test]
    #[should_panic(expected = "Implement on_clear_snapshot method for SubscriptionListener.")]
    fn test_default_on_clear_snapshot_implementation() {
        struct MinimalListener;
        impl SubscriptionListener for MinimalListener {}

        let mut listener = MinimalListener;
        listener.on_clear_snapshot(Some("item"), 1);
    }

    #[test]
    #[should_panic(
        expected = "Implement on_command_second_level_item_lost_updates method for SubscriptionListener."
    )]
    fn test_default_on_command_second_level_item_lost_updates_implementation() {
        struct MinimalListener;
        impl SubscriptionListener for MinimalListener {}

        let mut listener = MinimalListener;
        listener.on_command_second_level_item_lost_updates(5, "key");
    }

    #[test]
    #[should_panic(
        expected = "Implement on_command_second_level_subscription_error method for SubscriptionListener."
    )]
    fn test_default_on_command_second_level_subscription_error_implementation() {
        struct MinimalListener;
        impl SubscriptionListener for MinimalListener {}

        let mut listener = MinimalListener;
        listener.on_command_second_level_subscription_error(1, Some("error"), "key");
    }

    #[test]
    #[should_panic(expected = "Implement on_end_of_snapshot method for SubscriptionListener.")]
    fn test_default_on_end_of_snapshot_implementation() {
        struct MinimalListener;
        impl SubscriptionListener for MinimalListener {}

        let mut listener = MinimalListener;
        listener.on_end_of_snapshot(Some("item"), 1);
    }

    #[test]
    #[should_panic(expected = "Implement on_item_lost_updates method for SubscriptionListener.")]
    fn test_default_on_item_lost_updates_implementation() {
        struct MinimalListener;
        impl SubscriptionListener for MinimalListener {}

        let mut listener = MinimalListener;
        listener.on_item_lost_updates(Some("item"), 1, 5);
    }

    #[test]
    #[should_panic(expected = "Implement on_item_update method for SubscriptionListener.")]
    fn test_default_on_item_update_implementation() {
        struct MinimalListener;
        impl SubscriptionListener for MinimalListener {}

        let listener = MinimalListener;

        let mut fields = HashMap::new();
        fields.insert("field1".to_string(), Some("value1".to_string()));

        let mut changed_fields = HashMap::new();
        changed_fields.insert("field1".to_string(), "value1".to_string());

        let item_update = ItemUpdate {
            item_name: Some("testItem".to_string()),
            item_pos: 1,
            fields,
            changed_fields,
            is_snapshot: false,
        };

        listener.on_item_update(&item_update);
    }

    #[test]
    #[should_panic(expected = "Implement on_real_max_frequency method for SubscriptionListener.")]
    fn test_default_on_real_max_frequency_implementation() {
        struct MinimalListener;
        impl SubscriptionListener for MinimalListener {}

        let mut listener = MinimalListener;
        listener.on_real_max_frequency(Some(10.0));
    }

    #[test]
    #[should_panic(expected = "Implement on_subscription_error method for SubscriptionListener.")]
    fn test_default_on_subscription_error_implementation() {
        struct MinimalListener;
        impl SubscriptionListener for MinimalListener {}

        let mut listener = MinimalListener;
        listener.on_subscription_error(1, Some("error"));
    }

    #[tokio::test]
    async fn test_channel_subscription_listener_new() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let _listener = super::ChannelSubscriptionListener::new(tx);
        // If we get here without panic, the test passes
    }

    #[tokio::test]
    async fn test_channel_subscription_listener_create_channel() {
        let (_listener, _rx) = super::ChannelSubscriptionListener::create_channel();
        // If we get here without panic, the test passes
    }

    #[tokio::test]
    async fn test_channel_subscription_listener_forwards_updates() {
        let (listener, mut rx) = super::ChannelSubscriptionListener::create_channel();

        let mut fields = HashMap::new();
        fields.insert("field1".to_string(), Some("value1".to_string()));

        let mut changed_fields = HashMap::new();
        changed_fields.insert("field1".to_string(), "value1".to_string());

        let item_update = ItemUpdate {
            item_name: Some("testItem".to_string()),
            item_pos: 1,
            fields,
            changed_fields,
            is_snapshot: false,
        };

        // Send update through listener
        listener.on_item_update(&item_update);

        // Receive update from channel
        let received = rx.recv().await.expect("Should receive update");

        assert_eq!(received.item_name, Some("testItem".to_string()));
        assert_eq!(received.item_pos, 1);
        assert_eq!(received.get_value("field1"), Some("value1"));
    }

    #[tokio::test]
    async fn test_channel_subscription_listener_multiple_updates() {
        let (listener, mut rx) = super::ChannelSubscriptionListener::create_channel();

        // Send multiple updates
        for i in 1..=5 {
            let mut fields = HashMap::new();
            fields.insert("field1".to_string(), Some(format!("value{}", i)));

            let mut changed_fields = HashMap::new();
            changed_fields.insert("field1".to_string(), format!("value{}", i));

            let item_update = ItemUpdate {
                item_name: Some(format!("item{}", i)),
                item_pos: i,
                fields,
                changed_fields,
                is_snapshot: false,
            };

            listener.on_item_update(&item_update);
        }

        // Receive all updates
        for i in 1..=5 {
            let received = rx.recv().await.expect("Should receive update");
            assert_eq!(received.item_name, Some(format!("item{}", i)));
            assert_eq!(received.item_pos, i);
        }
    }

    #[tokio::test]
    async fn test_channel_subscription_listener_dropped_receiver() {
        let (listener, rx) = super::ChannelSubscriptionListener::create_channel();

        // Drop the receiver
        drop(rx);

        let mut fields = HashMap::new();
        fields.insert("field1".to_string(), Some("value1".to_string()));

        let mut changed_fields = HashMap::new();
        changed_fields.insert("field1".to_string(), "value1".to_string());

        let item_update = ItemUpdate {
            item_name: Some("testItem".to_string()),
            item_pos: 1,
            fields,
            changed_fields,
            is_snapshot: false,
        };

        // This should not panic even though receiver is dropped
        listener.on_item_update(&item_update);
    }

    #[tokio::test]
    async fn test_channel_subscription_listener_lifecycle_methods() {
        let (mut listener, _rx) = super::ChannelSubscriptionListener::create_channel();

        // These should not panic
        listener.on_subscription();
        listener.on_unsubscription();
    }
}
