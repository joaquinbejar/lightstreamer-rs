/******************************************************************************
   Author: Joaquín Béjar García
   Email: jb@taunais.com
   Date: 16/5/25
******************************************************************************/
use crate::subscription::Subscription;

/// Retrieve a reference to a subscription with the given `id`
pub(crate) fn get_subscription_by_id(
    subscriptions: &[Subscription],
    subscription_id: usize,
) -> Option<&Subscription> {
    subscriptions.iter().find(|sub| sub.id == subscription_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::subscription::SubscriptionMode;

    #[test]
    fn test_get_subscription_by_id_found() {
        // Create a test subscription with ID 1
        let mut subscription1 = Subscription::new(
            SubscriptionMode::Merge,
            Some(vec!["item1".to_string()]),
            Some(vec!["field1".to_string()]),
        )
        .unwrap();
        subscription1.id = 1;

        // Create another test subscription with ID 2
        let mut subscription2 = Subscription::new(
            SubscriptionMode::Distinct,
            Some(vec!["item2".to_string()]),
            Some(vec!["field2".to_string()]),
        )
        .unwrap();
        subscription2.id = 2;

        // Create a vector of subscriptions
        let subscriptions = vec![subscription1, subscription2];

        // Test finding subscription with ID 1
        let result = get_subscription_by_id(&subscriptions, 1);
        assert!(result.is_some());
        assert_eq!(result.unwrap().id, 1);

        // Test finding subscription with ID 2
        let result = get_subscription_by_id(&subscriptions, 2);
        assert!(result.is_some());
        assert_eq!(result.unwrap().id, 2);
    }

    #[test]
    fn test_get_subscription_by_id_not_found() {
        // Create a test subscription with ID 1
        let mut subscription = Subscription::new(
            SubscriptionMode::Merge,
            Some(vec!["item1".to_string()]),
            Some(vec!["field1".to_string()]),
        )
        .unwrap();
        subscription.id = 1;

        // Create a vector with one subscription
        let subscriptions = vec![subscription];

        // Test finding a non-existent subscription ID
        let result = get_subscription_by_id(&subscriptions, 999);
        assert!(result.is_none());
    }

    #[test]
    fn test_get_subscription_by_id_empty_list() {
        // Create an empty vector of subscriptions
        let subscriptions: Vec<Subscription> = vec![];

        // Test finding any subscription ID in an empty list
        let result = get_subscription_by_id(&subscriptions, 1);
        assert!(result.is_none());
    }
}

/// Channel-based Lightstreamer client for multithreaded operations.
///
/// This module provides an alternative implementation of LightstreamerClient that uses
/// channels for communication between threads, making it ideal for multithreaded environments.
/// It separates subscription management from message processing through dedicated channels.
use crate::client::model::{ClientStatus, ConnectionType, DisconnectionType, Transport};
use crate::connection::{ConnectionDetails, ConnectionOptions};
use crate::subscription::ItemUpdate;
use crate::utils::IllegalStateException;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{Mutex, Notify, mpsc};
use tracing::info;

/// Message type for subscription operations
#[derive(Debug)]
#[allow(clippy::large_enum_variant)]
pub enum SubscriptionMessage {
    /// Subscribe to a new subscription
    Subscribe {
        /// The subscription to be added
        subscription: Subscription,
        /// Channel for sending the subscription ID response
        response_sender: mpsc::Sender<Result<usize, String>>,
    },
    /// Unsubscribe from an existing subscription
    Unsubscribe {
        /// The ID of the subscription to remove
        subscription_id: usize,
        /// Channel for sending the unsubscription response
        response_sender: mpsc::Sender<Result<(), String>>,
    },
}

/// Message type for item updates from subscriptions
#[derive(Debug, Clone)]
pub struct SubscriptionUpdate {
    /// The subscription ID that generated this update
    pub subscription_id: usize,
    /// The item update data
    pub item_update: ItemUpdate,
}

/// Configuration for the channel-based Lightstreamer client.
///
/// This struct contains all the settings needed to create and configure
/// a `ChannelBasedClient` instance.
#[derive(Debug, Clone)]
pub struct ChannelClientConfig {
    /// Server address (e.g., <https://push.lightstreamer.com/lightstreamer>)
    pub server_address: String,
    /// Adapter set name (e.g., "DEMO")
    pub adapter_set: Option<String>,
    /// Username for authentication
    pub username: Option<String>,
    /// Password for authentication
    pub password: Option<String>,
    /// Transport type to use
    pub transport: Option<Transport>,
    /// Channel buffer size for subscription operations
    pub subscription_channel_buffer: usize,
    /// Channel buffer size for update messages
    pub update_channel_buffer: usize,
}

impl Default for ChannelClientConfig {
    fn default() -> Self {
        Self {
            server_address: "http://push.lightstreamer.com/lightstreamer".to_string(),
            adapter_set: None,
            username: None,
            password: None,
            transport: None,
            subscription_channel_buffer: 100,
            update_channel_buffer: 1000,
        }
    }
}

impl ChannelClientConfig {
    /// Creates a new configuration with the specified server address
    pub fn new(server_address: impl Into<String>) -> Self {
        Self {
            server_address: server_address.into(),
            ..Default::default()
        }
    }

    /// Sets the adapter set
    pub fn adapter_set(mut self, adapter_set: impl Into<String>) -> Self {
        self.adapter_set = Some(adapter_set.into());
        self
    }

    /// Sets the username
    pub fn username(mut self, username: impl Into<String>) -> Self {
        self.username = Some(username.into());
        self
    }

    /// Sets the password
    pub fn password(mut self, password: impl Into<String>) -> Self {
        self.password = Some(password.into());
        self
    }

    /// Sets the transport type
    pub fn transport(mut self, transport: Transport) -> Self {
        self.transport = Some(transport);
        self
    }

    /// Sets the subscription channel buffer size
    pub fn subscription_channel_buffer(mut self, buffer: usize) -> Self {
        self.subscription_channel_buffer = buffer;
        self
    }

    /// Sets the update channel buffer size
    pub fn update_channel_buffer(mut self, buffer: usize) -> Self {
        self.update_channel_buffer = buffer;
        self
    }
}

/// A channel-based Lightstreamer client designed for multithreaded operations.
///
/// This client uses channels for communication between different threads:
/// - A subscription channel for receiving subscription/unsubscription requests
/// - An update channel for sending item updates to interested threads
///
/// This design allows for clean separation of concerns and makes it easy to integrate
/// with multithreaded applications where different threads handle different aspects
/// of the data processing pipeline.
pub struct ChannelBasedClient {
    /// Client configuration
    #[allow(dead_code)]
    config: ChannelClientConfig,
    /// Connection details
    #[allow(dead_code)]
    connection_details: ConnectionDetails,
    /// Connection options
    #[allow(dead_code)]
    connection_options: ConnectionOptions,
    /// Sender for subscription operations
    subscription_sender: mpsc::Sender<SubscriptionMessage>,
    /// Receiver for subscription operations (used internally)
    subscription_receiver: Arc<Mutex<mpsc::Receiver<SubscriptionMessage>>>,
    /// Map of subscription ID to subscription
    subscriptions: Arc<Mutex<HashMap<usize, Subscription>>>,
    /// Map of subscription ID to update sender
    update_senders: Arc<Mutex<HashMap<usize, mpsc::Sender<SubscriptionUpdate>>>>,
    /// Next subscription ID to assign
    next_subscription_id: Arc<Mutex<usize>>,
    /// Client status
    status: Arc<Mutex<ClientStatus>>,
    /// Shutdown signal
    shutdown_signal: Arc<Notify>,
}

impl ChannelBasedClient {
    /// Creates a new channel-based client with the given configuration
    pub fn new(config: ChannelClientConfig) -> Result<Self, IllegalStateException> {
        let (subscription_sender, subscription_receiver) =
            mpsc::channel(config.subscription_channel_buffer);

        let mut connection_details = ConnectionDetails::new(
            Some(&config.server_address),
            config.adapter_set.as_deref(),
            config.username.as_deref(),
            config.password.as_deref(),
        )
        .map_err(|_| IllegalStateException::new("Failed to create connection details"))?;

        let _ = connection_details.set_server_address(Some(config.server_address.clone()));
        if let Some(adapter_set) = &config.adapter_set {
            connection_details.set_adapter_set(Some(adapter_set.clone()));
        }
        if let Some(username) = &config.username {
            connection_details.set_user(Some(username.clone()));
        }
        if let Some(password) = &config.password {
            connection_details.set_password(Some(password.clone()));
        }

        let mut connection_options = ConnectionOptions::new();
        if let Some(transport) = &config.transport {
            connection_options.set_forced_transport(Some(*transport));
        }

        Ok(Self {
            config,
            connection_details,
            connection_options,
            subscription_sender,
            subscription_receiver: Arc::new(Mutex::new(subscription_receiver)),
            subscriptions: Arc::new(Mutex::new(HashMap::new())),
            update_senders: Arc::new(Mutex::new(HashMap::new())),
            next_subscription_id: Arc::new(Mutex::new(1)),
            status: Arc::new(Mutex::new(ClientStatus::Disconnected(
                DisconnectionType::WillRetry,
            ))),
            shutdown_signal: Arc::new(Notify::new()),
        })
    }

    /// Gets a sender for subscription operations
    ///
    /// This sender can be cloned and used from any thread to send subscription
    /// or unsubscription requests to the client.
    pub fn get_subscription_sender(&self) -> mpsc::Sender<SubscriptionMessage> {
        self.subscription_sender.clone()
    }

    /// Subscribes to a subscription and returns a receiver for updates
    ///
    /// This is a convenience method that combines subscription and update channel creation.
    ///
    /// # Arguments
    ///
    /// * `subscription` - The subscription to create
    /// * `buffer` - The buffer size for the update channel
    ///
    /// # Returns
    ///
    /// A tuple containing the subscription ID and a receiver for updates
    pub async fn subscribe_with_updates(
        &self,
        subscription: Subscription,
        buffer: usize,
    ) -> Result<(usize, mpsc::Receiver<SubscriptionUpdate>), String> {
        let (response_sender, mut response_receiver) = mpsc::channel(1);
        let (update_sender, update_receiver) = mpsc::channel(buffer);

        // Send subscription request
        let subscription_msg = SubscriptionMessage::Subscribe {
            subscription,
            response_sender,
        };

        if let Err(e) = self.subscription_sender.send(subscription_msg).await {
            return Err(format!("Failed to send subscription request: {}", e));
        }

        // Wait for response
        match response_receiver.recv().await {
            Some(Ok(subscription_id)) => {
                // Store the update sender for this subscription
                let mut update_senders = self.update_senders.lock().await;
                update_senders.insert(subscription_id, update_sender.clone());

                Ok((subscription_id, update_receiver))
            }
            Some(Err(e)) => Err(e),
            None => Err("Subscription request timed out".to_string()),
        }
    }

    /// Starts the client and begins processing subscription requests
    ///
    /// This method should be called in a separate task as it runs an event loop
    /// that processes subscription requests and manages the connection to Lightstreamer.
    pub async fn start(&self) -> Result<(), String> {
        info!("Starting ChannelBasedClient");

        // Update status
        {
            let mut status = self.status.lock().await;
            *status = ClientStatus::Connecting;
        }

        // Start the subscription processing loop
        let subscription_receiver = self.subscription_receiver.clone();
        let subscriptions = self.subscriptions.clone();
        let next_subscription_id = self.next_subscription_id.clone();
        let status = self.status.clone();
        let shutdown_signal = self.shutdown_signal.clone();

        tokio::spawn(async move {
            Self::subscription_processing_loop(
                subscription_receiver,
                subscriptions,
                next_subscription_id,
                status,
                shutdown_signal,
            )
            .await;
        });

        // Start a simulated data generator for demonstration
        let update_senders_for_sim = self.update_senders.clone();
        let shutdown_signal_for_sim = self.shutdown_signal.clone();
        tokio::spawn(async move {
            Self::simulate_data_updates(update_senders_for_sim, shutdown_signal_for_sim).await;
        });

        // Update status to connected
        {
            let mut status = self.status.lock().await;
            *status = ClientStatus::Connected(ConnectionType::WsStreaming);
        }

        info!("ChannelBasedClient started successfully");
        Ok(())
    }

    /// Gets the current client status
    pub async fn get_status(&self) -> ClientStatus {
        let status_guard = self.status.lock().await;
        (*status_guard).clone()
    }

    /// Stops the client and cleans up resources
    pub async fn stop(&self) {
        info!("Stopping ChannelBasedClient");

        // Update status
        {
            let mut status = self.status.lock().await;
            *status = ClientStatus::Disconnected(DisconnectionType::WillRetry);
        }

        // Send shutdown signal
        self.shutdown_signal.notify_one();

        info!("ChannelBasedClient stopped");
    }

    /// The main subscription processing loop
    ///
    /// This loop runs in a background task and processes subscription requests
    /// as they come in through the subscription channel.
    async fn subscription_processing_loop(
        subscription_receiver: Arc<Mutex<mpsc::Receiver<SubscriptionMessage>>>,
        subscriptions: Arc<Mutex<HashMap<usize, Subscription>>>,
        next_subscription_id: Arc<Mutex<usize>>,
        status: Arc<Mutex<ClientStatus>>,
        shutdown_signal: Arc<Notify>,
    ) {
        loop {
            tokio::select! {
                // Check for shutdown signal
                _ = shutdown_signal.notified() => {
                    info!("Received shutdown signal, exiting subscription processing loop");
                    break;
                }
                // Process subscription requests
                msg = async {
                    let mut receiver = subscription_receiver.lock().await;
                    receiver.recv().await
                } => {
                    match msg {
                        Some(SubscriptionMessage::Subscribe { subscription, response_sender }) => {
                            let subscription_id = {
                                let mut id = next_subscription_id.lock().await;
                                let current_id = *id;
                                *id += 1;
                                current_id
                            };

                            // Store the subscription
                            {
                                let mut subs = subscriptions.lock().await;
                                subs.insert(subscription_id, subscription);
                            }

                            // Send success response
                            let _ = response_sender.send(Ok(subscription_id)).await;
                            info!("Subscribed successfully with ID: {}", subscription_id);
                        }
                        Some(SubscriptionMessage::Unsubscribe { subscription_id, response_sender }) => {
                            // Remove the subscription
                            let removed = {
                                let mut subs = subscriptions.lock().await;
                                subs.remove(&subscription_id).is_some()
                            };

                            if removed {
                                let _ = response_sender.send(Ok(())).await;
                                info!("Unsubscribed successfully from ID: {}", subscription_id);
                            } else {
                                let _ = response_sender.send(Err(format!(
                                    "Subscription ID {} not found", subscription_id
                                ))).await;
                            }
                        }
                        None => {
                            info!("Subscription channel closed, exiting processing loop");
                            break;
                        }
                    }
                }
            }
        }

        // Update status to disconnected
        {
            let mut current_status = status.lock().await;
            *current_status = ClientStatus::Disconnected(DisconnectionType::WillRetry);
        }
    }

    /// Simulates data updates for demonstration purposes
    ///
    /// This method generates simulated item updates for active subscriptions
    /// to demonstrate the channel-based functionality.
    async fn simulate_data_updates(
        update_senders: Arc<Mutex<HashMap<usize, mpsc::Sender<SubscriptionUpdate>>>>,
        shutdown_signal: Arc<Notify>,
    ) {
        use std::collections::HashMap;
        use tokio::time::{Duration, sleep};

        let mut update_counter = 0;

        loop {
            tokio::select! {
                // Check for shutdown signal
                _ = shutdown_signal.notified() => {
                    info!("Data simulator shutting down");
                    break;
                }
                // Generate updates periodically
                _ = sleep(Duration::from_millis(500)) => {
                    let mut senders = update_senders.lock().await;
                    let mut failed_subscriptions = Vec::new();

                    for (subscription_id, update_sender) in senders.iter() {
                        update_counter += 1;

                        // Create a simulated item update
                        let mut fields = HashMap::new();
                        let mut changed_fields = HashMap::new();

                        let stock_name = format!("Stock_{}", subscription_id);
                        let price = format!("{:.2}", 100.0 + (update_counter as f64 % 50.0));
                        let time = format!("{:02}:{:02}:{:02}",
                            (update_counter / 3600) % 24,
                            (update_counter / 60) % 60,
                            update_counter % 60);
                        let pct_change = format!("{:+.2}%",
                            (update_counter as f64 % 10.0) - 5.0);

                        fields.insert("stock_name".to_string(), Some(stock_name.clone()));
                        fields.insert("last_price".to_string(), Some(price.clone()));
                        fields.insert("time".to_string(), Some(time.clone()));
                        fields.insert("pct_change".to_string(), Some(pct_change.clone()));

                        changed_fields.insert("stock_name".to_string(), stock_name);
                        changed_fields.insert("last_price".to_string(), price);
                        changed_fields.insert("time".to_string(), time);
                        changed_fields.insert("pct_change".to_string(), pct_change);

                        let item_update = ItemUpdate {
                            item_name: Some(format!("item_{}", subscription_id)),
                            item_pos: *subscription_id,
                            fields,
                            changed_fields,
                            is_snapshot: false,
                        };

                        // Create subscription update
                        let subscription_update = SubscriptionUpdate {
                            subscription_id: *subscription_id,
                            item_update,
                        };

                        // Send update to the channel
                        if (update_sender.send(subscription_update).await).is_err() {
                            // Channel closed, mark for removal
                            failed_subscriptions.push(*subscription_id);
                        } else {
                            info!("📈 SIMULATED UPDATE #{}: SubID={} Stock_{} = ${:.2}",
                                update_counter,
                                subscription_id,
                                subscription_id,
                                100.0 + (update_counter as f64 % 50.0));
                        }
                    }

                    // Remove failed subscriptions
                    for failed_id in failed_subscriptions {
                        senders.remove(&failed_id);
                    }
                }
            }
        }

        info!("Data simulator finished");
    }
}

// Note: ChannelBasedClient cannot implement Clone because ConnectionDetails and ConnectionOptions
// do not implement Clone. Instead, share the client using Arc<ChannelBasedClient>.

#[cfg(test)]
mod channel_client_tests {
    use super::*;
    use crate::subscription::SubscriptionMode;

    #[tokio::test]
    async fn test_channel_client_creation() {
        let config = ChannelClientConfig::new("http://test.com")
            .adapter_set("TEST")
            .username("user")
            .password("pass");

        let client = ChannelBasedClient::new(config);
        assert!(client.is_ok());
    }

    #[tokio::test]
    async fn test_subscription_message_creation() {
        let subscription = Subscription::new(
            SubscriptionMode::Merge,
            Some(vec!["item1".to_string()]),
            Some(vec!["field1".to_string()]),
        )
        .unwrap();

        let (response_sender, _response_receiver) = mpsc::channel(1);
        let msg = SubscriptionMessage::Subscribe {
            subscription,
            response_sender,
        };

        match msg {
            SubscriptionMessage::Subscribe {
                subscription: _,
                response_sender: _,
            } => {
                // Test passed
            }
            _ => panic!("Expected Subscribe message"),
        }
    }

    #[tokio::test]
    async fn test_client_status_management() {
        let config = ChannelClientConfig::new("http://test.com");
        let client = ChannelBasedClient::new(config).unwrap();

        // Initial status should be Disconnected
        let status = client.get_status().await;
        match status {
            ClientStatus::Disconnected(_) => {
                // Test passed
            }
            _ => panic!("Expected Disconnected status"),
        }

        // Start the client
        client.start().await.unwrap();

        // Status should be Connected
        let status = client.get_status().await;
        match status {
            ClientStatus::Connected(_) => {
                // Test passed
            }
            _ => panic!("Expected Connected status"),
        }

        // Stop the client
        client.stop().await;

        // Status should be Disconnected
        let status = client.get_status().await;
        match status {
            ClientStatus::Disconnected(_) => {
                // Test passed
            }
            _ => panic!("Expected Disconnected status"),
        }
    }

    #[tokio::test]
    async fn test_channel_client_config_builder() {
        let config = ChannelClientConfig::new("http://example.com")
            .adapter_set("TEST_ADAPTER")
            .username("test_user")
            .password("test_pass")
            .subscription_channel_buffer(200)
            .update_channel_buffer(2000);

        assert_eq!(config.server_address, "http://example.com");
        assert_eq!(config.adapter_set, Some("TEST_ADAPTER".to_string()));
        assert_eq!(config.username, Some("test_user".to_string()));
        assert_eq!(config.password, Some("test_pass".to_string()));
        assert_eq!(config.subscription_channel_buffer, 200);
        assert_eq!(config.update_channel_buffer, 2000);
    }

    #[tokio::test]
    async fn test_subscribe_with_updates() {
        let config = ChannelClientConfig::new("http://test.com");
        let client = ChannelBasedClient::new(config).unwrap();
        client.start().await.unwrap();

        let subscription = Subscription::new(
            SubscriptionMode::Merge,
            Some(vec!["test_item".to_string()]),
            Some(vec!["test_field".to_string()]),
        )
        .unwrap();

        let result = client.subscribe_with_updates(subscription, 10).await;
        assert!(result.is_ok());

        let (subscription_id, _receiver) = result.unwrap();
        assert!(subscription_id > 0);

        client.stop().await;
    }

    #[tokio::test]
    async fn test_multiple_concurrent_subscriptions() {
        let config = ChannelClientConfig::new("http://test.com");
        let client = ChannelBasedClient::new(config).unwrap();
        client.start().await.unwrap();

        let mut handles = Vec::new();

        // Create multiple concurrent subscriptions
        for i in 0..5 {
            let subscription_sender = client.get_subscription_sender();
            let handle = tokio::spawn(async move {
                let subscription = Subscription::new(
                    SubscriptionMode::Merge,
                    Some(vec![format!("item_{}", i)]),
                    Some(vec![format!("field_{}", i)]),
                )
                .unwrap();

                let (response_sender, mut response_receiver) = mpsc::channel(1);
                let msg = SubscriptionMessage::Subscribe {
                    subscription,
                    response_sender,
                };

                if let Err(e) = subscription_sender.send(msg).await {
                    Err(format!("Failed to send subscription: {}", e))
                } else {
                    match response_receiver.recv().await {
                        Some(Ok(subscription_id)) => Ok(subscription_id),
                        Some(Err(e)) => Err(e),
                        None => Err("Subscription timed out".to_string()),
                    }
                }
            });
            handles.push(handle);
        }

        // Wait for all subscriptions to complete
        for handle in handles {
            let result = handle.await.unwrap();
            assert!(result.is_ok());
            let subscription_id = result.unwrap();
            assert!(subscription_id > 0);
        }

        client.stop().await;
    }
}
