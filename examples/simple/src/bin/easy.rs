/******************************************************************************
   Author: Joaquín Béjar García
   Email: jb@taunais.com
   Date: 30/10/25
******************************************************************************/

//! Channel-based client example for multithreaded operations.
//!
//! This example demonstrates how to use the `ChannelBasedClient` API
//! for multithreaded Lightstreamer integration with channels.

use lightstreamer_rs::client::{
    ChannelBasedClient, ChannelClientConfig, SubscriptionMessage, SubscriptionUpdate,
};
use lightstreamer_rs::subscription::{Subscription, SubscriptionMode};
use lightstreamer_rs::utils::setup_logger;
use tokio::sync::mpsc;
use tokio::time::{Duration, sleep};
use tracing::{error, info, warn};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    setup_logger();

    info!("🚀 Starting ChannelBasedClient Multithread Example");

    // 1. Create channel-based client configuration
    let config = ChannelClientConfig::new("http://push.lightstreamer.com/lightstreamer")
        .adapter_set("DEMO")
        .subscription_channel_buffer(100)
        .update_channel_buffer(1000);

    // 2. Create the channel-based client
    let client = ChannelBasedClient::new(config)?;

    // 3. Start the client
    client.start().await?;

    // Wait a moment for the client to start
    sleep(Duration::from_millis(100)).await;

    // 4. Create subscription for stock data
    let subscription = Subscription::new(
        SubscriptionMode::Merge,
        Some(vec![
            "item1".to_string(),
            "item2".to_string(),
            "item3".to_string(),
        ]),
        Some(vec![
            "stock_name".to_string(),
            "last_price".to_string(),
            "time".to_string(),
            "pct_change".to_string(),
        ]),
    )
    .unwrap();

    // 5. Subscribe and get update receiver
    let (subscription_id, mut receiver) = client.subscribe_with_updates(subscription, 100).await?;
    info!("📈 Subscription created with ID: {}", subscription_id);

    // 6. Spawn update processor task
    let _processor_handle = tokio::spawn(async move {
        let mut update_count = 0;
        loop {
            tokio::select! {
                // Check for updates
                update = receiver.recv() => {
                    match update {
                        Some(SubscriptionUpdate { subscription_id: _, item_update }) => {
                            update_count += 1;
                            if let Some(price) = item_update.get_value("last_price")
                                && let Some(name) = item_update.get_value("stock_name") {
                                    info!("📈 UPDATE #{}: {} = ${}", update_count, name, price);
                                }
                        }
                        None => {
                            warn!("Update channel closed");
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

    // 7. Spawn subscription manager task
    let subscription_sender = client.get_subscription_sender();
    let manager_handle = tokio::spawn(async move {
        subscription_manager_task(subscription_sender).await;
    });

    // 8. Run for a while and then shutdown
    info!("⏱️ Running for 10 seconds...");
    sleep(Duration::from_secs(10)).await;

    info!("🛑 Shutting down...");

    // Stop the client
    client.stop().await;

    // Wait for manager task to finish
    let _ = manager_handle.await;

    info!("✅ Shutdown complete");
    Ok(())
}

/// Subscription manager that handles dynamic subscription management
async fn subscription_manager_task(subscription_sender: mpsc::Sender<SubscriptionMessage>) {
    info!("🎛️ Starting subscription manager task");

    // Simulate dynamic subscription management
    for i in 1..=3 {
        sleep(Duration::from_secs(3)).await;

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
            let msg = SubscriptionMessage::Subscribe {
                subscription,
                response_sender,
            };

            if let Err(e) = subscription_sender.send(msg).await {
                error!("🎛️ Failed to send dynamic subscription: {}", e);
            } else {
                match response_receiver.recv().await {
                    Some(Ok(subscription_id)) => {
                        info!(
                            "🎛️ Dynamic subscription created with ID: {}",
                            subscription_id
                        );
                    }
                    Some(Err(e)) => {
                        error!("🎛️ Dynamic subscription failed: {}", e);
                    }
                    None => {
                        warn!("🎛️ Dynamic subscription timed out");
                    }
                }
            }
        }
    }

    info!("🎛️ Subscription manager task finished");
}
