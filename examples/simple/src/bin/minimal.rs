/******************************************************************************
   Author: Joaquín Béjar García
   Email: jb@taunais.com
   Date: 25/10/25
******************************************************************************/

//! Minimal example showing the simplest possible usage.

use lightstreamer_rs::client::{ClientConfig, SimpleClient, SubscriptionParams};
use lightstreamer_rs::subscription::SubscriptionMode;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 1. Configure
    let config =
        ClientConfig::new("http://push.lightstreamer.com/lightstreamer").adapter_set("DEMO");

    // 2. Create client
    let client = SimpleClient::new(config)?;

    // 3. Subscribe - returns a channel receiver automatically
    let params = SubscriptionParams::new(
        SubscriptionMode::Merge,
        vec!["item1".to_string()],
        vec!["last_price".to_string()],
    )
    .data_adapter("QUOTE_ADAPTER");

    let mut updates = client.subscribe(params).await?;

    // 4. Process updates in background
    tokio::spawn(async move {
        while let Some(update) = updates.recv().await {
            println!("Price: {:?}", update.get_value("last_price"));
        }
    });

    // 5. Connect and run
    client.connect().await?;

    Ok(())
}
