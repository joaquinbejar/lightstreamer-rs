/******************************************************************************
   Author: Joaquín Béjar García
   Email: jb@taunais.com
   Date: 25/10/25
******************************************************************************/

//! Simplified builder API for creating Lightstreamer clients.
//!
//! This module provides a high-level, ergonomic API for creating and configuring
//! Lightstreamer clients with minimal boilerplate.

use crate::client::{LightstreamerClient, Transport};
use crate::subscription::{
    ChannelSubscriptionListener, ItemUpdate, Snapshot, Subscription, SubscriptionMode,
};
use std::sync::Arc;
use tokio::sync::{Mutex, Notify, mpsc};

/// Configuration for a Lightstreamer client.
///
/// This struct provides a simple way to configure all aspects of a Lightstreamer
/// connection with sensible defaults.
#[derive(Debug, Clone)]
pub struct ClientConfig {
    /// Server address (e.g., "<http://push.lightstreamer.com/lightstreamer>")
    pub server_address: String,
    /// Adapter set name (e.g., "DEMO")
    pub adapter_set: Option<String>,
    /// Username for authentication
    pub username: Option<String>,
    /// Password for authentication
    pub password: Option<String>,
    /// Transport type to use
    pub transport: Option<Transport>,
    /// Keepalive interval in milliseconds
    pub keepalive_interval: Option<u64>,
    /// Idle timeout in milliseconds
    pub idle_timeout: Option<u64>,
    /// Reconnect timeout in milliseconds
    pub reconnect_timeout: Option<u64>,
}

impl ClientConfig {
    /// Creates a new configuration with the specified server address.
    ///
    /// # Arguments
    ///
    /// * `server_address` - The Lightstreamer server URL
    ///
    /// # Returns
    ///
    /// A new `ClientConfig` with default values
    pub fn new(server_address: impl Into<String>) -> Self {
        Self {
            server_address: server_address.into(),
            adapter_set: None,
            username: None,
            password: None,
            transport: Some(Transport::WsStreaming),
            keepalive_interval: None,
            idle_timeout: None,
            reconnect_timeout: None,
        }
    }

    /// Sets the adapter set name.
    #[must_use]
    pub fn adapter_set(mut self, adapter_set: impl Into<String>) -> Self {
        self.adapter_set = Some(adapter_set.into());
        self
    }

    /// Sets the username for authentication.
    #[must_use]
    pub fn username(mut self, username: impl Into<String>) -> Self {
        self.username = Some(username.into());
        self
    }

    /// Sets the password for authentication.
    #[must_use]
    pub fn password(mut self, password: impl Into<String>) -> Self {
        self.password = Some(password.into());
        self
    }

    /// Sets the transport type.
    #[must_use]
    pub fn transport(mut self, transport: Transport) -> Self {
        self.transport = Some(transport);
        self
    }

    /// Sets the keepalive interval in milliseconds.
    #[must_use]
    pub fn keepalive_interval(mut self, interval: u64) -> Self {
        self.keepalive_interval = Some(interval);
        self
    }

    /// Sets the idle timeout in milliseconds.
    #[must_use]
    pub fn idle_timeout(mut self, timeout: u64) -> Self {
        self.idle_timeout = Some(timeout);
        self
    }

    /// Sets the reconnect timeout in milliseconds.
    #[must_use]
    pub fn reconnect_timeout(mut self, timeout: u64) -> Self {
        self.reconnect_timeout = Some(timeout);
        self
    }
}

/// Subscription parameters for simplified subscription creation.
#[derive(Debug, Clone)]
pub struct SubscriptionParams {
    /// Subscription mode
    pub mode: SubscriptionMode,
    /// Items to subscribe to
    pub items: Vec<String>,
    /// Fields to subscribe to
    pub fields: Vec<String>,
    /// Data adapter name
    pub data_adapter: Option<String>,
    /// Snapshot preference
    pub snapshot: Option<Snapshot>,
}

impl SubscriptionParams {
    /// Creates new subscription parameters.
    ///
    /// # Arguments
    ///
    /// * `mode` - The subscription mode (MERGE, DISTINCT, COMMAND, RAW)
    /// * `items` - List of items to subscribe to
    /// * `fields` - List of fields to retrieve
    ///
    /// # Returns
    ///
    /// A new `SubscriptionParams` instance
    pub fn new(mode: SubscriptionMode, items: Vec<String>, fields: Vec<String>) -> Self {
        Self {
            mode,
            items,
            fields,
            data_adapter: None,
            snapshot: Some(Snapshot::Yes),
        }
    }

    /// Sets the data adapter.
    #[must_use]
    pub fn data_adapter(mut self, adapter: impl Into<String>) -> Self {
        self.data_adapter = Some(adapter.into());
        self
    }

    /// Sets the snapshot preference.
    #[must_use]
    pub fn snapshot(mut self, snapshot: Snapshot) -> Self {
        self.snapshot = Some(snapshot);
        self
    }
}

/// Simplified Lightstreamer client with high-level API.
///
/// This provides an easy-to-use interface for common Lightstreamer operations.
#[derive(Clone)]
pub struct SimpleClient {
    client: Arc<Mutex<LightstreamerClient>>,
    shutdown_signal: Arc<Notify>,
}

impl SimpleClient {
    /// Creates a new simple client with the given configuration.
    ///
    /// # Arguments
    ///
    /// * `config` - Client configuration
    ///
    /// # Returns
    ///
    /// A new `SimpleClient` instance or an error
    ///
    /// # Errors
    ///
    /// Returns an error if the client cannot be created
    pub fn new(config: ClientConfig) -> Result<Self, Box<dyn std::error::Error>> {
        let mut client = LightstreamerClient::new(
            Some(&config.server_address),
            config.adapter_set.as_deref(),
            config.username.as_deref(),
            config.password.as_deref(),
        )?;

        // Apply configuration
        if let Some(transport) = config.transport {
            client
                .connection_options
                .set_forced_transport(Some(transport));
        }

        if let Some(interval) = config.keepalive_interval {
            client
                .connection_options
                .set_keepalive_interval(interval)
                .map_err(|e| format!("Failed to set keepalive interval: {}", e))?;
        }

        if let Some(timeout) = config.idle_timeout {
            client
                .connection_options
                .set_idle_timeout(timeout)
                .map_err(|e| format!("Failed to set idle timeout: {}", e))?;
        }

        if let Some(timeout) = config.reconnect_timeout {
            client
                .connection_options
                .set_reconnect_timeout(timeout)
                .map_err(|e| format!("Failed to set reconnect timeout: {}", e))?;
        }

        Ok(Self {
            client: Arc::new(Mutex::new(client)),
            shutdown_signal: Arc::new(Notify::new()),
        })
    }

    /// Subscribes to items and returns a channel receiver for updates.
    ///
    /// # Arguments
    ///
    /// * `params` - Subscription parameters
    ///
    /// # Returns
    ///
    /// A receiver for `ItemUpdate` events
    ///
    /// # Errors
    ///
    /// Returns an error if the subscription cannot be created
    pub async fn subscribe(
        &self,
        params: SubscriptionParams,
    ) -> Result<mpsc::UnboundedReceiver<ItemUpdate>, Box<dyn std::error::Error>> {
        let mut subscription =
            Subscription::new(params.mode, Some(params.items), Some(params.fields))?;

        if let Some(adapter) = params.data_adapter {
            subscription.set_data_adapter(Some(adapter))?;
        }

        if let Some(snapshot) = params.snapshot {
            subscription.set_requested_snapshot(Some(snapshot))?;
        }

        // Create channel listener
        let (listener, receiver) = ChannelSubscriptionListener::create_channel();
        subscription.add_listener(Box::new(listener));

        // Add subscription to client
        let client_guard = self.client.lock().await;
        LightstreamerClient::subscribe(client_guard.subscription_sender.clone(), subscription)
            .await;

        Ok(receiver)
    }

    /// Connects to the Lightstreamer server.
    ///
    /// # Returns
    ///
    /// A future that resolves when the connection is established
    ///
    /// # Errors
    ///
    /// Returns an error if the connection fails
    pub async fn connect(&self) -> Result<(), Box<dyn std::error::Error>> {
        LightstreamerClient::connect(self.client.clone(), self.shutdown_signal.clone()).await
    }

    /// Disconnects from the Lightstreamer server.
    pub async fn disconnect(&self) {
        let mut client_guard = self.client.lock().await;
        client_guard.disconnect().await;
    }

    /// Gets a clone of the shutdown signal.
    ///
    /// This can be used to trigger shutdown from external code.
    pub fn shutdown_signal(&self) -> Arc<Notify> {
        self.shutdown_signal.clone()
    }

    /// Triggers a shutdown of the client.
    pub fn shutdown(&self) {
        self.shutdown_signal.notify_waiters();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_client_config_builder() {
        let config = ClientConfig::new("http://localhost:8080")
            .adapter_set("DEMO")
            .username("user")
            .password("pass")
            .transport(Transport::WsStreaming)
            .keepalive_interval(5000)
            .idle_timeout(120000)
            .reconnect_timeout(3000);

        assert_eq!(config.server_address, "http://localhost:8080");
        assert_eq!(config.adapter_set, Some("DEMO".to_string()));
        assert_eq!(config.username, Some("user".to_string()));
        assert_eq!(config.password, Some("pass".to_string()));
        assert_eq!(config.transport, Some(Transport::WsStreaming));
        assert_eq!(config.keepalive_interval, Some(5000));
        assert_eq!(config.idle_timeout, Some(120000));
        assert_eq!(config.reconnect_timeout, Some(3000));
    }

    #[test]
    fn test_subscription_params_builder() {
        let params = SubscriptionParams::new(
            SubscriptionMode::Merge,
            vec!["item1".to_string()],
            vec!["field1".to_string()],
        )
        .data_adapter("QUOTE_ADAPTER")
        .snapshot(Snapshot::Yes);

        assert_eq!(params.mode, SubscriptionMode::Merge);
        assert_eq!(params.items, vec!["item1".to_string()]);
        assert_eq!(params.fields, vec!["field1".to_string()]);
        assert_eq!(params.data_adapter, Some("QUOTE_ADAPTER".to_string()));
        assert!(matches!(params.snapshot, Some(Snapshot::Yes)));
    }
}
