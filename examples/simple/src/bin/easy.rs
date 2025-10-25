/******************************************************************************
   Author: Joaquín Béjar García
   Email: jb@taunais.com
   Date: 25/10/25
******************************************************************************/

//! Simplified example demonstrating the easy-to-use API.
//!
//! This example shows how to use the high-level `SimpleClient` API
//! for quick and easy Lightstreamer integration.

use lightstreamer_rs::client::{ClientConfig, SimpleClient, SubscriptionParams};
use lightstreamer_rs::subscription::SubscriptionMode;
use lightstreamer_rs::utils::setup_logger;
use tracing::info;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    setup_logger();

    info!("🚀 Starting Easy Lightstreamer Example");

    // 1. Create configuration
    let config =
        ClientConfig::new("http://push.lightstreamer.com/lightstreamer").adapter_set("DEMO");

    // 2. Create client
    let client = SimpleClient::new(config)?;

    // 3. Subscribe and get channel receiver
    // The channel is created automatically inside subscribe()
    let params = SubscriptionParams::new(
        SubscriptionMode::Merge,
        vec![
            "item1".to_string(),
            "item2".to_string(),
            "item3".to_string(),
        ],
        vec![
            "stock_name".to_string(),
            "last_price".to_string(),
            "time".to_string(),
        ],
    )
    .data_adapter("QUOTE_ADAPTER");

    let mut receiver = client.subscribe(params).await?;

    // 4. Spawn task to process updates
    let processor = tokio::spawn(async move {
        let mut count = 0;
        while let Some(update) = receiver.recv().await {
            count += 1;
            info!(
                "[#{}] Item: {:?}, Price: {:?}",
                count,
                update.get_item_name(),
                update.get_value("last_price")
            );
        }
        info!("Update processor finished");
    });

    // 5. Connect in background
    let client_clone = client.clone();
    let connect_handle = tokio::spawn(async move {
        if let Err(e) = client_clone.connect().await {
            tracing::error!("Connection error: {:?}", e);
        }
    });

    // 6. Wait for shutdown signal
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {
            info!("Received Ctrl+C, shutting down...");
        }
    }

    // 7. Trigger shutdown and cleanup
    info!("Triggering shutdown...");
    client.shutdown();
    client.disconnect().await;

    info!("✅ Shutdown complete");

    // Wait for tasks to finish
    let _ = tokio::time::timeout(std::time::Duration::from_secs(2), connect_handle).await;

    drop(processor);

    Ok(())
}
