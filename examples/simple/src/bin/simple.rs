use colored::*;
use lightstreamer_rs::client::{ChannelBasedClient, ChannelClientConfig, SubscriptionUpdate};
use lightstreamer_rs::subscription::{Subscription, SubscriptionMode};
use lightstreamer_rs::utils::{setup_logger, setup_signal_hook};
use std::sync::Arc;
use tokio::sync::Notify;
use tokio::sync::mpsc;
use tokio::time::{Duration, sleep};
use tracing::info;

/// Helper function to format and display item updates with colored output
fn display_update(update: &SubscriptionUpdate) {
    let not_available = "N/A".to_string();
    let item_name = update
        .item_update
        .item_name
        .clone()
        .unwrap_or(not_available.clone());

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
        let value = update
            .item_update
            .get_value(field)
            .unwrap_or(&not_available);
        let value_str = if update.item_update.changed_fields.contains_key(field) {
            value.yellow().to_string()
        } else {
            value.to_string()
        };
        output.push_str(&format!("{}: {}, ", field, value_str));
    }
    info!("{}, {}", item_name, output);
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    setup_logger();

    info!("🚀 Starting Simple ChannelBasedClient Example");

    // Create channel-based client configuration
    let config = ChannelClientConfig::new("http://push.lightstreamer.com/lightstreamer")
        .adapter_set("DEMO")
        .subscription_channel_buffer(100)
        .update_channel_buffer(1000);

    // Create the channel-based client
    let client = ChannelBasedClient::new(config)?;

    // Start the client
    client.start().await?;

    // Wait a moment for the client to start
    sleep(Duration::from_millis(100)).await;

    // Create subscription for multiple stock items
    let subscription = Subscription::new(
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

    // Subscribe and get update receiver
    let (subscription_id, mut receiver) = client.subscribe_with_updates(subscription, 1000).await?;
    info!("📈 Subscription created with ID: {}", subscription_id);

    // Spawn update processor task
    let processor_handle = tokio::spawn(async move {
        let mut update_count = 0;
        loop {
            tokio::select! {
                // Check for updates
                update = receiver.recv() => {
                    match update {
                        Some(subscription_update) => {
                            update_count += 1;
                            display_update(&subscription_update);
                        }
                        None => {
                            info!("Update channel closed");
                            break;
                        }
                    }
                }
                // Timeout to prevent infinite blocking
                _ = sleep(Duration::from_millis(100)) => {
                    // Continue
                }
            }
        }
        info!("Update processor finished after {} updates", update_count);
    });

    // Spawn subscription manager task for dynamic operations
    let subscription_sender = client.get_subscription_sender();
    let manager_handle = tokio::spawn(async move {
        subscription_manager_task(subscription_sender).await;
    });

    // Create a new Notify instance to send a shutdown signal to the signal handler thread.
    let shutdown_signal = Arc::new(Notify::new());
    // Spawn a new thread to handle SIGINT and SIGTERM process signals.
    setup_signal_hook(shutdown_signal.clone()).await;

    info!("⏱️ Running example... Press Ctrl+C to stop");

    // Wait for shutdown signal
    shutdown_signal.notified().await;

    info!("🛑 Shutting down...");

    // Stop the client
    client.stop().await;

    // Wait for all tasks to finish
    let _ = tokio::join!(processor_handle, manager_handle);

    info!("✅ Shutdown complete");
    Ok(())
}

/// Subscription manager that handles dynamic subscription management
async fn subscription_manager_task(
    subscription_sender: mpsc::Sender<lightstreamer_rs::client::SubscriptionMessage>,
) {
    info!("🎛️ Starting subscription manager task");

    // Simulate dynamic subscription management
    for i in 1..=3 {
        sleep(Duration::from_secs(5)).await;

        info!("🎛️ Manager cycle #{}", i);

        // Example of sending a subscription request directly
        if i == 2 {
            let subscription = Subscription::new(
                SubscriptionMode::Merge,
                Some(vec!["dynamic_item".to_string()]),
                Some(vec!["dynamic_field".to_string()]),
            )
            .unwrap();

            let (response_sender, mut response_receiver) = mpsc::channel(1);
            let msg = lightstreamer_rs::client::SubscriptionMessage::Subscribe {
                subscription,
                response_sender,
            };

            if let Err(e) = subscription_sender.send(msg).await {
                info!("🎛️ Failed to send dynamic subscription: {}", e);
            } else {
                match response_receiver.recv().await {
                    Some(Ok(subscription_id)) => {
                        info!(
                            "🎛️ Dynamic subscription created with ID: {}",
                            subscription_id
                        );
                    }
                    Some(Err(e)) => {
                        info!("🎛️ Dynamic subscription failed: {}", e);
                    }
                    None => {
                        info!("🎛️ Dynamic subscription timed out");
                    }
                }
            }
        }
    }

    info!("🎛️ Subscription manager task finished");
}
