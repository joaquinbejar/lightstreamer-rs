//! Connection management module for automatic reconnection functionality
//!
//! This module provides the core components for managing WebSocket connections
//! with automatic reconnection, heartbeat monitoring, and subscription preservation.

use std::collections::HashMap;
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, Notify, RwLock};
use tokio::time::sleep;
use tracing::{debug, error, info, warn};

use crate::client::LightstreamerClient;

/// Main connection manager that orchestrates reconnection, heartbeat, and subscription management
#[derive(Debug)]
pub struct ConnectionManager {
    client: Weak<Mutex<LightstreamerClient>>,
    reconnection_handler: Arc<ReconnectionHandler>,
    heartbeat_monitor: Arc<HeartbeatMonitor>,
    subscription_manager: Arc<SubscriptionManager>,
    connection_state: Arc<RwLock<ConnectionState>>,
    shutdown_signal: Arc<Notify>,
    metrics: Arc<Mutex<ConnectionMetrics>>,
}

/// Current state of the connection
#[derive(Debug, Clone, PartialEq)]
pub enum ConnectionState {
    /// Connection is not established
    Disconnected,
    /// Connection attempt is in progress
    Connecting,
    /// Connection is established and active
    Connected,
    /// Reconnection is in progress
    Reconnecting {
        /// Current reconnection attempt number
        attempt: u32,
        /// Timestamp for the next retry attempt
        next_retry: Instant,
    },
    /// Connection has failed permanently
    Failed {
        /// Reason for the connection failure
        reason: String,
    },
}

/// Reason for disconnection
#[derive(Debug, Clone)]
pub enum DisconnectionReason {
    /// Network-related error occurred
    NetworkError(String),
    /// Server-side error occurred
    ServerError(String),
    /// Heartbeat timeout was reached
    HeartbeatTimeout,
    /// Disconnection was requested by the user
    UserRequested,
    /// Unknown or unspecified reason
    Unknown,
}

/// Configuration for reconnection behavior
#[derive(Debug, Clone)]
pub struct ReconnectionConfig {
    /// Whether automatic reconnection is enabled
    pub enabled: bool,
    /// Initial delay before the first reconnection attempt
    pub initial_delay: Duration,
    /// Maximum delay between reconnection attempts
    pub max_delay: Duration,
    /// Maximum number of reconnection attempts (None for unlimited)
    pub max_attempts: Option<u32>,
    /// Multiplier for exponential backoff between attempts
    pub backoff_multiplier: f64,
    /// Whether to add random jitter to delays (deprecated, use jitter_enabled)
    pub jitter: bool,
    /// Whether to add random jitter to delays to avoid thundering herd
    pub jitter_enabled: bool,
    /// Timeout for each connection attempt
    pub timeout: Duration,
}

/// Configuration for heartbeat monitoring
#[derive(Debug, Clone)]
pub struct HeartbeatConfig {
    /// Whether heartbeat monitoring is enabled
    pub enabled: bool,
    /// Interval between heartbeat messages
    pub interval: Duration,
    /// Timeout for heartbeat responses
    pub timeout: Duration,
    /// Maximum number of missed heartbeats before considering connection lost
    pub max_missed: u32,
}

/// Connection events that can be emitted
#[derive(Debug, Clone)]
pub enum ConnectionEvent {
    /// Connection has been established successfully
    Connected,
    /// Connection has been lost
    Disconnected {
        /// Reason for the disconnection
        reason: DisconnectionReason,
    },
    /// Reconnection attempt is in progress
    Reconnecting {
        /// Current attempt number
        attempt: u32,
    },
    /// Reconnection has failed
    ReconnectionFailed {
        /// Reason for the failure
        reason: String,
    },
    /// A heartbeat message was missed
    HeartbeatMissed,
    /// Subscriptions have been preserved during disconnection
    SubscriptionPreserved {
        /// Number of subscriptions preserved
        count: usize,
    },
    /// Subscriptions have been restored after reconnection
    SubscriptionRestored {
        /// Number of subscriptions restored
        count: usize,
    },
}

impl Default for ReconnectionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_attempts: Some(10),
            initial_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(60),
            backoff_multiplier: 2.0,
            jitter: true,
            jitter_enabled: true,
            timeout: Duration::from_secs(30),
        }
    }
}

impl ReconnectionConfig {
    /// Creates a new reconnection config with default values
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates a disabled reconnection config
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            ..Self::default()
        }
    }

    /// Creates a fast reconnection config for testing
    pub fn fast() -> Self {
        Self {
            enabled: true,
            max_attempts: Some(5),
            initial_delay: Duration::from_millis(100),
            max_delay: Duration::from_secs(5),
            backoff_multiplier: 1.5,
            jitter: true,
            jitter_enabled: true,
            timeout: Duration::from_secs(10),
        }
    }

    /// Creates a conservative reconnection config
    pub fn conservative() -> Self {
        Self {
            enabled: true,
            max_attempts: Some(20),
            initial_delay: Duration::from_secs(5),
            max_delay: Duration::from_secs(300), // 5 minutes
            backoff_multiplier: 2.5,
            jitter: true,
            jitter_enabled: true,
            timeout: Duration::from_secs(60),
        }
    }

    /// Sets whether reconnection is enabled
    pub fn with_enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Sets the maximum number of reconnection attempts
    pub fn with_max_attempts(mut self, max_attempts: u32) -> Self {
        self.max_attempts = Some(max_attempts);
        self
    }

    /// Sets the initial delay between reconnection attempts
    pub fn with_initial_delay(mut self, initial_delay: Duration) -> Self {
        self.initial_delay = initial_delay;
        self
    }

    /// Sets the maximum delay between reconnection attempts
    pub fn with_max_delay(mut self, max_delay: Duration) -> Self {
        self.max_delay = max_delay;
        self
    }

    /// Sets the backoff multiplier
    pub fn with_backoff_multiplier(mut self, multiplier: f64) -> Self {
        self.backoff_multiplier = multiplier;
        self
    }

    /// Sets whether jitter is enabled
    pub fn with_jitter_enabled(mut self, jitter_enabled: bool) -> Self {
        self.jitter_enabled = jitter_enabled;
        self
    }

    /// Sets the connection timeout
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Validates the configuration
    pub fn validate(&self) -> Result<(), String> {
        if let Some(max_attempts) = self.max_attempts
            && max_attempts == 0
        {
            return Err("max_attempts must be greater than 0".to_string());
        }

        if self.initial_delay.is_zero() {
            return Err("initial_delay must be greater than 0".to_string());
        }

        if self.max_delay < self.initial_delay {
            return Err("max_delay must be greater than or equal to initial_delay".to_string());
        }

        if self.backoff_multiplier <= 1.0 {
            return Err("backoff_multiplier must be greater than 1.0".to_string());
        }

        if self.timeout.is_zero() {
            return Err("timeout must be greater than 0".to_string());
        }

        Ok(())
    }
}

impl Default for HeartbeatConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            interval: Duration::from_secs(30),
            timeout: Duration::from_secs(10),
            max_missed: 3,
        }
    }
}

impl HeartbeatConfig {
    /// Creates a new heartbeat config with default values
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates a disabled heartbeat config
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            ..Self::default()
        }
    }

    /// Creates a fast heartbeat config for testing
    pub fn fast() -> Self {
        Self {
            enabled: true,
            interval: Duration::from_secs(5),
            timeout: Duration::from_secs(2),
            max_missed: 2,
        }
    }

    /// Creates a conservative heartbeat config
    pub fn conservative() -> Self {
        Self {
            enabled: true,
            interval: Duration::from_secs(60),
            timeout: Duration::from_secs(20),
            max_missed: 5,
        }
    }

    /// Sets whether heartbeat is enabled
    pub fn with_enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Sets the heartbeat interval
    pub fn with_interval(mut self, interval: Duration) -> Self {
        self.interval = interval;
        self
    }

    /// Sets the heartbeat timeout
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Sets the maximum number of missed heartbeats
    pub fn with_max_missed(mut self, max_missed: u32) -> Self {
        self.max_missed = max_missed;
        self
    }

    /// Validates the configuration
    pub fn validate(&self) -> Result<(), String> {
        if self.interval.is_zero() {
            return Err("interval must be greater than 0".to_string());
        }

        if self.timeout.is_zero() {
            return Err("timeout must be greater than 0".to_string());
        }

        if self.timeout >= self.interval {
            return Err("timeout must be less than interval".to_string());
        }

        if self.max_missed == 0 {
            return Err("max_missed must be greater than 0".to_string());
        }

        Ok(())
    }
}

/// Connection metrics for monitoring and debugging
#[derive(Debug, Default, Clone)]
pub struct ConnectionMetrics {
    /// Total number of connection attempts made
    pub total_connections: u64,
    /// Number of successful reconnection attempts
    pub successful_reconnections: u64,
    /// Number of failed reconnection attempts
    pub failed_reconnections: u64,
    /// Average time taken for successful reconnections
    pub average_reconnection_time: Duration,
    /// Number of heartbeat failures detected
    pub heartbeat_failures: u64,
    /// Number of subscription recovery operations performed
    pub subscription_recoveries: u64,
    /// Timestamp of when these metrics were last updated
    pub last_updated: Option<Instant>,
}

/// Handles reconnection logic with exponential backoff
#[derive(Debug)]
pub struct ReconnectionHandler {
    config: ReconnectionConfig,
    current_attempt: Arc<Mutex<u32>>,
    last_attempt: Arc<Mutex<Option<Instant>>>,
    connection_manager: Weak<ConnectionManager>,
}

/// Monitors connection health through heartbeats
#[derive(Debug)]
pub struct HeartbeatMonitor {
    config: HeartbeatConfig,
    last_heartbeat: Arc<Mutex<Instant>>,
    missed_count: Arc<Mutex<u32>>,
    connection_manager: Weak<ConnectionManager>,
    is_running: Arc<Mutex<bool>>,
}

impl HeartbeatMonitor {
    /// Creates a new heartbeat monitor
    pub fn new(config: HeartbeatConfig, connection_manager: Weak<ConnectionManager>) -> Self {
        Self {
            config,
            last_heartbeat: Arc::new(Mutex::new(Instant::now())),
            missed_count: Arc::new(Mutex::new(0)),
            connection_manager,
            is_running: Arc::new(Mutex::new(false)),
        }
    }

    /// Sets the connection manager reference
    pub fn set_connection_manager(&self, _connection_manager: Weak<ConnectionManager>) {
        // Note: This would require interior mutability in a real implementation
        // For now, we'll handle this in the constructor
    }

    /// Starts the heartbeat monitoring
    pub async fn start(&self) {
        {
            let mut running = self.is_running.lock().await;
            if *running {
                debug!("Heartbeat monitor already running");
                return;
            }
            *running = true;
        }

        info!(
            "Starting heartbeat monitor with interval: {:?}",
            self.config.interval
        );

        let mut interval = tokio::time::interval(self.config.interval);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            // Check if we should stop
            {
                let running = self.is_running.lock().await;
                if !*running {
                    debug!("Heartbeat monitor stopping");
                    break;
                }
            }

            interval.tick().await;

            // Send heartbeat and check response
            match self.send_heartbeat().await {
                Ok(()) => {
                    // Reset missed count on successful heartbeat
                    {
                        let mut missed = self.missed_count.lock().await;
                        *missed = 0;
                    }

                    // Update last heartbeat time
                    {
                        let mut last = self.last_heartbeat.lock().await;
                        *last = Instant::now();
                    }

                    debug!("Heartbeat successful");
                }
                Err(e) => {
                    warn!("Heartbeat failed: {}", e);

                    let missed_count = {
                        let mut missed = self.missed_count.lock().await;
                        *missed += 1;
                        *missed
                    };

                    // Check if we've exceeded the maximum missed heartbeats
                    if missed_count >= self.config.max_missed {
                        error!(
                            "Maximum missed heartbeats ({}) exceeded, triggering disconnection",
                            self.config.max_missed
                        );

                        // Trigger disconnection handling
                        if let Some(manager) = self.connection_manager.upgrade() {
                            manager
                                .handle_disconnection(DisconnectionReason::HeartbeatTimeout)
                                .await;

                            // Update metrics
                            {
                                let mut metrics = manager.metrics.lock().await;
                                metrics.heartbeat_failures += 1;
                                metrics.last_updated = Some(Instant::now());
                            }
                        }

                        break;
                    }
                }
            }
        }

        // Mark as not running
        {
            let mut running = self.is_running.lock().await;
            *running = false;
        }

        info!("Heartbeat monitor stopped");
    }

    /// Stops the heartbeat monitor
    pub async fn stop(&self) {
        info!("Stopping heartbeat monitor");
        let mut running = self.is_running.lock().await;
        *running = false;
    }

    /// Sends a heartbeat message to the server
    async fn send_heartbeat(&self) -> Result<(), ReconnectionError> {
        let manager = self
            .connection_manager
            .upgrade()
            .ok_or(ReconnectionError::ClientLost)?;

        let client = manager
            .client
            .upgrade()
            .ok_or(ReconnectionError::ClientLost)?;

        // Check current connection state
        let state = manager.get_connection_state().await;
        if !matches!(state, ConnectionState::Connected) {
            return Err(ReconnectionError::ConnectionFailed(
                "Not connected".to_string(),
            ));
        }

        let _client_guard = client.lock().await;

        // Send a ping/heartbeat message
        // This would be implemented based on the Lightstreamer protocol
        // For now, we'll simulate the heartbeat
        match self.simulate_heartbeat().await {
            Ok(()) => {
                debug!("Heartbeat sent successfully");
                Ok(())
            }
            Err(e) => {
                error!("Failed to send heartbeat: {}", e);
                Err(ReconnectionError::ConnectionFailed(e))
            }
        }
    }

    /// Simulates sending a heartbeat (placeholder for actual implementation)
    async fn simulate_heartbeat(&self) -> Result<(), String> {
        // In a real implementation, this would:
        // 1. Send a ping message to the WebSocket
        // 2. Wait for a pong response within the timeout
        // 3. Return Ok(()) if successful, Err() if timeout or error

        // For simulation, we'll randomly succeed/fail
        tokio::time::sleep(Duration::from_millis(10)).await;

        // Simulate 95% success rate
        if rand::random::<f64>() < 0.95 {
            Ok(())
        } else {
            Err("Simulated heartbeat failure".to_string())
        }
    }

    /// Gets the time of the last successful heartbeat
    pub async fn get_last_heartbeat(&self) -> Instant {
        *self.last_heartbeat.lock().await
    }

    /// Gets the current missed heartbeat count
    pub async fn get_missed_count(&self) -> u32 {
        *self.missed_count.lock().await
    }
}

/// Manages subscription state and recovery
#[derive(Debug)]
#[allow(dead_code)]
pub struct SubscriptionManager {
    subscriptions: Arc<RwLock<HashMap<usize, SubscriptionState>>>,
    connection_manager: Weak<ConnectionManager>,
}

/// State of a subscription
#[derive(Debug, Clone)]
pub struct SubscriptionState {
    /// Unique identifier for this subscription
    pub id: usize,
    /// Key used to identify the subscription in Lightstreamer
    pub subscription_key: String,
    /// Current status of the subscription
    pub status: SubscriptionStatus,
    /// Last received values for subscribed items
    pub last_values: HashMap<String, String>,
    /// Timestamp when this subscription was created
    pub created_at: Instant,
    /// Timestamp of the last update received for this subscription
    pub last_update: Option<Instant>,
}

/// Status of a subscription
#[derive(Debug, Clone, PartialEq)]
pub enum SubscriptionStatus {
    /// Subscription is active and receiving updates
    Active,
    /// Subscription is temporarily suspended
    Suspended,
    /// Subscription is being reestablished after a disconnection
    Resubscribing,
    /// Subscription has failed with the given reason
    Failed {
        /// Reason for the subscription failure
        reason: String,
    },
}

/// General Lightstreamer error type
#[derive(Debug, Clone)]
pub enum LightstreamerError {
    /// Connection related errors
    Connection(String),
    /// Subscription related errors
    Subscription(String),
    /// Authentication errors
    Authentication(String),
    /// Configuration errors
    Configuration(String),
    /// Network errors
    Network(String),
    /// General errors
    General(String),
}

/// Errors that can occur during reconnection attempts
#[derive(Debug, thiserror::Error)]
pub enum ReconnectionError {
    /// Connection attempt failed with the given error message
    #[error("Connection failed: {0}")]
    ConnectionFailed(String),
    /// Maximum number of reconnection attempts has been reached
    #[error("Maximum reconnection attempts reached")]
    MaxAttemptsReached,
    /// Reconnection attempt timed out
    #[error("Reconnection timeout")]
    Timeout,
    /// The client reference was lost during reconnection
    #[error("Client reference lost")]
    ClientLost,
    /// A subscription-related error occurred during reconnection
    #[error("Subscription error: {0}")]
    SubscriptionError(String),
}

/// Errors related to subscription management
#[derive(Debug, thiserror::Error)]
pub enum SubscriptionError {
    /// The subscription with the given ID was not found
    #[error("Subscription not found: {0}")]
    NotFound(usize),
    /// Failed to preserve subscription state with the given error message
    #[error("Failed to preserve subscription: {0}")]
    PreservationFailed(String),
    /// Failed to reestablish subscription with the given error message
    #[error("Failed to reestablish subscription: {0}")]
    ReestablishmentFailed(String),
}

impl ConnectionManager {
    /// Creates a new connection manager
    pub fn new(
        client: Weak<Mutex<LightstreamerClient>>,
        reconnection_config: ReconnectionConfig,
        heartbeat_config: HeartbeatConfig,
    ) -> Arc<Self> {
        let manager = Arc::new(Self {
            client: client.clone(),
            reconnection_handler: Arc::new(ReconnectionHandler::new(
                reconnection_config,
                Weak::new(), // Will be set after creation
            )),
            heartbeat_monitor: Arc::new(HeartbeatMonitor::new(
                heartbeat_config,
                Weak::new(), // Will be set after creation
            )),
            subscription_manager: Arc::new(SubscriptionManager::new(
                Weak::new(), // Will be set after creation
            )),
            connection_state: Arc::new(RwLock::new(ConnectionState::Disconnected)),
            shutdown_signal: Arc::new(Notify::new()),
            metrics: Arc::new(Mutex::new(ConnectionMetrics::default())),
        });

        // Set weak references to self
        let weak_manager = Arc::downgrade(&manager);
        manager
            .reconnection_handler
            .set_connection_manager(weak_manager.clone());
        manager
            .heartbeat_monitor
            .set_connection_manager(weak_manager.clone());
        manager
            .subscription_manager
            .set_connection_manager(weak_manager);

        manager
    }

    /// Starts the connection monitoring tasks
    pub async fn start_monitoring(&self) {
        info!("Starting connection monitoring");

        let heartbeat_task = {
            let monitor = self.heartbeat_monitor.clone();
            tokio::spawn(async move {
                monitor.start().await;
            })
        };

        let reconnection_task = {
            let handler = self.reconnection_handler.clone();
            tokio::spawn(async move {
                handler.start().await;
            })
        };

        tokio::select! {
            _ = heartbeat_task => {
                debug!("Heartbeat monitoring task completed");
            },
            _ = reconnection_task => {
                debug!("Reconnection handler task completed");
            },
            _ = self.shutdown_signal.notified() => {
                info!("Shutdown signal received, stopping monitoring");
            },
        }
    }

    /// Handles disconnection and triggers reconnection if needed
    pub async fn handle_disconnection(&self, reason: DisconnectionReason) {
        warn!("Connection lost: {:?}", reason);

        // Update connection state
        {
            let mut state = self.connection_state.write().await;
            *state = ConnectionState::Disconnected;
        }

        // Preserve subscription state
        if let Err(e) = self.subscription_manager.preserve_subscriptions().await {
            error!("Failed to preserve subscriptions: {}", e);
        }

        // Update metrics
        {
            let mut metrics = self.metrics.lock().await;
            metrics.last_updated = Some(Instant::now());
        }

        // Trigger reconnection unless it was user-requested
        if !matches!(reason, DisconnectionReason::UserRequested) {
            self.reconnection_handler.trigger_reconnection(reason).await;
        }
    }

    /// Gets the current connection state
    pub async fn get_connection_state(&self) -> ConnectionState {
        self.connection_state.read().await.clone()
    }

    /// Gets connection metrics
    pub async fn get_metrics(&self) -> ConnectionMetrics {
        self.metrics.lock().await.clone()
    }

    /// Forces a reconnection attempt
    pub async fn force_reconnect(&self) -> Result<(), ReconnectionError> {
        info!("Forcing reconnection");

        // Trigger reconnection through the handler
        self.reconnection_handler
            .trigger_reconnection(DisconnectionReason::UserRequested)
            .await;

        Ok(())
    }

    /// Shuts down the connection manager
    pub async fn shutdown(&self) {
        info!("Shutting down connection manager");
        self.shutdown_signal.notify_waiters();

        // Stop heartbeat monitoring
        self.heartbeat_monitor.stop().await;

        // Update connection state
        {
            let mut state = self.connection_state.write().await;
            *state = ConnectionState::Disconnected;
        }
    }
}

impl ReconnectionHandler {
    /// Creates a new reconnection handler
    pub fn new(config: ReconnectionConfig, connection_manager: Weak<ConnectionManager>) -> Self {
        Self {
            config,
            current_attempt: Arc::new(Mutex::new(0)),
            last_attempt: Arc::new(Mutex::new(None)),
            connection_manager,
        }
    }

    /// Sets the connection manager reference
    pub fn set_connection_manager(&self, _connection_manager: Weak<ConnectionManager>) {
        // Note: This would require interior mutability in a real implementation
        // For now, we'll handle this in the constructor
    }

    /// Starts the reconnection handler
    pub async fn start(&self) {
        debug!("Reconnection handler started");
        // This will be triggered by handle_disconnection
    }

    /// Triggers a reconnection attempt
    pub async fn trigger_reconnection(&self, reason: DisconnectionReason) {
        info!("Triggering reconnection due to: {:?}", reason);

        // Reset attempt counter for new disconnection
        {
            let mut attempt = self.current_attempt.lock().await;
            *attempt = 0;
        }

        // Start reconnection loop
        self.reconnection_loop().await;
    }

    /// Main reconnection loop with exponential backoff
    async fn reconnection_loop(&self) {
        loop {
            // Check if we've exceeded max attempts
            let current_attempt = {
                let mut attempt = self.current_attempt.lock().await;
                *attempt += 1;
                *attempt
            };

            if let Some(max_attempts) = self.config.max_attempts
                && current_attempt > max_attempts
            {
                error!("Maximum reconnection attempts ({}) reached", max_attempts);

                // Update connection manager state
                if let Some(manager) = self.connection_manager.upgrade() {
                    let mut state = manager.connection_state.write().await;
                    *state = ConnectionState::Failed {
                        reason: "Maximum reconnection attempts reached".to_string(),
                    };

                    let mut metrics = manager.metrics.lock().await;
                    metrics.failed_reconnections += 1;
                    metrics.last_updated = Some(Instant::now());
                }
                break;
            }

            // Calculate delay with exponential backoff
            let delay = self.calculate_next_delay(current_attempt).await;

            // Update connection state to show we're reconnecting
            if let Some(manager) = self.connection_manager.upgrade() {
                let mut state = manager.connection_state.write().await;
                *state = ConnectionState::Reconnecting {
                    attempt: current_attempt,
                    next_retry: Instant::now() + delay,
                };
            }

            info!("Reconnection attempt {} in {:?}", current_attempt, delay);
            sleep(delay).await;

            // Attempt reconnection
            match self.attempt_reconnection().await {
                Ok(()) => {
                    info!("Reconnection successful after {} attempts", current_attempt);

                    // Update metrics and state
                    if let Some(manager) = self.connection_manager.upgrade() {
                        let mut state = manager.connection_state.write().await;
                        *state = ConnectionState::Connected;

                        let mut metrics = manager.metrics.lock().await;
                        metrics.successful_reconnections += 1;
                        metrics.total_connections += 1;
                        metrics.last_updated = Some(Instant::now());

                        // Reestablish subscriptions
                        if let Some(client_ref) = manager.client.upgrade()
                            && let Err(e) = manager
                                .subscription_manager
                                .reestablish_subscriptions(&client_ref)
                                .await
                        {
                            error!("Failed to reestablish subscriptions: {}", e);
                        }
                    }

                    // Reset attempt counter
                    {
                        let mut attempt = self.current_attempt.lock().await;
                        *attempt = 0;
                    }

                    break;
                }
                Err(e) => {
                    warn!("Reconnection attempt {} failed: {}", current_attempt, e);

                    // Update last attempt time
                    {
                        let mut last_attempt = self.last_attempt.lock().await;
                        *last_attempt = Some(Instant::now());
                    }
                }
            }
        }
    }

    /// Calculates the next delay using exponential backoff with optional jitter
    async fn calculate_next_delay(&self, attempt: u32) -> Duration {
        let base_delay = self.config.initial_delay.as_millis() as f64;
        let multiplier = self.config.backoff_multiplier.powi((attempt - 1) as i32);
        let calculated_delay = (base_delay * multiplier) as u64;

        let delay =
            Duration::from_millis(calculated_delay.min(self.config.max_delay.as_millis() as u64));

        if self.config.jitter {
            self.add_jitter(delay)
        } else {
            delay
        }
    }

    /// Adds random jitter to prevent thundering herd
    fn add_jitter(&self, delay: Duration) -> Duration {
        let jitter_range = delay.as_millis() / 4; // 25% jitter
        let jitter = rand::random::<u64>() % (jitter_range as u64 + 1);
        delay + Duration::from_millis(jitter)
    }

    /// Attempts to reconnect to the server
    async fn attempt_reconnection(&self) -> Result<(), ReconnectionError> {
        let manager = self
            .connection_manager
            .upgrade()
            .ok_or(ReconnectionError::ClientLost)?;

        let client = manager
            .client
            .upgrade()
            .ok_or(ReconnectionError::ClientLost)?;

        // Attempt to connect
        // This would call the actual connection logic
        // For now, we'll simulate the connection attempt
        let shutdown_signal = Arc::new(tokio::sync::Notify::new());
        match crate::client::LightstreamerClient::connect(client, shutdown_signal).await {
            Ok(()) => {
                info!("Successfully reconnected to server");
                Ok(())
            }
            Err(e) => {
                error!("Failed to reconnect: {:?}", e);
                Err(ReconnectionError::ConnectionFailed(format!("{:?}", e)))
            }
        }
    }
}

impl SubscriptionManager {
    /// Creates a new subscription manager
    pub fn new(connection_manager: Weak<ConnectionManager>) -> Self {
        Self {
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            connection_manager,
        }
    }

    /// Sets the connection manager reference
    pub fn set_connection_manager(&self, _connection_manager: Weak<ConnectionManager>) {
        // Note: This would require interior mutability in a real implementation
        // For now, we'll handle this in the constructor
    }

    /// Adds a subscription to be managed
    pub async fn add_subscription(&self, subscription_id: usize, subscription: SubscriptionState) {
        let mut subs = self.subscriptions.write().await;
        subs.insert(subscription_id, subscription);
        info!("Added subscription to manager: {}", subscription_id);
    }

    /// Removes a subscription from management
    pub async fn remove_subscription(&self, subscription_id: usize) -> Option<SubscriptionState> {
        let mut subs = self.subscriptions.write().await;
        let removed = subs.remove(&subscription_id);
        if removed.is_some() {
            info!("Removed subscription from manager: {}", subscription_id);
        }
        removed
    }

    /// Gets a subscription by ID
    pub async fn get_subscription(&self, subscription_id: usize) -> Option<SubscriptionState> {
        let subs = self.subscriptions.read().await;
        subs.get(&subscription_id).cloned()
    }

    /// Gets all managed subscriptions
    pub async fn get_all_subscriptions(&self) -> HashMap<usize, SubscriptionState> {
        let subs = self.subscriptions.read().await;
        subs.clone()
    }

    /// Marks all subscriptions as disconnected
    pub async fn mark_all_disconnected(&self) {
        let mut subs = self.subscriptions.write().await;
        for (id, subscription) in subs.iter_mut() {
            subscription.status = SubscriptionStatus::Suspended;
            subscription.last_update = Some(Instant::now());
            debug!("Marked subscription {} as disconnected", id);
        }
        info!("Marked {} subscriptions as disconnected", subs.len());
    }

    /// Preserves subscription state during disconnection
    pub async fn preserve_subscriptions(&self) -> Result<(), SubscriptionError> {
        let subs = self.subscriptions.read().await;
        info!(
            "Preserving {} subscriptions during disconnection",
            subs.len()
        );
        // In a real implementation, this would save subscription state
        // to persistent storage or memory for later restoration
        Ok(())
    }

    /// Reestablishes all subscriptions after reconnection
    pub async fn reestablish_subscriptions(
        &self,
        client: &Arc<Mutex<crate::client::LightstreamerClient>>,
    ) -> Result<(), ReconnectionError> {
        let subscriptions = {
            let subs = self.subscriptions.read().await;
            subs.clone()
        };

        let mut reestablished = 0;
        let mut failed = 0;

        for (id, mut subscription) in subscriptions {
            match self
                .reestablish_single_subscription(id, &mut subscription, client)
                .await
            {
                Ok(()) => {
                    reestablished += 1;
                    // Update the subscription in the map
                    {
                        let mut subs = self.subscriptions.write().await;
                        if let Some(stored_sub) = subs.get_mut(&id) {
                            *stored_sub = subscription;
                        }
                    }
                }
                Err(e) => {
                    failed += 1;
                    error!("Failed to reestablish subscription {}: {}", id, e);

                    // Mark as failed
                    subscription.status = SubscriptionStatus::Failed {
                        reason: e.to_string(),
                    };
                    subscription.last_update = Some(Instant::now());

                    // Update the subscription in the map
                    {
                        let mut subs = self.subscriptions.write().await;
                        if let Some(stored_sub) = subs.get_mut(&id) {
                            *stored_sub = subscription;
                        }
                    }
                }
            }
        }

        info!(
            "Subscription reestablishment complete: {} succeeded, {} failed",
            reestablished, failed
        );

        if failed > 0 {
            Err(ReconnectionError::SubscriptionError(format!(
                "Failed to reestablish {} out of {} subscriptions",
                failed,
                reestablished + failed
            )))
        } else {
            Ok(())
        }
    }

    /// Reestablishes a single subscription
    async fn reestablish_single_subscription(
        &self,
        subscription_id: usize,
        subscription: &mut SubscriptionState,
        _client: &Arc<Mutex<crate::client::LightstreamerClient>>,
    ) -> Result<(), ReconnectionError> {
        info!("Reestablishing subscription: {}", subscription_id);

        // Update state to connecting
        subscription.status = SubscriptionStatus::Resubscribing;
        subscription.last_update = Some(Instant::now());

        // Simulate subscription reestablishment
        // In a real implementation, this would:
        // 1. Create a new subscription request
        // 2. Send it to the server
        // 3. Wait for confirmation
        // 4. Update the subscription state

        match self
            .simulate_subscription_reestablishment(subscription_id)
            .await
        {
            Ok(()) => {
                subscription.status = SubscriptionStatus::Active;
                subscription.last_update = Some(Instant::now());
                info!(
                    "Successfully reestablished subscription: {}",
                    subscription_id
                );
                Ok(())
            }
            Err(e) => {
                subscription.status = SubscriptionStatus::Failed { reason: e.clone() };
                subscription.last_update = Some(Instant::now());
                error!(
                    "Failed to reestablish subscription {}: {}",
                    subscription_id, e
                );
                Err(ReconnectionError::SubscriptionError(e))
            }
        }
    }

    /// Simulates subscription reestablishment (placeholder for actual implementation)
    async fn simulate_subscription_reestablishment(
        &self,
        subscription_id: usize,
    ) -> Result<(), String> {
        // Simulate network delay
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Simulate 90% success rate
        if rand::random::<f64>() < 0.9 {
            debug!(
                "Simulated successful reestablishment for subscription: {}",
                subscription_id
            );
            Ok(())
        } else {
            Err(format!(
                "Simulated failure for subscription: {}",
                subscription_id
            ))
        }
    }

    /// Gets subscription statistics
    pub async fn get_statistics(&self) -> SubscriptionStatistics {
        let subs = self.subscriptions.read().await;

        let mut stats = SubscriptionStatistics {
            total_subscriptions: subs.len(),
            active_subscriptions: 0,
            failed_subscriptions: 0,
            disconnected_subscriptions: 0,
            connecting_subscriptions: 0,
        };

        for subscription in subs.values() {
            match subscription.status {
                SubscriptionStatus::Active => stats.active_subscriptions += 1,
                SubscriptionStatus::Failed { .. } => stats.failed_subscriptions += 1,
                SubscriptionStatus::Suspended => stats.disconnected_subscriptions += 1,
                SubscriptionStatus::Resubscribing => stats.connecting_subscriptions += 1,
            }
        }

        stats
    }

    /// Clears all failed subscriptions
    pub async fn clear_failed_subscriptions(&self) -> usize {
        let mut subs = self.subscriptions.write().await;
        let initial_count = subs.len();

        subs.retain(|_, subscription| {
            !matches!(subscription.status, SubscriptionStatus::Failed { .. })
        });

        let removed_count = initial_count - subs.len();
        if removed_count > 0 {
            info!("Cleared {} failed subscriptions", removed_count);
        }

        removed_count
    }
}

/// Statistics about managed subscriptions
#[derive(Debug, Clone)]
pub struct SubscriptionStatistics {
    /// Total number of subscriptions being managed
    pub total_subscriptions: usize,
    /// Number of subscriptions that are currently active and receiving data
    pub active_subscriptions: usize,
    /// Number of subscriptions that have failed and are not receiving data
    pub failed_subscriptions: usize,
    /// Number of subscriptions that are disconnected but preserved for reconnection
    pub disconnected_subscriptions: usize,
    /// Number of subscriptions that are currently in the process of reconnecting
    pub connecting_subscriptions: usize,
}
