//! Watches what a session does when the network misbehaves.
//!
//! Reconnection is automatic in this crate, but *what a reconnection meant* is
//! not something a client should hide. TLCP distinguishes three outcomes, and
//! an application holding derived state — an order book, a portfolio, a cache
//! — needs to tell them apart
//! [`docs/adr/0005-recovery-is-visible-in-the-event-stream.md`]:
//!
//! - the **same** session came back and no data was missed, so whatever you
//!   computed from it is still valid;
//! - the same session came back but the server could not replay everything, so
//!   there is a **gap**;
//! - a **new** session replaced the old one, so every subscription was
//!   re-executed from scratch and derived state must be discarded.
//!
//! This example prints one line per data update and a loud banner for every
//! session-level event, so the two can be watched together.
//!
//! Run it, then break the network — turn off Wi-Fi for ten seconds, or block
//! the host — and watch what it reports:
//!
//! ```text
//! cargo run --example demo_resilience
//! ```

use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use futures_util::StreamExt;
use lightstreamer_rs::{
    AdapterSet, Client, ClientConfig, Continuity, FieldSchema, ItemGroup, ServerAddress,
    SessionEvent, SessionEvents, Snapshot, Subscription, SubscriptionEvent, SubscriptionMode,
};

/// The server the specification's own transcripts use.
const DEMO_SERVER: &str = "https://push.lightstreamer.com";

/// The Adapter Set that serves the demo feeds.
const DEMO_ADAPTER_SET: &str = "DEMO";

/// The Data Adapter that publishes the simulated stock feed.
const QUOTE_ADAPTER: &str = "QUOTE_ADAPTER";

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
    println!("break the network at any point and watch the banners.\n");

    let (client, session_events) = Client::connect(config).await?;

    // Counting updates is the crudest possible "derived state", and it is
    // enough to make the point: after a Replaced session the count is
    // meaningless and has to start again.
    let updates_seen = Arc::new(AtomicU64::new(0));
    tokio::spawn(watch_session(session_events, Arc::clone(&updates_seen)));

    let subscription = Subscription::new(
        SubscriptionMode::Merge,
        ItemGroup::from_items(["item1", "item2", "item3"])?,
        FieldSchema::from_fields(["stock_name", "last_price", "time"])?,
    )
    .with_data_adapter(QUOTE_ADAPTER)
    .with_snapshot(Snapshot::On);

    let mut updates = client.subscribe(subscription).await?;

    while let Some(event) = updates.next().await {
        match event {
            SubscriptionEvent::Update(update) => {
                let seen = updates_seen.fetch_add(1, Ordering::Relaxed) + 1;
                let price = update
                    .field_by_name("last_price")
                    .map_or("-", |value| value.text_or("(null)"));
                println!("  #{seen:<5} {:<10} {price:>10}", update.item_name());
            }

            SubscriptionEvent::Rejected(error) => {
                eprintln!("the server refused the subscription: {error}");
                break;
            }

            SubscriptionEvent::Unsubscribed => break,

            // Worth showing: after a session replacement the subscription is
            // re-activated, and its snapshot arrives again.
            other => println!("  subscription: {other:?}"),
        }
    }

    client.disconnect().await
}

/// Prints a banner for everything that happens to the session itself.
async fn watch_session(mut events: SessionEvents, updates_seen: Arc<AtomicU64>) {
    while let Some(event) = events.next().await {
        match event {
            SessionEvent::Connected(connected) => match &connected.continuity {
                Continuity::New => {
                    banner(&format!("session {} opened", connected.session_id));
                }
                Continuity::Preserved => {
                    banner("RECONNECTED — same session, nothing missed. State is still valid.");
                }
                Continuity::Recovered { requested_from } => {
                    banner(&format!(
                        "RECOVERED — resuming from notification {requested_from}."
                    ));
                }
                Continuity::Replaced { .. } => {
                    // The one case where derived state is worthless.
                    let discarded = updates_seen.swap(0, Ordering::Relaxed);
                    banner(&format!(
                        "REPLACED — a NEW session. Discarding {discarded} updates' worth \
                         of derived state; subscriptions are being re-executed."
                    ));
                }
                other => banner(&format!("connected: {other:?}")),
            },

            SessionEvent::Recovered(recovery) => {
                if recovery.is_lossless() {
                    banner("recovery was lossless");
                } else {
                    // The server could not replay everything it buffered.
                    banner(&format!("RECOVERY LOST DATA: {:?}", recovery.outcome));
                }
            }

            SessionEvent::Resubscribed(entries) => {
                banner(&format!("{} subscriptions re-executed", entries.len()));
            }

            SessionEvent::Disconnected { reason, retry_in } => match retry_in {
                Some(delay) => banner(&format!(
                    "DISCONNECTED ({reason:?}) — retrying in {delay:?}"
                )),
                None => banner(&format!("DISCONNECTED ({reason:?}) — giving up")),
            },

            SessionEvent::Closed(reason) => {
                banner(&format!("session closed: {reason:?}"));
                break;
            }

            _ => {}
        }
    }
}

/// Makes a session event impossible to miss among the data lines.
fn banner(text: &str) {
    println!("\n===> {text}\n");
}
