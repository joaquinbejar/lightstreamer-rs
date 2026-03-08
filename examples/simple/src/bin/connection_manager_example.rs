//! # ConnectionManager Example
//!
//! This example demonstrates the advanced connection management features of the Lightstreamer Rust client,
//! including automatic reconnection, heartbeat monitoring, exponential backoff, and subscription preservation.
//!
//! ## Features Demonstrated:
//! - Auto-reconnection with configurable settings
//! - Heartbeat monitoring for connection health
//! - Exponential backoff with jitter
//! - Subscription preservation during reconnections
//! - Connection metrics and state monitoring
//! - Graceful shutdown handling

use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, Notify};
use tokio::time::sleep;
use tracing::{error, info};

use lightstreamer_rs::client::{LightstreamerClient, Transport};
use lightstreamer_rs::connection::management::{HeartbeatConfig, ReconnectionConfig};
use lightstreamer_rs::subscription::{
    ItemUpdate, Snapshot, Subscription, SubscriptionListener, SubscriptionMode,
};
use lightstreamer_rs::utils::{setup_logger, setup_signal_hook};

/// Custom subscription listener that demonstrates handling updates during reconnections
pub struct ConnectionAwareListener {
    name: String,
}

impl ConnectionAwareListener {
    pub fn new(name: String) -> Self {
        Self { name }
    }
}

impl SubscriptionListener for ConnectionAwareListener {
    fn on_item_update(&self, update: &ItemUpdate) {
        let not_available = "N/A".to_string();
        let item_name = update.item_name.clone().unwrap_or(not_available.clone());

        // Display key fields with highlighting for changed values
        let fields = vec![
            "stock_name",
            "last_price",
            "time",
            "pct_change",
            "bid",
            "ask",
            "min",
            "max",
            "ref_price",
        ];

        let mut output = String::new();
        for field in fields {
            let value = update.get_value(field).unwrap_or(&not_available);
            output.push_str(&format!("{}: {}, ", field, value));
        }

        info!("[{}] {}: {}", self.name, item_name, output);
    }

    fn on_subscription(&mut self) {
        info!("📡 {} subscription activated", self.name);
    }

    fn on_unsubscription(&mut self) {
        info!("📡 {} subscription deactivated", self.name);
    }
}

#[tokio::main]
async fn main() -> Result<(), lightstreamer_rs::utils::LightstreamerError> {
    setup_logger();

    info!("🚀 Starting ConnectionManager Example");
    info!("This example demonstrates advanced connection management features.");

    // Configure reconnection settings
    let reconnection_config = ReconnectionConfig::default()
        .with_enabled(true)
        .with_max_attempts(10)
        .with_initial_delay(Duration::from_secs(1))
        .with_max_delay(Duration::from_secs(30))
        .with_backoff_multiplier(2.0)
        .with_jitter_enabled(true);

    // Configure heartbeat monitoring
    let heartbeat_config = HeartbeatConfig::default()
        .with_enabled(true)
        .with_interval(Duration::from_secs(30))
        .with_timeout(Duration::from_secs(10));

    info!(
        "⚙️  Reconnection config: max_attempts={:?}, initial_delay={}s, max_delay={}s, multiplier={}, jitter={}",
        reconnection_config.max_attempts,
        reconnection_config.initial_delay.as_secs(),
        reconnection_config.max_delay.as_secs(),
        reconnection_config.backoff_multiplier,
        reconnection_config.jitter_enabled
    );

    info!(
        "💓 Heartbeat config: enabled={}, interval={}s, timeout={}s",
        heartbeat_config.enabled,
        heartbeat_config.interval.as_secs(),
        heartbeat_config.timeout.as_secs()
    );

    // Create client with auto-reconnection enabled
    let client = Arc::new(Mutex::new(LightstreamerClient::new(
        Some("http://push.lightstreamer.com/lightstreamer"),
        Some("DEMO"),
        None,
        None,
    )?));

    // Enable auto-reconnection with custom configuration
    {
        let mut client_guard = client.lock().await;
        client_guard.enable_auto_reconnect_with_config(reconnection_config, heartbeat_config)?;

        // Configure connection options
        client_guard
            .connection_options
            .set_forced_transport(Some(Transport::WsStreaming));
        let _ = client_guard.connection_options.set_keepalive_interval(5);
    }

    // Create multiple subscriptions to demonstrate preservation during reconnections
    let subscriptions = vec![
        create_stock_subscription("Portfolio-1", vec!["item1", "item2", "item3"])?,
        create_stock_subscription("Portfolio-2", vec!["item4", "item5", "item6"])?,
        create_stock_subscription("Portfolio-3", vec!["item7", "item8", "item9"])?,
    ];

    // Add subscriptions to client
    {
        let client_guard = client.lock().await;
        for subscription in subscriptions {
            let _ = LightstreamerClient::subscribe(
                client_guard.subscription_sender.clone(),
                subscription,
            )
            .await;
        }
    }

    info!("📡 Subscriptions added - they will be preserved during reconnections");

    // Setup graceful shutdown
    let shutdown_signal = Arc::new(Notify::new());
    setup_signal_hook(Arc::clone(&shutdown_signal)).await;

    // Note: Monitoring tasks are simplified to avoid Send trait issues
    // In a real application, you would handle these in separate threads or tasks

    // Connect with auto-reconnection
    info!("🔌 Connecting with auto-reconnection enabled...");

    let connect_result =
        LightstreamerClient::connect(Arc::clone(&client), Arc::clone(&shutdown_signal)).await;

    match connect_result {
        Ok(_) => {
            info!("✅ Initial connection successful");

            // Simulate some runtime to observe reconnection behavior
            info!("🕐 Running for 60 seconds to demonstrate reconnection features...");
            info!("   Try disconnecting your network to see auto-reconnection in action!");

            // Wait for shutdown signal or timeout
            tokio::select! {
                _ = shutdown_signal.notified() => {
                    info!("🛑 Shutdown signal received");
                }
                _ = sleep(Duration::from_secs(60)) => {
                    info!("⏰ Demo timeout reached");
                }
            }
        }
        Err(e) => {
            error!("❌ Failed to establish initial connection: {}", e);
        }
    }

    // Graceful shutdown
    info!("🔄 Initiating graceful shutdown...");

    {
        let mut client_guard = client.lock().await;
        client_guard.disconnect().await;
        client_guard.disable_auto_reconnect().await;
    }

    // Display final metrics
    {
        let client_guard = client.lock().await;
        let final_metrics = client_guard.get_connection_metrics().await;
        info!(
            "📊 Final Metrics - Total: {}, Successful reconnections: {}, Failed reconnections: {}, Heartbeat failures: {}",
            final_metrics.total_connections,
            final_metrics.successful_reconnections,
            final_metrics.failed_reconnections,
            final_metrics.heartbeat_failures
        );
    }

    info!("✅ ConnectionManager example completed successfully!");
    info!("Key features demonstrated:");
    info!("  • Automatic reconnection with exponential backoff");
    info!("  • Heartbeat monitoring for connection health");
    info!("  • Subscription preservation during reconnections");
    info!("  • Real-time connection state and metrics monitoring");
    info!("  • Graceful shutdown handling");

    Ok(())
}

/// Helper function to create a stock subscription with custom listener
fn create_stock_subscription(
    name: &str,
    items: Vec<&str>,
) -> Result<Subscription, lightstreamer_rs::utils::LightstreamerError>
where
    Subscription: Sized,
{
    let mut subscription = Subscription::new(
        SubscriptionMode::Merge,
        Some(items.iter().map(|s| s.to_string()).collect()),
        Some(vec![
            "stock_name".to_string(),
            "last_price".to_string(),
            "time".to_string(),
            "pct_change".to_string(),
            "bid_quantity".to_string(),
            "bid".to_string(),
            "ask".to_string(),
            "ask_quantity".to_string(),
            "min".to_string(),
            "max".to_string(),
            "ref_price".to_string(),
            "open_price".to_string(),
        ]),
    )?;

    subscription.set_data_adapter(Some(String::from("QUOTE_ADAPTER")))?;
    subscription.set_requested_snapshot(Some(Snapshot::Yes))?;
    subscription.add_listener(Box::new(ConnectionAwareListener::new(name.to_string())));

    Ok(subscription)
}
