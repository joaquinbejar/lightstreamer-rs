# lightstreamer-rs

[![MIT License](https://img.shields.io/badge/license-MIT-blue)](./LICENSE)
[![Crates.io](https://img.shields.io/crates/v/lightstreamer-rs.svg)](https://crates.io/crates/lightstreamer-rs)
[![Documentation](https://img.shields.io/badge/docs-latest-blue.svg)](https://docs.rs/lightstreamer-rs)

A Rust client for **TLCP**, the Text Lightstreamer Client Protocol.

Lightstreamer is a server that pushes real-time data to clients over an
ordinary web connection. This crate implements TLCP 2.5.0 and nothing else: it
opens a session, keeps it alive through the network's bad behaviour, delivers
updates as a typed stream, and gets out of the way. No market-data model, no
cache, no reconnect-and-hope layer — just the protocol, done properly.

```rust,no_run
use futures_util::StreamExt;
use lightstreamer_rs::{
    AdapterSet, Client, ClientConfig, FieldSchema, ItemGroup, ServerAddress, Snapshot,
    Subscription, SubscriptionEvent, SubscriptionMode,
};

#[tokio::main]
async fn main() -> lightstreamer_rs::Result<()> {
    let config = ClientConfig::builder(ServerAddress::try_new("https://push.lightstreamer.com")?)
        .with_adapter_set(AdapterSet::try_new("DEMO")?)
        .build()?;

    let (client, _session_events) = Client::connect(config).await?;

    let mut updates = client
        .subscribe(
            Subscription::new(
                SubscriptionMode::Merge,
                ItemGroup::from_items(["item1", "item2"])?,
                FieldSchema::from_fields(["last_price", "time", "pct_change"])?,
            )
            .with_snapshot(Snapshot::On),
        )
        .await?;

    while let Some(event) = updates.next().await {
        if let SubscriptionEvent::Update(update) = event {
            println!("{}: {:?}", update.item_name(), update.changed_fields());
        }
    }

    client.disconnect().await
}
```

Run it with `cargo run --example demo_quotes` — it connects to the public demo
server the specification's own transcripts use.

## What this crate gives you

- **Delivery is a `Stream`, not a callback.** Subscribing yields an update
  stream; the client yields a session-event stream. Both carry closed,
  exhaustively-matchable enums, so the borrow checker never lands in the middle
  of your update handler.
- **Reconnection is automatic; its consequences are not hidden.** When the
  connection breaks the client reconnects — and tells you whether the *same*
  session came back, so whatever you computed from it is still valid, or a
  **new** one replaced it, so it is not. An application holding an order book
  or a portfolio needs that distinction, and TLCP is one of the few protocols
  that actually provides it.
- **Null and empty stay distinct.** TLCP separates "this field has no value"
  from "this field is the empty string"; so does this crate, all the way to
  your code, via `FieldValue`.
- **Streams are bounded and lossless.** When a stream fills, the client blocks
  rather than discarding — a consumer that stops reading stalls the client
  instead of silently losing data.
- **Unknown is not fatal.** An unrecognized notification or an unexpected
  literal is surfaced with its raw text preserved, never swallowed and never
  fatal: a server newer than this crate must not be able to break it.

Transports: WebSocket, HTTP streaming, and HTTP long polling, behind one port,
with an option to force a specific one.

## Licence and the 1.0 rewrite

**Version 1.0 is a complete, from-scratch rewrite, licensed under MIT.**

Versions up to and including 0.3.3 were **GPL-3.0-only**, because they
contained code derived from
[`daniloaz/lightstreamer-client`](https://github.com/daniloaz/lightstreamer-client).
Those versions remain published under that licence as historical versions and
are not affected by this change. They share no code with 1.0.

The 1.0 tree was written independently from the official
[TLCP specification](https://www.lightstreamer.com/sdks/ls-generic-client/2.5.0/TLCP%20Specifications.pdf),
distilled into [`docs/spec/`](docs/spec/) with a page citation behind every
statement; each implementation file cites the chapter it derives from. The
reasoning is recorded in
[ADR-0001](docs/adr/0001-mit-relicensing-clean-room.md).

**Upgrading from 0.3.x is not a drop-in replacement.** The API was designed
fresh rather than ported; expect to rewrite your integration, not to adjust
imports.

## Status

Under active development toward 1.0.0. The protocol layer, the WebSocket
transport, the session state machine, and the public API are implemented and
tested; HTTP streaming and long polling are in progress.

## Documentation

- [`docs/SPEC.md`](docs/SPEC.md) — the distilled TLCP reference and the map
  from specification section to module.
- [`docs/adr/`](docs/adr/) — the architecture decisions and their reasoning.

## Licence

MIT — see [LICENSE](LICENSE).
