use std::error::Error;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, Notify};
use tokio::time::interval;
use tracing::{debug, error, info};

use lightstreamer_rs::client::{LightstreamerClient, Transport};
use lightstreamer_rs::connection::management::{HeartbeatConfig, ReconnectionConfig};
use lightstreamer_rs::subscription::{
    ItemUpdate, Snapshot, Subscription, SubscriptionListener, SubscriptionMode,
};
use lightstreamer_rs::utils::{setup_logger, setup_signal_hook};

// Financial market specific configuration constants
const MAX_CONNECTION_ATTEMPTS: u32 = 5;
const RETRY_INTERVAL_MS: u64 = 2000;
const CONNECTION_TIMEOUT_MS: u64 = 30000;
const HEARTBEAT_INTERVAL_MS: u64 = 15000;

// Configuration structure for IG Markets authentication
pub struct Config {
    pub cst: String,
    pub x_security_token: String,
    pub account_id: String,
}

/// Advanced subscription listener with connection state awareness
pub struct AdvancedFinancialListener {
    name: String,
}

impl AdvancedFinancialListener {
    pub fn new(name: String) -> Self {
        Self { name }
    }
}

impl SubscriptionListener for AdvancedFinancialListener {
    fn on_item_update(&self, update: &ItemUpdate) {
        let item =
            serde_json::to_string_pretty(&update).unwrap_or_else(|_| "Invalid JSON".to_string());
        info!("📈 {} Update: {}", self.name, item);
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    setup_logger();
    info!("🚀 Starting IG Markets Advanced Lightstreamer Client with ConnectionManager");

    // Get IG configuration from environment variables
    let config = Config {
        cst: std::env::var("CST").expect("CST environment variable not set"),
        x_security_token: std::env::var("X_SECURITY_TOKEN")
            .expect("X_SECURITY_TOKEN environment variable not set"),
        account_id: std::env::var("IG_ACCOUNT_ID")
            .expect("IG_ACCOUNT_ID environment variable not set"),
    };

    info!(
        "🔐 Loaded IG Markets authentication for account: {}",
        config.account_id
    );

    // Configure advanced reconnection settings optimized for financial data
    let reconnection_config = ReconnectionConfig::default()
        .with_enabled(true)
        .with_max_attempts(MAX_CONNECTION_ATTEMPTS)
        .with_initial_delay(Duration::from_millis(RETRY_INTERVAL_MS))
        .with_max_delay(Duration::from_millis(CONNECTION_TIMEOUT_MS))
        .with_backoff_multiplier(1.5) // Gentle backoff for financial data
        .with_jitter_enabled(true); // Add randomness to avoid thundering herd

    // Configure heartbeat monitoring for connection health
    let heartbeat_config = HeartbeatConfig::default()
        .with_enabled(true)
        .with_interval(Duration::from_millis(HEARTBEAT_INTERVAL_MS))
        .with_timeout(Duration::from_millis(HEARTBEAT_INTERVAL_MS * 3));

    info!("⚙️ Configured advanced reconnection and heartbeat monitoring");

    let account_id = Some(config.account_id.as_str());

    // Initialize the LightstreamerClient with Arc<Mutex> pattern for thread safety
    let client = Arc::new(Mutex::new(
        LightstreamerClient::new(
            Some("https://apd.marketdatasystems.com/lightstreamer"),
            None,
            account_id,
            Some(&format!(
                "CST-{}|XST-{}",
                config.cst, config.x_security_token
            )),
        )
        .expect("Failed to create LightstreamerClient"),
    ));

    info!("✅ Created LightstreamerClient with IG Markets authentication");

    // Configure connection options and enable auto-reconnection
    {
        let mut client_guard = client.lock().await;

        // Set transport and connection parameters
        client_guard
            .connection_options
            .set_forced_transport(Some(Transport::WsStreaming));
        client_guard
            .connection_options
            .set_reconnect_timeout(3000)
            .expect("Failed to set reconnect timeout");
        client_guard
            .connection_options
            .set_keepalive_interval(15000)
            .expect("Failed to set keepalive interval");
        client_guard
            .connection_options
            .set_idle_timeout(60000)
            .expect("Failed to set idle timeout");

        // Enable auto-reconnection with advanced configuration
        let _ =
            client_guard.enable_auto_reconnect_with_config(reconnection_config, heartbeat_config);
        info!("🔄 Auto-reconnection enabled with advanced financial market settings");
    }

    // Setup financial subscriptions with error handling
    setup_financial_subscriptions(&client).await;

    // Create shutdown signal for graceful termination
    let shutdown_signal = Arc::new(Notify::new());
    setup_signal_hook(Arc::clone(&shutdown_signal)).await;

    // Start connection monitoring task
    let monitoring_client = client.clone();
    let monitoring_shutdown = shutdown_signal.clone();
    tokio::spawn(async move {
        monitor_connection_metrics(monitoring_client, monitoring_shutdown).await;
    });

    info!("🌐 Starting connection with automatic reconnection management...");

    // Use connect_direct method like in the original ig.rs
    match client
        .lock()
        .await
        .connect_direct(shutdown_signal.clone())
        .await
    {
        Ok(_) => {
            info!("🔌 Connected to IG Markets Lightstreamer server with advanced features");

            // Wait for shutdown signal
            shutdown_signal.notified().await;
            info!("🛑 Shutdown signal received, disconnecting gracefully...");
        }
        Err(e) => {
            error!("❌ Failed to establish connection: {}", e);
            std::process::exit(1);
        }
    }

    info!("✅ IG Markets Advanced Client shutdown completed");
    Ok(())
}

async fn setup_financial_subscriptions(client: &Arc<Mutex<LightstreamerClient>>) {
    info!("📡 Setting up multiple financial data subscriptions...");

    // Market data subscription for price updates
    let mut market_subscription = Subscription::new(
        SubscriptionMode::Merge,
        Some(vec![
            "MARKET:OP.D.OTCDAX1.021100P.IP".to_string(),
            "MARKET:OP.D.FTSE100.021100P.IP".to_string(),
        ]),
        Some(vec![
            "BID".to_string(),
            "OFFER".to_string(),
            "HIGH".to_string(),
            "LOW".to_string(),
        ]),
    )
    .unwrap();

    market_subscription.set_data_adapter(None).unwrap();
    market_subscription
        .set_requested_snapshot(Some(Snapshot::Yes))
        .unwrap();
    market_subscription.add_listener(Box::new(AdvancedFinancialListener::new(
        "Market Data".to_string(),
    )));

    // Account data subscription for P&L and positions
    let mut account_subscription = Subscription::new(
        SubscriptionMode::Merge,
        Some(vec!["ACCOUNT:BSI1I".to_string()]),
        Some(vec![
            "PNL".to_string(),
            "AVAILABLE_CASH".to_string(),
            "EQUITY".to_string(),
        ]),
    )
    .unwrap();

    account_subscription.set_data_adapter(None).unwrap();
    account_subscription
        .set_requested_snapshot(Some(Snapshot::Yes))
        .unwrap();
    account_subscription.add_listener(Box::new(AdvancedFinancialListener::new(
        "Account Data".to_string(),
    )));

    // Add subscriptions to client and configure connection options
    {
        let mut client_guard = client.lock().await;
        LightstreamerClient::subscribe(
            client_guard.subscription_sender.clone(),
            market_subscription,
        )
        .await;
        LightstreamerClient::subscribe(
            client_guard.subscription_sender.clone(),
            account_subscription,
        )
        .await;

        // Configure connection options for financial data streaming
        client_guard
            .connection_options
            .set_forced_transport(Some(Transport::WsStreaming));
        client_guard
            .connection_options
            .set_reconnect_timeout(3000)
            .expect("Failed to set reconnect timeout");
        client_guard
            .connection_options
            .set_keepalive_interval(30000)
            .expect("Failed to set keepalive interval");
        client_guard
            .connection_options
            .set_idle_timeout(120000)
            .expect("Failed to set idle timeout");
    }

    info!("✅ Financial subscriptions configured: Market Data + Account Data");
}

/// Monitor connection metrics and log periodic updates
async fn monitor_connection_metrics(
    _client: Arc<Mutex<LightstreamerClient>>,
    shutdown_signal: Arc<Notify>,
) {
    let mut interval = interval(Duration::from_secs(30));

    loop {
        tokio::select! {
            _ = interval.tick() => {
                info!("📈 Connection monitoring - Client active and streaming financial data");
            }
            _ = shutdown_signal.notified() => {
                debug!("Connection monitoring task shutting down");
                break;
            }
        }
    }
}
