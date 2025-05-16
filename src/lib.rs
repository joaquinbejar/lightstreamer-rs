//! # Lightstreamer Rust Client
//!
//! This project is a partial implementation of the Lightstreamer TLCP (Text-based Live Connections Protocol) in Rust. It provides a client SDK to interact with Lightstreamer servers, focused on supporting the specific needs of the [ig_trading_api](https://github.com/joaquinbejar/ig_trading_api) project.
//!
//! ## Features
//!
//! - Full-duplex WebSocket-based connection mode.
//! - Subscriptions to items and item groups.
//! - MERGE subscription mode.
//! - Listening to connection events and messages.
//! - Configuration of connection options and connection details.
//! - Subscription lifecycle management.
//! - Retrieval of real-time item updates.
//!
//! Please note that this SDK currently does not support all the features and capabilities of the full Lightstreamer protocol. It has been developed to cover the requirements of the ig_trading_api project mentioned above. Features like other connection modes, subscription modes (DISTINCT, RAW, COMMAND), and some other advanced options are not implemented at this time.
//!
//! ## Installation
//!
//! To use this SDK in your Rust project, add the following dependency to your `Cargo.toml`:
//!
//! ```toml
//! [dependencies]
//! lightstreamer-rs = "0.1.0"
//! ```
//!
//! ## Usage
//!
//! Here's a comprehensive example of how to use the Lightstreamer Rust Client SDK:
//!
//! ```ignore
//! // This example shows how to use the Lightstreamer Rust client
//! use lightstreamer_rs::client::{LightstreamerClient, Transport};
//! use lightstreamer_rs::subscription::{Subscription, SubscriptionMode, SubscriptionListener, ItemUpdate};
//! use std::sync::Arc;
//! use tokio::sync::Notify;
//! use std::time::Duration;
//!
//! // Define a custom subscription listener
//! struct MySubscriptionListener;
//!
//! impl SubscriptionListener for MySubscriptionListener {
//!     fn on_subscription(&self) {
//!         println!("Subscription confirmed by the server");
//!     }
//!     
//!     fn on_item_update(&self, update: ItemUpdate) {
//!         println!("Received update for item: {}", update.get_item_name());
//!         for field in update.get_fields() {
//!             if let Some(value) = update.get_value(field) {
//!                 println!("  {} = {}", field, value);
//!             }
//!         }
//!     }
//! }
//!
//! async fn example() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
//!     // Create a new LightstreamerClient instance
//!     let result = LightstreamerClient::new(
//!         Some("ws://your-lightstreamer-server.com"),  // Server address
//!         Some("YOUR_ADAPTER_SET"),                   // Adapter set
//!         None,                                       // User (optional)
//!         None,                                       // Password (optional)
//!     );
//!     
//!     let mut client = match result {
//!         Ok(client) => client,
//!         Err(e) => return Err(e),
//!     };
//!     
//!     // Configure the connection details if needed
//!     client.connection_details.set_user("YOUR_USERNAME");
//!     client.connection_details.set_password("YOUR_PASSWORD");
//!     
//!     // Configure connection options if needed
//!     client.connection_options.set_content_length(50000000);
//!     client.connection_options.set_keepalive_interval(5);
//!     client.connection_options.set_forced_transport(Some(Transport::WsStreaming));
//!     
//!     // Create a shutdown signal for graceful termination
//!     let shutdown_signal = Arc::new(Notify::new());
//!     
//!     // Connect to the Lightstreamer server
//!     if let Err(e) = client.connect(shutdown_signal.clone()).await {
//!         return Err(e);
//!     }
//!     
//!     // Create a subscription
//!     let subscription_result = Subscription::new(
//!         SubscriptionMode::Merge,
//!         Some(vec!["item1".to_string(), "item2".to_string()]),
//!         Some(vec!["field1".to_string(), "field2".to_string()]),
//!     );
//!     
//!     let subscription = match subscription_result {
//!         Ok(sub) => sub,
//!         Err(e) => return Err(e),
//!     };
//!     
//!     // Add a listener to the subscription (optional)
//!     let listener = Box::new(MySubscriptionListener);
//!     subscription.add_listener(listener);
//!     
//!     // Get the subscription sender from the client
//!     // Note: This method might not exist in the current API, check the documentation
//!     let subscription_sender = client.get_subscriptions()[0].clone();
//!     
//!     // Subscribe and get the subscription ID
//!     let subscription_id_result = LightstreamerClient::subscribe_get_id(
//!         subscription_sender.clone(),
//!         subscription
//!     ).await;
//!     
//!     let subscription_id = match subscription_id_result {
//!         Ok(id) => id,
//!         Err(e) => return Err(e),
//!     };
//!     
//!     println!("Subscribed with ID: {}", subscription_id);
//!     
//!     // Wait for some time (in a real application, you would wait for a shutdown signal)
//!     tokio::time::sleep(Duration::from_secs(5)).await;
//!     
//!     // Unsubscribe before disconnecting
//!     LightstreamerClient::unsubscribe(subscription_sender, subscription_id).await;
//!     
//!     Ok(())
//! }
//! ```
//!
//! ### Handling Client Events
//!
//! You can also add listeners to handle client events:
//!
//! Here's an example of implementing a client listener:
//!
//! ```ignore
//! // Note: ClientListener is a private trait, this is just for illustration
//! use lightstreamer_rs::client::model::ClientStatus;
//!
//! struct MyClientListener;
//!
//! // This is just an example of what the ClientListener trait might look like
//! // The actual implementation is internal to the library
//! trait ClientListener {
//!     fn on_status_change(&self, status: &ClientStatus);
//!     fn on_server_error(&self, error_code: i32, error_message: &str);
//!     fn on_property_change(&self, property: &str);
//! }
//!
//! impl ClientListener for MyClientListener {
//!     fn on_status_change(&self, status: &ClientStatus) {
//!         println!("Client status changed to: {:?}", status);
//!     }
//!     
//!     fn on_server_error(&self, error_code: i32, error_message: &str) {
//!         println!("Server error: {} - {}", error_code, error_message);
//!     }
//!     
//!     fn on_property_change(&self, property: &str) {
//!         println!("Property changed: {}", property);
//!     }
//! }
//!
//! // Then add the listener to your client
//! // client.add_listener(Box::new(MyClientListener));
//! ```
//!

/// Module containing subscription-related functionality.
///
/// This module provides the necessary types and functions to create and manage subscriptions
/// to Lightstreamer items. It includes the `Subscription` struct, subscription modes,
/// item updates, and subscription listeners.
pub mod subscription;

/// Module containing utility functions and error types.
///
/// This module provides common utilities and error types used throughout the library,
/// including exception types for handling illegal arguments and states.
pub mod utils;

/// Module containing client-related functionality.
///
/// This module provides the main `LightstreamerClient` type and related components for
/// connecting to Lightstreamer servers, managing sessions, and handling client events.
pub mod client;

/// Module containing connection-related functionality.
///
/// This module provides types for managing connection details and options.
mod connection;
