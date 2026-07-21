//! A Rust client for **TLCP**, the Text Lightstreamer Client Protocol.
//!
//! Lightstreamer is a server that pushes real-time data to clients over an
//! ordinary web connection. TLCP is the text protocol it speaks. This crate
//! implements TLCP 2.5.0 and nothing else: it opens a session, keeps it alive
//! through the network's bad behaviour, delivers updates as a typed stream,
//! and gets out of the way. There is no market-data model here, no cache, no
//! reconnect-and-hope layer — just the protocol, done properly.
//!
//! # A complete program
//!
//! This connects to the public demo server the specification's own transcripts
//! use, subscribes to two items, and prints what changes.
//!
//! ```no_run
//! use futures_util::StreamExt;
//! use lightstreamer_rs::{
//!     AdapterSet, Client, ClientConfig, FieldSchema, ItemGroup, ServerAddress, Snapshot,
//!     Subscription, SubscriptionEvent, SubscriptionMode,
//! };
//!
//! #[tokio::main]
//! async fn main() -> lightstreamer_rs::Result<()> {
//!     let config = ClientConfig::builder(ServerAddress::try_new("https://push.lightstreamer.com")?)
//!         .with_adapter_set(AdapterSet::try_new("DEMO")?)
//!         .build()?;
//!
//!     let (client, _session_events) = Client::connect(config).await?;
//!
//!     let mut updates = client
//!         .subscribe(
//!             Subscription::new(
//!                 SubscriptionMode::Merge,
//!                 ItemGroup::from_items(["item1", "item2"])?,
//!                 FieldSchema::from_fields(["last_price", "time", "pct_change"])?,
//!             )
//!             .with_snapshot(Snapshot::On),
//!         )
//!         .await?;
//!
//!     while let Some(event) = updates.next().await {
//!         match event {
//!             SubscriptionEvent::Update(update) => {
//!                 println!("{}: {:?}", update.item_name(), update.changed_fields());
//!             }
//!             SubscriptionEvent::Rejected(error) => {
//!                 eprintln!("the server refused the subscription: {error}");
//!                 break;
//!             }
//!             _ => {}
//!         }
//!     }
//!
//!     client.disconnect().await
//! }
//! ```
//!
//! # The three things worth knowing before you start
//!
//! **Delivery is a `Stream`, not a callback.** Subscribing gives you an
//! [`Updates`] stream; the client gives you a [`SessionEvents`] stream. Both
//! are ordinary [`futures_util::Stream`]s carrying closed, matchable enums, so
//! the borrow checker never lands in the middle of your update handler
//! (`docs/adr/0003-typed-event-stream-as-delivery-surface.md`).
//!
//! **Reconnection is automatic; its consequences are not hidden.** When the
//! connection breaks, this crate reconnects. What it will not do is pretend
//! nothing happened: [`SessionEvent::Connected`] carries a [`Continuity`] that
//! says whether the *same* session came back — in which case whatever you
//! computed from it is still valid — or whether a **new** one replaced it, in
//! which case it is not
//! (`docs/adr/0005-recovery-is-visible-in-the-event-stream.md`). An
//! application holding an order book or a portfolio needs that distinction,
//! and TLCP is one of the few protocols that actually provides it.
//!
//! **Streams are bounded and lossless.** Every stream this crate hands you has
//! a fixed capacity and, when it fills, the client blocks rather than
//! discarding anything — so a consumer that stops reading stalls the client
//! instead of silently losing data. The full reasoning, and what to do if you
//! genuinely cannot keep up, is on [`Updates`].
//!
//! # Two things that catch people out
//!
//! **Null and empty are different values.** TLCP distinguishes a field with no
//! value from a field holding the empty string, and so does
//! [`FieldValue`]: `Null` and `Text("")` are not equal and do not mean the
//! same thing. A quote adapter with no bid to report sends the first; one
//! whose status line is deliberately blank sends the second. Collapsing them
//! is a decision you make explicitly, with
//! [`FieldValue::text`] or [`FieldValue::text_or`] — this crate will not make
//! it for you. Note also the two levels of absence:
//! [`ItemUpdate::field`] returns `None` when the schema has no such field at
//! all, and `Some(FieldValue::Null)` when the field exists and is null.
//!
//! **Item and field names come from you, not from the server.** TLCP transmits
//! counts, never names [`docs/spec/04-notifications.md` §3.1]. So
//! [`ItemUpdate::item_name`] can only report a name you supplied — which you
//! do by spelling out the list with [`ItemGroup::from_items`] and
//! [`FieldSchema::from_fields`]. If you subscribe by naming a server-side
//! group with [`ItemGroup::try_new`] instead, the server resolves it and this
//! crate never learns what the items are called, so names fall back to the
//! 1-based position rendered in decimal. That stand-in is deliberately not a
//! lookup key: [`ItemUpdate::field_by_name`] with `"3"` returns `None` rather
//! than the third field, so a placeholder can never be mistaken for a real
//! name. Use [`ItemUpdate::declared_item_name`] when you need to know which
//! you got.
//!
//! # Shutdown
//!
//! Dropping the [`Client`] stops everything: the background tasks, the socket,
//! and every stream still outstanding. Dropping an [`Updates`] stream
//! unsubscribes. Nothing needs to be deregistered and nothing needs
//! `std::process::exit`.
//!
//! # What is deliberately not here
//!
//! The transports, the wire parser and the session state machine are private.
//! They are the crate's job, not yours, and keeping them private is what lets
//! them change without breaking you. What [`lib.rs`](self) re-exports is the
//! whole public surface and the whole semantic-versioning promise.
//!
//! # Testing your own code against this crate
//!
//! An [`ItemUpdate`] cannot be constructed from outside, because forging one
//! that a session never produced is not something an application should be
//! able to do by accident. Your own parsing layer still deserves unit tests,
//! so the off-by-default `test-util` feature adds one thing and nothing else:
//! `test_util::ItemUpdateBuilder`, which assembles an update with no session
//! behind it. Enable it under `[dev-dependencies]` and it never reaches your
//! release build.
//!
//! # Errors
//!
//! Everything fallible returns [`Result`], and [`Error`] has one variant per
//! condition worth reacting to. Codes the server supplied are preserved as
//! numbers in [`ServerError`], never flattened into a message — and which of
//! the protocol's two code catalogs a number belongs to is decided by the
//! variant carrying it, because the two overlap. See [`error`].
//!
//! # Provenance
//!
//! Version 1.0.0 onwards is an independently authored implementation licensed
//! under MIT. Versions up to and including 0.3.3 were GPL-3.0-only and share
//! no code with this one; they remain published, under that licence, as
//! historical versions.
//!
//! Every wire behaviour in this crate is derived from the official
//! specification and cites the chapter of `docs/spec/` it comes from. See
//! `docs/adr/0001-mit-relicensing-clean-room.md`.

#![forbid(unsafe_code)]

pub mod config;
pub mod error;

#[cfg(feature = "test-util")]
pub mod test_util;

mod client;
mod protocol;
mod session;
mod subscription;
mod transport;

// --- The client -------------------------------------------------------------
pub use client::Client;

// --- Configuration ----------------------------------------------------------
pub use config::{
    AdapterSet, ClientConfig, ClientConfigBuilder, ConfigError, ConnectionOptions, Credentials,
    RetryPolicy, ServerAddress, Transport,
};

// --- Describing a subscription ----------------------------------------------
pub use client::{
    BufferSize, FieldSchema, FrequencyLimit, ItemGroup, MaxFrequency, Snapshot, Subscription,
    SubscriptionMode,
};

// --- Consuming a subscription -----------------------------------------------
pub use client::{
    CommandFields, Filtering, SubscriptionEvent, SubscriptionId, UpdateFrequency, Updates,
};
pub use subscription::item_update::{
    ChangedFields, Field, FieldValue, Fields, ItemCommand, ItemUpdate, UpdateKind,
};

// --- Sending a message ------------------------------------------------------
pub use client::{Message, MessageOutcome, MessageResult, SequenceName};

// --- Consuming the session --------------------------------------------------
pub use client::{
    ClosedReason, Connected, Continuity, DisconnectReason, Recovery, RecoveryOutcome, Resubscribed,
    ServerInfo, SessionEvent, SessionEvents,
};

// --- Errors -----------------------------------------------------------------
pub use error::{Error, Result, ServerError};
pub use transport::TransportError;
