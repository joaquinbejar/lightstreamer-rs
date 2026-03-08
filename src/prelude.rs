//! # Prelude Module
//!
//! This module provides convenient re-exports of the most commonly used types
//! and traits from the Lightstreamer client library.
//!
//! By importing the prelude, you can quickly access all essential types:
//!
//! ```ignore
//! use lightstreamer_rs::prelude::*;
//! ```
//!
//! This imports:
//! - Client types: `LightstreamerClient`, `ClientConfig`, `SimpleClient`
//! - Subscription types: `Subscription`, `SubscriptionMode`, `ItemUpdate`
//! - Connection types: `ConnectionOptions`, `ConnectionDetails`
//! - Listener traits: `ClientListener`, `SubscriptionListener`
//! - Error types: `LightstreamerError`, `Result`
//! - Utility types and functions

// Client types
pub use crate::client::{
    ClientConfig, ClientListener, ClientMessageListener, ClientStatus, ConnectionType,
    DisconnectionType, LightstreamerClient, LogType, SimpleClient, SubscriptionParams,
    SubscriptionRequest, Transport,
};

// Subscription types
pub use crate::subscription::{
    ChannelSubscriptionListener, ItemUpdate, Snapshot, Subscription, SubscriptionListener,
    SubscriptionMode,
};

// Connection types
pub use crate::connection::{
    ConnectionDetails, ConnectionEvent, ConnectionManager, ConnectionMetrics, ConnectionOptions,
    ConnectionState, DisconnectionReason, HeartbeatConfig, ReconnectionConfig, ReconnectionError,
    SubscriptionError, SubscriptionManager, SubscriptionState, SubscriptionStatistics,
    SubscriptionStatus,
};

// Error types
pub use crate::utils::{LightstreamerError, Result};

// Utility types and functions
pub use crate::utils::{Proxy, clean_message, parse_arguments, setup_logger, setup_signal_hook};
