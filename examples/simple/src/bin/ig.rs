use lightstreamer_rs::client::{LightstreamerClient, Transport};
use lightstreamer_rs::subscription::{
    ItemUpdate, Snapshot, Subscription, SubscriptionListener, SubscriptionMode,
};
use lightstreamer_rs::utils::{setup_logger, setup_signal_hook};
use std::sync::Arc;
use tokio::sync::{Mutex, Notify};
use tracing::{error, info, warn};

const MAX_CONNECTION_ATTEMPTS: u64 = 1;

pub struct MySubscriptionListener {}

impl SubscriptionListener for MySubscriptionListener {
    fn on_item_update(&self, update: &ItemUpdate) {
        let item = serde_json::to_string_pretty(&update).unwrap();
        info!("ITEM: {}", item)
    }
}

pub struct Config {
    pub cst: String,
    pub x_security_token: String,
    pub account_id: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    setup_logger();
    //
    // Create a new subscription instance.
    //
    let mut my_subscription = Subscription::new(
        SubscriptionMode::Merge,
        Some(vec![
            // "MARKET:OP.D.OTCDAX1.021100P.IP".to_string(),
            "ACCOUNT:ZR24W".to_string(),
        ]),
        Some(
            // vec!["BID".to_string(), "OFFER".to_string()]
            vec!["PNL".to_string()],
        ),
    )?;

    my_subscription.set_data_adapter(None)?;
    my_subscription.set_requested_snapshot(Some(Snapshot::Yes))?;
    my_subscription.add_listener(Box::new(MySubscriptionListener {}));

    let config = Config {
        cst: std::env::var("CST")?,
        x_security_token: std::env::var("X_SECURITY_TOKEN")?,
        account_id: std::env::var("IG_ACCOUNT_ID")?,
    };

    let account_id = Some(config.account_id.as_str());

    // Create a new Lightstreamer client instance and wrap it in an Arc<Mutex<>> so it can be shared across threads.
    let client = Arc::new(Mutex::new(LightstreamerClient::new(
        Some("https://demo-apd.marketdatasystems.com/lightstreamer"),
        None,
        account_id,
        Some(&format!(
            "CST-{}|XST-{}",
            config.cst, config.x_security_token
        )),
    )?));

    //
    // Add the subscription to the client.
    //
    {
        let mut client = client.lock().await;
        LightstreamerClient::subscribe(client.subscription_sender.clone(), my_subscription).await;
        client
            .connection_options
            .set_forced_transport(Some(Transport::WsStreaming));
        client
            .connection_options
            .set_reconnect_timeout(3000)
            .expect("Failed to set reconnect timeout");
        client
            .connection_options
            .set_keepalive_interval(30000)
            .expect("Failed to set keepalive interval");
        client
            .connection_options
            .set_idle_timeout(120000)
            .expect("Failed to set idle timeout");
    }

    // Create a new Notify instance to send a shutdown signal to the signal handler thread.
    let shutdown_signal = Arc::new(Notify::new());
    // Spawn a new thread to handle SIGINT and SIGTERM process signals.
    setup_signal_hook(Arc::clone(&shutdown_signal)).await;

    //
    // Infinite loop that will indefinitely retry failed connections unless
    // a SIGTERM or SIGINT signal is received.
    //
    let mut retry_interval_milis: u64 = 0;
    let mut retry_counter: u64 = 0;
    while retry_counter < MAX_CONNECTION_ATTEMPTS {
        let mut client = client.lock().await;
        match client.connect_direct(Arc::clone(&shutdown_signal)).await {
            Ok(_) => {
                client.disconnect().await;
                break;
            }
            Err(e) => {
                error!("Failed to connect: {:?}", e);
                tokio::time::sleep(std::time::Duration::from_millis(retry_interval_milis)).await;
                retry_interval_milis = (retry_interval_milis + (200 * retry_counter)).min(5000);
                retry_counter += 1;
                warn!(
                    "Retrying connection in {} seconds...",
                    format!("{:.2}", retry_interval_milis as f64 / 1000.0)
                );
            }
        }
    }

    if retry_counter == MAX_CONNECTION_ATTEMPTS {
        error!(
            "Failed to connect after {} retries. Exiting...",
            retry_counter
        );
    } else {
        info!("Exiting orderly from Lightstreamer client...");
    }

    // Exit using std::process::exit() to avoid waiting for existing tokio tasks to complete.
    std::process::exit(0);
}
