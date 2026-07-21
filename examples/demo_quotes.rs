//! Streams live quotes from the public Lightstreamer demo server.
//!
//! The server and Adapter Set are the ones the TLCP specification uses in its
//! own worked transcripts: `push.lightstreamer.com` with `LS_adapter_set=DEMO`
//! [`docs/spec/02-session-lifecycle.md` §10]. The `DEMO` Adapter Set publishes
//! thirty simulated stocks named `item1` … `item30`.
//!
//! Run it with:
//!
//! ```text
//! cargo run --example demo_quotes
//! ```
//!
//! It prints a snapshot of five stocks, then every change until you press
//! Ctrl-C. The session event stream is consumed on a task of its own, which is
//! the shape most programs want: reconnection is automatic, but what a
//! reconnection *meant* is something the application has to see.

use std::process::ExitCode;

use futures_util::StreamExt;
use lightstreamer_rs::{
    AdapterSet, Client, ClientConfig, Continuity, FieldSchema, ItemGroup, ServerAddress,
    SessionEvent, SessionEvents, Snapshot, Subscription, SubscriptionEvent, SubscriptionMode,
    UpdateKind,
};

/// The server the specification's own transcripts use.
const DEMO_SERVER: &str = "https://push.lightstreamer.com";

/// The Adapter Set that serves the simulated stock feed.
const DEMO_ADAPTER_SET: &str = "DEMO";

/// The Data Adapter inside `DEMO` that publishes the simulated stock feed.
const QUOTE_ADAPTER: &str = "QUOTE_ADAPTER";

/// The stocks to watch. The demo adapter publishes `item1` … `item30`.
const ITEMS: [&str; 5] = ["item1", "item2", "item3", "item4", "item5"];

/// The fields to ask for, in the order updates will carry them.
const FIELDS: [&str; 4] = ["stock_name", "last_price", "time", "pct_change"];

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            // A code the server supplied is worth showing on its own: it is
            // what distinguishes "your password is wrong" from "come back
            // later".
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
        // The demo Adapter Set authenticates nobody, which is the default.
        .build()?;

    println!("connecting to {DEMO_SERVER} …");
    let (client, session_events) = Client::connect(config).await?;
    println!("connected");

    // Session events on their own task. Dropping `session_events` instead
    // would be the supported way to say "I do not care" — see `SessionEvents`.
    tokio::spawn(watch_session(session_events));

    let subscription = Subscription::new(
        SubscriptionMode::Merge,
        ItemGroup::from_items(ITEMS)?,
        FieldSchema::from_fields(FIELDS)?,
    )
    // The `DEMO` Adapter Set contains more than one Data Adapter, so the
    // stock feed has to be named: omitting it is answered with control error
    // 17, "Data Adapter not found" [`docs/spec/05-error-codes.md` §3].
    .with_data_adapter(QUOTE_ADAPTER)
    // Start from a complete picture instead of waiting for every field to
    // tick. In MERGE mode the snapshot is the current value of every field.
    .with_snapshot(Snapshot::On);

    let mut updates = client.subscribe(subscription).await?;

    while let Some(event) = updates.next().await {
        match event {
            SubscriptionEvent::Activated {
                item_count,
                field_count,
                ..
            } => {
                println!("subscribed: {item_count} items × {field_count} fields");
            }

            SubscriptionEvent::Update(update) => {
                let marker = match update.kind() {
                    UpdateKind::Snapshot => "snapshot",
                    UpdateKind::RealTime => "live",
                };
                let name = update
                    .field_by_name("stock_name")
                    .and_then(|value| value.text())
                    .unwrap_or(update.item_name());
                let price = update
                    .field_by_name("last_price")
                    .map_or("-", |value| value.text_or("(null)"));
                let change = update
                    .field_by_name("pct_change")
                    .map_or("-", |value| value.text_or("(null)"));

                println!(
                    "[{marker:>8}] {name:<20} {price:>10} {change:>8}%   changed: {:?}",
                    update
                        .changed_fields()
                        .map(|field| field.name())
                        .collect::<Vec<_>>()
                );
            }

            SubscriptionEvent::Overflow {
                item_index,
                dropped_count,
            } => {
                // The *server* discarded updates. This client never does.
                eprintln!("the server dropped {dropped_count} updates for item {item_index}");
            }

            SubscriptionEvent::Rejected(error) => {
                eprintln!("the server refused the subscription: {error}");
                break;
            }

            SubscriptionEvent::Unsubscribed => {
                println!("unsubscribed");
                break;
            }

            other => println!("{other:?}"),
        }
    }

    // Tell the server the session is over rather than just going quiet.
    client.disconnect().await
}

/// Reports what happens to the session underneath the data.
async fn watch_session(mut events: SessionEvents) {
    while let Some(event) = events.next().await {
        match event {
            SessionEvent::Connected(connected) => match &connected.continuity {
                // The id is printed because this is the public demo server. The
                // crate never logs it itself: it is what a control request
                // names the session with, so treat it as a token elsewhere.
                Continuity::New => println!("session {} opened", connected.session_id),
                Continuity::Preserved => println!("reconnected; nothing was lost"),
                Continuity::Recovered { requested_from } => {
                    println!("recovering from notification {requested_from}");
                }
                Continuity::Replaced { .. } => {
                    // The one case where derived state must be thrown away.
                    println!("a NEW session replaced the old one — discard derived state");
                }
                other => println!("connected: {other:?}"),
            },

            SessionEvent::Recovered(recovery) => {
                if recovery.is_lossless() {
                    println!("recovery was lossless");
                } else {
                    println!("recovery lost data: {:?}", recovery.outcome);
                }
            }

            SessionEvent::Disconnected { reason, retry_in } => match retry_in {
                Some(delay) => println!("disconnected ({reason:?}); retrying in {delay:?}"),
                None => println!("disconnected ({reason:?}); giving up"),
            },

            SessionEvent::Closed(reason) => {
                println!("session closed: {reason:?}");
                break;
            }

            other => println!("session: {other:?}"),
        }
    }
}
