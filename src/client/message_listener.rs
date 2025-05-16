/// Interface to be implemented to listen to `LightstreamerClient.sendMessage()` events reporting
/// a message processing outcome. Events for these listeners are dispatched by a different
/// thread than the one that generates them. All the notifications for a single `LightstreamerClient`,
/// including notifications to `ClientListener`, `SubscriptionListener` and `ClientMessageListener`
/// will be dispatched by the same thread. Only one event per message is fired on this listener.
pub trait ClientMessageListener {
    /// Event handler that is called by Lightstreamer when any notifications of the processing
    /// outcome of the related message haven't been received yet and can no longer be received.
    /// Typically, this happens after the session has been closed. In this case, the client has
    /// no way of knowing the processing outcome and any outcome is possible.
    ///
    /// # Parameters
    ///
    /// * `msg`: the message to which this notification is related.
    /// * `sent_on_network`: `true` if the message was sent on the network, `false` otherwise.
    ///   Even if the flag is `true`, it is not possible to infer whether the message actually
    ///   reached the Lightstreamer Server or not.
    fn on_abort(&self, _msg: &str, _sent_on_network: bool) {
        // Implementation for on_abort
        unimplemented!("Implement on_abort method for ClientMessageListener.");
    }

    /// Event handler that is called by Lightstreamer when the related message has been processed
    /// by the Server but the expected processing outcome could not be achieved for any reason.
    ///
    /// # Parameters
    ///
    /// * `msg`: the message to which this notification is related.
    /// * `code`: the error code sent by the Server. It can be one of the following:
    ///   - `<= 0`: the Metadata Adapter has refused the message; the code value is dependent
    ///     on the specific Metadata Adapter implementation.
    /// * `error`: the description of the error sent by the Server.
    fn on_deny(&self, _msg: &str, _code: i32, _error: &str) {
        // Implementation for on_deny
        unimplemented!("Implement on_deny method for ClientMessageListener.");
    }

    /// Event handler that is called by Lightstreamer to notify that the related message has
    /// been discarded by the Server. This means that the message has not reached the Metadata
    /// Adapter and the message next in the sequence is considered enabled for processing.
    ///
    /// # Parameters
    ///
    /// * `msg`: the message to which this notification is related.
    fn on_discarded(&self, _msg: &str) {
        // Implementation for on_discarded
        unimplemented!("Implement on_discarded method for ClientMessageListener.");
    }

    /// Event handler that is called by Lightstreamer when the related message has been processed
    /// by the Server but the processing has failed for any reason. The level of completion of
    /// the processing by the Metadata Adapter cannot be determined.
    ///
    /// # Parameters
    ///
    /// * `msg`: the message to which this notification is related.
    fn on_error(&self, _msg: &str) {
        // Implementation for on_error
        unimplemented!("Implement on_error method for ClientMessageListener.");
    }

    /// Event handler that is called by Lightstreamer when the related message has been processed
    /// by the Server with success.
    ///
    /// # Parameters
    ///
    /// * `msg`: the message to which this notification is related.
    /// * `response`: the response from the Metadata Adapter. If not supplied (i.e. supplied as `None`),
    ///   an empty message is received here.
    fn on_processed(&self, _msg: &str, _response: Option<&str>) {
        // Implementation for on_processed
        unimplemented!("Implement on_processed method for ClientMessageListener.");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    // Mock implementation of ClientMessageListener for testing
    struct TestClientMessageListener {
        on_abort_called: Arc<Mutex<bool>>,
        on_abort_message: Arc<Mutex<Option<String>>>,
        on_abort_sent_on_network: Arc<Mutex<bool>>,

        on_deny_called: Arc<Mutex<bool>>,
        on_deny_message: Arc<Mutex<Option<String>>>,
        on_deny_code: Arc<Mutex<i32>>,
        on_deny_error: Arc<Mutex<Option<String>>>,

        on_discarded_called: Arc<Mutex<bool>>,
        on_discarded_message: Arc<Mutex<Option<String>>>,

        on_error_called: Arc<Mutex<bool>>,
        on_error_message: Arc<Mutex<Option<String>>>,

        on_processed_called: Arc<Mutex<bool>>,
        on_processed_message: Arc<Mutex<Option<String>>>,
        on_processed_response: Arc<Mutex<Option<String>>>,
    }

    impl TestClientMessageListener {
        fn new() -> Self {
            TestClientMessageListener {
                on_abort_called: Arc::new(Mutex::new(false)),
                on_abort_message: Arc::new(Mutex::new(None)),
                on_abort_sent_on_network: Arc::new(Mutex::new(false)),

                on_deny_called: Arc::new(Mutex::new(false)),
                on_deny_message: Arc::new(Mutex::new(None)),
                on_deny_code: Arc::new(Mutex::new(0)),
                on_deny_error: Arc::new(Mutex::new(None)),

                on_discarded_called: Arc::new(Mutex::new(false)),
                on_discarded_message: Arc::new(Mutex::new(None)),

                on_error_called: Arc::new(Mutex::new(false)),
                on_error_message: Arc::new(Mutex::new(None)),

                on_processed_called: Arc::new(Mutex::new(false)),
                on_processed_message: Arc::new(Mutex::new(None)),
                on_processed_response: Arc::new(Mutex::new(None)),
            }
        }

        // Helper methods to check what was called
        fn was_on_abort_called(&self) -> bool {
            *self.on_abort_called.lock().unwrap()
        }

        fn get_on_abort_message(&self) -> Option<String> {
            self.on_abort_message.lock().unwrap().clone()
        }

        fn was_sent_on_network(&self) -> bool {
            *self.on_abort_sent_on_network.lock().unwrap()
        }

        fn was_on_deny_called(&self) -> bool {
            *self.on_deny_called.lock().unwrap()
        }

        fn get_on_deny_message(&self) -> Option<String> {
            self.on_deny_message.lock().unwrap().clone()
        }

        fn get_on_deny_code(&self) -> i32 {
            *self.on_deny_code.lock().unwrap()
        }

        fn get_on_deny_error(&self) -> Option<String> {
            self.on_deny_error.lock().unwrap().clone()
        }

        fn was_on_discarded_called(&self) -> bool {
            *self.on_discarded_called.lock().unwrap()
        }

        fn get_on_discarded_message(&self) -> Option<String> {
            self.on_discarded_message.lock().unwrap().clone()
        }

        fn was_on_error_called(&self) -> bool {
            *self.on_error_called.lock().unwrap()
        }

        fn get_on_error_message(&self) -> Option<String> {
            self.on_error_message.lock().unwrap().clone()
        }

        fn was_on_processed_called(&self) -> bool {
            *self.on_processed_called.lock().unwrap()
        }

        fn get_on_processed_message(&self) -> Option<String> {
            self.on_processed_message.lock().unwrap().clone()
        }

        fn get_on_processed_response(&self) -> Option<String> {
            self.on_processed_response.lock().unwrap().clone()
        }
    }

    // Implement ClientMessageListener for our test struct
    impl ClientMessageListener for TestClientMessageListener {
        fn on_abort(&self, msg: &str, sent_on_network: bool) {
            *self.on_abort_called.lock().unwrap() = true;
            *self.on_abort_message.lock().unwrap() = Some(msg.to_string());
            *self.on_abort_sent_on_network.lock().unwrap() = sent_on_network;
        }

        fn on_deny(&self, msg: &str, code: i32, error: &str) {
            *self.on_deny_called.lock().unwrap() = true;
            *self.on_deny_message.lock().unwrap() = Some(msg.to_string());
            *self.on_deny_code.lock().unwrap() = code;
            *self.on_deny_error.lock().unwrap() = Some(error.to_string());
        }

        fn on_discarded(&self, msg: &str) {
            *self.on_discarded_called.lock().unwrap() = true;
            *self.on_discarded_message.lock().unwrap() = Some(msg.to_string());
        }

        fn on_error(&self, msg: &str) {
            *self.on_error_called.lock().unwrap() = true;
            *self.on_error_message.lock().unwrap() = Some(msg.to_string());
        }

        fn on_processed(&self, msg: &str, response: Option<&str>) {
            *self.on_processed_called.lock().unwrap() = true;
            *self.on_processed_message.lock().unwrap() = Some(msg.to_string());
            *self.on_processed_response.lock().unwrap() = response.map(|s| s.to_string());
        }
    }

    // Minimal implementation of ClientMessageListener
    struct MinimalClientMessageListener;

    impl ClientMessageListener for MinimalClientMessageListener {
        // All methods use the default unimplemented! implementation
    }

    #[test]
    fn test_on_abort() {
        let listener = TestClientMessageListener::new();

        // Call the on_abort method with test values
        listener.on_abort("Test message", true);

        // Verify that the method was called and the parameters were stored correctly
        assert!(listener.was_on_abort_called());
        assert_eq!(
            listener.get_on_abort_message(),
            Some("Test message".to_string())
        );
        assert!(listener.was_sent_on_network());

        // Test with different parameters
        listener.on_abort("Another message", false);
        assert_eq!(
            listener.get_on_abort_message(),
            Some("Another message".to_string())
        );
        assert!(!listener.was_sent_on_network());
    }

    #[test]
    fn test_on_deny() {
        let listener = TestClientMessageListener::new();

        // Call the on_deny method with test values
        listener.on_deny("Test message", 123, "Test error");

        // Verify that the method was called and the parameters were stored correctly
        assert!(listener.was_on_deny_called());
        assert_eq!(
            listener.get_on_deny_message(),
            Some("Test message".to_string())
        );
        assert_eq!(listener.get_on_deny_code(), 123);
        assert_eq!(listener.get_on_deny_error(), Some("Test error".to_string()));

        // Test with different parameters
        listener.on_deny("Another message", -1, "Another error");
        assert_eq!(
            listener.get_on_deny_message(),
            Some("Another message".to_string())
        );
        assert_eq!(listener.get_on_deny_code(), -1);
        assert_eq!(
            listener.get_on_deny_error(),
            Some("Another error".to_string())
        );
    }

    #[test]
    fn test_on_discarded() {
        let listener = TestClientMessageListener::new();

        // Call the on_discarded method with test values
        listener.on_discarded("Test message");

        // Verify that the method was called and the parameters were stored correctly
        assert!(listener.was_on_discarded_called());
        assert_eq!(
            listener.get_on_discarded_message(),
            Some("Test message".to_string())
        );

        // Test with different parameters
        listener.on_discarded("Another message");
        assert_eq!(
            listener.get_on_discarded_message(),
            Some("Another message".to_string())
        );
    }

    #[test]
    fn test_on_error() {
        let listener = TestClientMessageListener::new();

        // Call the on_error method with test values
        listener.on_error("Test message");

        // Verify that the method was called and the parameters were stored correctly
        assert!(listener.was_on_error_called());
        assert_eq!(
            listener.get_on_error_message(),
            Some("Test message".to_string())
        );

        // Test with different parameters
        listener.on_error("Another message");
        assert_eq!(
            listener.get_on_error_message(),
            Some("Another message".to_string())
        );
    }

    #[test]
    fn test_on_processed() {
        let listener = TestClientMessageListener::new();

        // Call the on_processed method with test values
        listener.on_processed("Test message", Some("Test response"));

        // Verify that the method was called and the parameters were stored correctly
        assert!(listener.was_on_processed_called());
        assert_eq!(
            listener.get_on_processed_message(),
            Some("Test message".to_string())
        );
        assert_eq!(
            listener.get_on_processed_response(),
            Some("Test response".to_string())
        );

        // Test with a None response
        listener.on_processed("Another message", None);
        assert_eq!(
            listener.get_on_processed_message(),
            Some("Another message".to_string())
        );
        assert_eq!(listener.get_on_processed_response(), None);
    }

    #[test]
    #[should_panic(expected = "Implement on_abort method for ClientMessageListener")]
    fn test_default_on_abort_implementation() {
        let listener = MinimalClientMessageListener;
        listener.on_abort("Test message", true);
    }

    #[test]
    #[should_panic(expected = "Implement on_deny method for ClientMessageListener")]
    fn test_default_on_deny_implementation() {
        let listener = MinimalClientMessageListener;
        listener.on_deny("Test message", 123, "Test error");
    }

    #[test]
    #[should_panic(expected = "Implement on_discarded method for ClientMessageListener")]
    fn test_default_on_discarded_implementation() {
        let listener = MinimalClientMessageListener;
        listener.on_discarded("Test message");
    }

    #[test]
    #[should_panic(expected = "Implement on_error method for ClientMessageListener")]
    fn test_default_on_error_implementation() {
        let listener = MinimalClientMessageListener;
        listener.on_error("Test message");
    }

    #[test]
    #[should_panic(expected = "Implement on_processed method for ClientMessageListener")]
    fn test_default_on_processed_implementation() {
        let listener = MinimalClientMessageListener;
        listener.on_processed("Test message", Some("Test response"));
    }
}
