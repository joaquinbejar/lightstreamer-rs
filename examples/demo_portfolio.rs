//! Tracks a live portfolio with a **COMMAND-mode** subscription.
//!
//! COMMAND mode is how TLCP models a *set of rows* rather than a fixed list of
//! items. Each update carries a **key**, naming the row, and a **command**
//! saying what happened to it — `ADD`, `UPDATE` or `DELETE`
//! [`docs/spec/04-notifications.md` §3.2]. The client's job is to maintain the
//! table; this example does exactly that and prints it whenever it changes.
//!
//! The feed is the `PORTFOLIO_ADAPTER` Data Adapter of the public demo
//! server's `DEMO` Adapter Set. It simulates a trader buying and selling, so
//! rows appear, change quantity, and disappear.
//!
//! Run it with:
//!
//! ```text
//! cargo run --example demo_portfolio
//! ```

use std::collections::BTreeMap;
use std::process::ExitCode;

use futures_util::StreamExt;
use lightstreamer_rs::{
    AdapterSet, Client, ClientConfig, FieldSchema, ItemCommand, ItemGroup, ServerAddress, Snapshot,
    Subscription, SubscriptionEvent, SubscriptionMode,
};

/// The server the specification's own transcripts use.
const DEMO_SERVER: &str = "https://push.lightstreamer.com";

/// The Adapter Set that serves the demo feeds.
const DEMO_ADAPTER_SET: &str = "DEMO";

/// The Data Adapter that publishes the simulated portfolio.
const PORTFOLIO_ADAPTER: &str = "PORTFOLIO_ADAPTER";

/// The single item: one portfolio, whose rows are the positions held.
const ITEM: &str = "portfolio1";

/// `key` and `command` are mandatory in COMMAND mode and must come first;
/// `qty` is the payload this feed carries.
const FIELDS: [&str; 3] = ["key", "command", "qty"];

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            if let Some(cause) = error.server_error() {
                eprintln!("  server code {}: {}", cause.code(), cause.message());
            }
            ExitCode::FAILURE
        }
    }
}

async fn run() -> lightstreamer_rs::Result<()> {
    let config = ClientConfig::builder(ServerAddress::try_new(DEMO_SERVER)?)
        .with_adapter_set(AdapterSet::try_new(DEMO_ADAPTER_SET)?)
        .build()?;

    println!("connecting to {DEMO_SERVER} …");
    let (client, _session_events) = Client::connect(config).await?;

    let subscription = Subscription::new(
        SubscriptionMode::Command,
        ItemGroup::from_items([ITEM])?,
        FieldSchema::from_fields(FIELDS)?,
    )
    .with_data_adapter(PORTFOLIO_ADAPTER)
    // In COMMAND mode the snapshot is the set of rows that already exist,
    // delivered as a run of `ADD`s [`docs/spec/04-notifications.md` §2.4].
    .with_snapshot(Snapshot::On);

    let mut updates = client.subscribe(subscription).await?;

    // The table this client maintains: key -> quantity.
    let mut positions: BTreeMap<String, String> = BTreeMap::new();

    while let Some(event) = updates.next().await {
        match event {
            SubscriptionEvent::Activated { .. } => {
                println!("subscribed in COMMAND mode; waiting for the snapshot\n");
            }

            SubscriptionEvent::Update(update) => {
                // Both are `None` outside COMMAND mode, which is why they are
                // options rather than plain fields.
                let (Some(key), Some(command)) = (update.key(), update.command()) else {
                    eprintln!("an update arrived without a key or a command; ignoring");
                    continue;
                };
                let Some(key) = key.text() else {
                    eprintln!("a row arrived with a null key; ignoring");
                    continue;
                };

                match &command {
                    ItemCommand::Add | ItemCommand::Update => {
                        // `qty` may legitimately be absent from an UPDATE that
                        // changed nothing else, so keep the previous value.
                        let quantity = update
                            .field_by_name("qty")
                            .and_then(|value| value.text())
                            .map(str::to_owned)
                            .or_else(|| positions.get(key).cloned())
                            .unwrap_or_else(|| "-".to_owned());
                        positions.insert(key.to_owned(), quantity);
                    }
                    ItemCommand::Delete => {
                        positions.remove(key);
                    }
                    other => eprintln!("unrecognized command {other:?} for row {key}"),
                }

                print_table(&command, key, &positions);
            }

            SubscriptionEvent::EndOfSnapshot { .. } => {
                println!("--- end of snapshot: everything below is live ---\n");
            }

            SubscriptionEvent::SnapshotCleared { .. } => {
                // The server says the whole set is void; so must this client.
                println!("the server cleared the snapshot; dropping every row\n");
                positions.clear();
            }

            SubscriptionEvent::Rejected(error) => {
                eprintln!("the server refused the subscription: {error}");
                break;
            }

            SubscriptionEvent::Unsubscribed => break,

            other => println!("{other:?}"),
        }
    }

    client.disconnect().await
}

/// Prints the whole table, and what just happened to it.
fn print_table(command: &ItemCommand, key: &str, positions: &BTreeMap<String, String>) {
    let verb = match command {
        ItemCommand::Add => "added",
        ItemCommand::Update => "updated",
        ItemCommand::Delete => "deleted",
        _ => "changed",
    };
    println!("{verb} {key} — {} positions held", positions.len());
    for (stock, quantity) in positions {
        println!("    {stock:<12} {quantity:>8}");
    }
    println!();
}
