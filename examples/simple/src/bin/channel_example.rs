use colored::*;
use lightstreamer_rs::client::{LightstreamerClient, Transport};
use lightstreamer_rs::subscription::{
    ChannelSubscriptionListener, ItemUpdate, Snapshot, Subscription, SubscriptionMode,
};
use lightstreamer_rs::utils::{setup_logger, setup_signal_hook};
use std::sync::Arc;
use tokio::sync::{Mutex, Notify};
use tracing::{error, info, warn};

const MAX_CONNECTION_ATTEMPTS: u64 = 1;

/// Example demonstrating channel-based update processing.
///
/// This example shows how to use `ChannelSubscriptionListener` to receive
/// item updates through a tokio channel, enabling asynchronous processing
/// in a separate task.
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    setup_logger();

    info!(
        "{}",
        "🚀 Starting Channel-based Subscription Example"
            .bright_green()
            .bold()
    );
    info!(
        "{}",
        "This example demonstrates asynchronous update processing using channels".bright_cyan()
    );

    // Create a channel for receiving updates
    let (listener, mut update_receiver) = ChannelSubscriptionListener::create_channel();

    info!(
        "{}",
        "✅ Channel created for async update processing".bright_green()
    );

    // Spawn a task to process updates asynchronously
    let processor_handle = tokio::spawn(async move {
        info!("{}", "📡 Update processor task started".bright_cyan());

        let mut update_count = 0u64;
        let mut items_seen = std::collections::HashSet::new();

        while let Some(update) = update_receiver.recv().await {
            update_count += 1;

            // Track unique items
            if let Some(item_name) = &update.item_name {
                items_seen.insert(item_name.clone());
            }

            // Process the update
            process_update(&update, update_count);

            // Log statistics every 10 updates
            if update_count.is_multiple_of(10) {
                info!(
                    "{}",
                    format!(
                        "📊 Stats: {} updates processed, {} unique items",
                        update_count,
                        items_seen.len()
                    )
                    .bright_yellow()
                );
            }
        }

        info!(
            "{}",
            format!(
                "📈 Final Stats: {} total updates, {} unique items",
                update_count,
                items_seen.len()
            )
            .bright_yellow()
            .bold()
        );
    });

    // Create subscription with the channel listener
    let mut subscription = Subscription::new(
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

    subscription.set_data_adapter(Some(String::from("QUOTE_ADAPTER")))?;
    subscription.set_requested_snapshot(Some(Snapshot::Yes))?;
    subscription.add_listener(Box::new(listener));

    info!(
        "{}",
        "✅ Subscription configured with ChannelSubscriptionListener".bright_green()
    );

    // Create Lightstreamer client
    let client = Arc::new(Mutex::new(LightstreamerClient::new(
        Some("http://push.lightstreamer.com/lightstreamer"),
        Some("DEMO"),
        None,
        None,
    )?));

    // Configure client
    {
        let mut client_guard = client.lock().await;
        LightstreamerClient::subscribe(client_guard.subscription_sender.clone(), subscription)
            .await;
        client_guard
            .connection_options
            .set_forced_transport(Some(Transport::WsStreaming));
    }

    info!(
        "{}",
        "✅ Client configured and subscription added".bright_green()
    );

    // Setup shutdown signal
    let shutdown_signal = Arc::new(Notify::new());
    setup_signal_hook(Arc::clone(&shutdown_signal)).await;

    info!(
        "{}",
        "🔌 Connecting to Lightstreamer server...".bright_cyan()
    );

    // Connection loop
    let mut retry_interval_millis: u64 = 0;
    let mut retry_counter: u64 = 0;

    while retry_counter < MAX_CONNECTION_ATTEMPTS {
        match LightstreamerClient::connect(client.clone(), Arc::clone(&shutdown_signal)).await {
            Ok(_) => {
                info!("{}", "🔌 Disconnecting from server...".bright_yellow());
                {
                    let mut client_guard = client.lock().await;
                    client_guard.disconnect().await;
                }
                break;
            }
            Err(e) => {
                error!("❌ Failed to connect: {:?}", e);
                tokio::time::sleep(std::time::Duration::from_millis(retry_interval_millis)).await;
                retry_interval_millis = (retry_interval_millis + (200 * retry_counter)).min(5000);
                retry_counter += 1;
                warn!(
                    "🔄 Retrying connection in {} seconds...",
                    format!("{:.2}", retry_interval_millis as f64 / 1000.0)
                );
            }
        }
    }

    if retry_counter == MAX_CONNECTION_ATTEMPTS {
        error!(
            "❌ Failed to connect after {} retries. Exiting...",
            retry_counter
        );
    } else {
        info!(
            "{}",
            "✅ Exiting orderly from Lightstreamer client..."
                .bright_green()
                .bold()
        );
    }

    // Wait for processor to finish
    info!(
        "{}",
        "⏳ Waiting for update processor to finish...".bright_yellow()
    );
    let _ = processor_handle.await;

    info!(
        "{}",
        "✨ Channel-based subscription example completed!"
            .bright_green()
            .bold()
    );
    info!("{}", "Key features demonstrated:".dimmed());
    info!("{}", "  • Channel-based async update processing".dimmed());
    info!("{}", "  • Decoupled reception and processing".dimmed());
    info!("{}", "  • Non-blocking update handling".dimmed());
    info!("{}", "  • Easy integration with async workflows".dimmed());

    std::process::exit(0);
}

/// Process a single item update.
///
/// # Arguments
///
/// * `update` - The item update to process
/// * `count` - The sequential number of this update
fn process_update(update: &ItemUpdate, count: u64) {
    let not_available = "N/A".to_string();
    let item_name = update
        .item_name
        .clone()
        .unwrap_or_else(|| not_available.clone());

    let fields = vec![
        "stock_name",
        "last_price",
        "time",
        "pct_change",
        "bid",
        "ask",
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

    info!(
        "[{}] {} - {}",
        format!("#{}", count).bright_blue(),
        item_name.bright_cyan(),
        output
    );
}
