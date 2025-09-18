
use colored::*;
use std::error::Error;
use std::sync::Arc;
use tokio::sync::{Mutex, Notify};
use tracing::info;
use lightstreamer_rs::subscription::{ItemUpdate, Snapshot, Subscription, SubscriptionListener, SubscriptionMode};
use lightstreamer_rs::client::{LightstreamerClient, Transport};
use lightstreamer_rs::utils::{setup_logger, setup_signal_hook};
const MAX_CONNECTION_ATTEMPTS: u64 = 1;

pub struct MySubscriptionListener {}

impl SubscriptionListener for MySubscriptionListener {
    fn on_item_update(&self, update: &ItemUpdate) {
        let not_available = "N/A".to_string();
        let item_name = update.item_name.clone().unwrap_or(not_available.clone());
        let fields = vec![
            "stock_name",
            "last_price",
            "time",
            "pct_change",
            "bid_quantity",
            "bid",
            "ask",
            "ask_quantity",
            "min",
            "max",
            "ref_price",
            "open_price",
        ];
        let mut output = String::new();
        for field in fields {
            let value = update.get_value(field).unwrap_or(&not_available);
            let value_str = if update.changed_fields.contains_key(field) {
                value.yellow().to_string()
            } else {
                value.to_string()
            };
            output.push_str(&format!("{}: {}, ", field, value_str));
        }
        info!("{}, {}", item_name, output);
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    setup_logger();
    //
    // Create a new subscription instance.
    //
    let mut my_subscription = Subscription::new(
        SubscriptionMode::Merge,
        Some(vec![
            "item1".to_string(),
            "item2".to_string(),
            "item3".to_string(),
            "item4".to_string(),
            "item5".to_string(),
            "item6".to_string(),
            "item7".to_string(),
            "item8".to_string(),
            "item9".to_string(),
            "item10".to_string(),
        ]),
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

    my_subscription.set_data_adapter(Some(String::from("QUOTE_ADAPTER")))?;
    my_subscription.set_requested_snapshot(Some(Snapshot::Yes))?;
    my_subscription.add_listener(Box::new(MySubscriptionListener {}));

    // Create a new Lightstreamer client instance and wrap it in an Arc<Mutex<>> so it can be shared across threads.
    let client = Arc::new(Mutex::new(LightstreamerClient::new(
        Some("http://push.lightstreamer.com/lightstreamer"),
        Some("DEMO"),
        None,
        None,
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
        match LightstreamerClient::connect(client.clone(), Arc::clone(&shutdown_signal)).await {
            Ok(_) => {
                {
                    let mut client_guard = client.lock().await;
                    client_guard.disconnect().await;
                }
                break;
            }
            Err(e) => {
                info!("Failed to connect: {:?}", e);
                tokio::time::sleep(std::time::Duration::from_millis(retry_interval_milis)).await;
                retry_interval_milis = (retry_interval_milis + (200 * retry_counter)).min(5000);
                retry_counter += 1;
                info!(
                    "Retrying connection in {} seconds...",
                    format!("{:.2}", retry_interval_milis as f64 / 1000.0)
                );
            }
        }
    }

    if retry_counter == MAX_CONNECTION_ATTEMPTS {
        info!(
            "Failed to connect after {} retries. Exiting...",
            retry_counter
        );
    } else {
        info!("Exiting orderly from Lightstreamer client...");
    }

    // Exit using std::process::exit() to avoid waiting for existing tokio tasks to complete.
    std::process::exit(0);
}
