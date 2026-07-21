//! Sends messages upstream and reads the room they land in.
//!
//! Everything so far has been one-directional: the server pushes, the client
//! consumes. TLCP also carries messages *to* the server, where the Metadata
//! Adapter decides what they mean [`docs/spec/03-requests.md` §12]. The demo
//! server's `CHAT_ROOM` Data Adapter echoes them back to every subscriber, so
//! one program can watch both halves.
//!
//! **What you will actually see.** The public demo's Metadata Adapter refuses
//! messages from an unidentified client, answering each one with Appendix B
//! code `34`, "Message processing error". That is not a failure of this
//! example — it is the interesting half of it. The message reached the server,
//! the server judged it, and the refusal came back correlated to the exact
//! message that caused it, with the Adapter's own code intact. Against a
//! server whose Adapter accepts your messages, the same code prints
//! `delivered` instead.
//!
//! Two things worth noticing:
//!
//! - A **numbered** message gets an outcome reported on the session event
//!   stream — processed, or refused, with the Adapter's own code. A
//!   [`Message::fire_and_forget`] one gets nothing back, by protocol rule: a
//!   message may omit its progressive only if it also declines the report.
//! - The chat room is a **DISTINCT**-mode subscription, not MERGE: every
//!   message is its own event rather than a new value for the same field
//!   [`docs/spec/04-notifications.md` §2.4].
//!
//! Run it with:
//!
//! ```text
//! cargo run --example demo_chat
//! ```

use std::num::NonZeroU32;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use futures_util::StreamExt;
use lightstreamer_rs::{
    AdapterSet, Client, ClientConfig, FieldSchema, ItemGroup, Message, MessageResult,
    ServerAddress, SessionEvent, SessionEvents, Subscription, SubscriptionEvent, SubscriptionMode,
};

/// The server the specification's own transcripts use.
const DEMO_SERVER: &str = "https://push.lightstreamer.com";

/// The Adapter Set that serves the demo feeds.
const DEMO_ADAPTER_SET: &str = "DEMO";

/// The Data Adapter that echoes chat messages to every subscriber.
const CHAT_ADAPTER: &str = "CHAT_ROOM";

/// The single item carrying the room's traffic.
const ITEM: &str = "chat_room";

/// What each message carries.
const FIELDS: [&str; 3] = ["raw_timestamp", "IP", "message"];

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
    let (client, session_events) = Client::connect(config).await?;
    // `Client` is deliberately not `Clone`: sharing it through an `Arc` keeps
    // "the session ends when the last reference goes" a single, visible rule.
    let client = Arc::new(client);

    // Message outcomes arrive on the session stream, so it has to be watched
    // for this example to show anything about sending.
    tokio::spawn(watch_messages(session_events));

    let subscription = Subscription::new(
        // DISTINCT: each message is an event in its own right, and none
        // supersedes the last.
        SubscriptionMode::Distinct,
        ItemGroup::from_items([ITEM])?,
        FieldSchema::from_fields(FIELDS)?,
    )
    .with_data_adapter(CHAT_ADAPTER);

    let mut updates = client.subscribe(subscription).await?;

    // Send a few numbered messages, in order. The progressive is what lets the
    // server order and deduplicate them.
    tokio::spawn({
        let client = Arc::clone(&client);
        async move {
            for number in 1..=3u32 {
                tokio::time::sleep(Duration::from_secs(2)).await;
                let Some(progressive) = NonZeroU32::new(number) else {
                    continue;
                };
                let text = format!("hello from lightstreamer-rs #{number}");
                println!("→ sending: {text}");
                if let Err(error) = client.send_message(Message::new(text, progressive)).await {
                    eprintln!("could not send: {error}");
                    return;
                }
            }
        }
    });

    while let Some(event) = updates.next().await {
        match event {
            SubscriptionEvent::Activated { .. } => println!("listening to the room\n"),

            SubscriptionEvent::Update(update) => {
                let who = update
                    .field_by_name("IP")
                    .map_or("somebody", |value| value.text_or("(anonymous)"));
                let said = update
                    .field_by_name("message")
                    .map_or("", |value| value.text_or(""));
                println!("← {who}: {said}");
            }

            SubscriptionEvent::Rejected(error) => {
                eprintln!("the server refused the subscription: {error}");
                break;
            }

            SubscriptionEvent::Unsubscribed => break,

            other => println!("{other:?}"),
        }
    }

    match Arc::try_unwrap(client) {
        Ok(client) => client.disconnect().await,
        // A sender task still holds a reference; dropping ours is enough,
        // since the session ends with the last one.
        Err(_) => Ok(()),
    }
}

/// Reports what the server did with each message this client sent.
async fn watch_messages(mut events: SessionEvents) {
    while let Some(event) = events.next().await {
        match event {
            SessionEvent::Message(outcome) => match &outcome.result {
                MessageResult::Delivered { response } => {
                    println!(
                        "\u{2713} message {:?} delivered{}",
                        outcome.progressive,
                        if response.is_empty() {
                            String::new()
                        } else {
                            format!(" (adapter said: {response})")
                        }
                    );
                }
                MessageResult::Refused(error) | MessageResult::Failed(error) => {
                    // The Metadata Adapter said no, with a code of its own.
                    println!(
                        "\u{2717} message {:?} refused: {error}",
                        outcome.progressive
                    );
                }
                other => println!("message {:?}: {other:?}", outcome.progressive),
            },

            SessionEvent::Closed(reason) => {
                println!("session closed: {reason:?}");
                break;
            }

            _ => {}
        }
    }
}
