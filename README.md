[![License](https://img.shields.io/badge/license-MIT-blue)](./LICENSE)
[![Crates.io](https://img.shields.io/crates/v/lightstreamer-rs.svg)](https://crates.io/crates/lightstreamer-rs)
[![Downloads](https://img.shields.io/crates/d/lightstreamer-rs.svg)](https://crates.io/crates/lightstreamer-rs)
[![Stars](https://img.shields.io/github/stars/joaquinbejar/lightstreamer-rs.svg)](https://github.com/joaquinbejar/lightstreamer-rs/stargazers)
[![Issues](https://img.shields.io/github/issues/joaquinbejar/lightstreamer-rs.svg)](https://github.com/joaquinbejar/lightstreamer-rs/issues)
[![PRs](https://img.shields.io/github/issues-pr/joaquinbejar/lightstreamer-rs.svg)](https://github.com/joaquinbejar/lightstreamer-rs/pulls)
[![Documentation](https://img.shields.io/badge/docs-latest-blue.svg)](https://docs.rs/lightstreamer-rs)

## Lightstreamer TLCP Client for Rust

A Rust client for **TLCP 2.5.0**, the Text Lightstreamer Client Protocol.
Async-first, strongly typed, and deliberately narrow: it speaks the protocol
and nothing else.

### Overview

Lightstreamer is a server that pushes real-time data to clients over an
ordinary web connection — used by brokers, exchanges, betting exchanges and
IoT platforms. TLCP is the text protocol it speaks.

This crate opens a session, keeps it alive through the network's bad
behaviour, delivers updates as a typed `Stream`, and gets out of the way.
There is no market-data model here, no cache, no reconnect-and-hope layer. It
is the protocol, implemented from the specification, with a citation behind
every wire behaviour.

### Features

- **Both directions**: subscribe to real-time items, and send messages
  upstream to the server's Metadata Adapter with per-message outcome
  reporting.
- **All four subscription modes**: `MERGE`, `DISTINCT`, `COMMAND` (with key /
  command row semantics) and `RAW`.
- **Complete update decoding**: unchanged-field runs, null-versus-empty,
  snapshot classification, and both diff formats — TLCP-diff in pure Rust and
  RFC 6902 JSON Patch behind a feature.
- **Honest reconnection**: automatic, and it tells you whether the session
  survived, was recovered with a gap, or was replaced — the distinction that
  decides whether your derived state is still valid.
- **Liveness enforcement**: keepalive and reverse heartbeat, so a connection
  that is wedged but never errors is detected and recovered rather than
  silently trusted.
- **Typed errors**: server codes preserved as numbers, tagged with which of
  the protocol's two overlapping code catalogs they came from.
- **Bounded and lossless**: every stream has a fixed capacity and blocks
  rather than discarding, so a slow consumer stalls the client instead of
  losing data behind your back.
- **Forward compatible**: an unknown notification or an unrecognized literal
  is surfaced with its raw text intact, never fatal.
- **No `unsafe`**: `#![forbid(unsafe_code)]`, no `unwrap` or `expect` in
  library code.

### Installation

```toml
[dependencies]
lightstreamer-rs = "1.0.0-alpha.1"
tokio = { version = "1", features = ["rt-multi-thread", "macros"] }
futures-util = "0.3"   # for StreamExt, to poll the update streams
```

#### Cargo features

| Feature | Default | Pulls in | Gives you |
|---|---|---|---|
| `json-patch` | **off** | `serde_json`, `json-patch` | decoding of `^P` JSON Patch diff-encoded field values (RFC 6902) |
| `test-util` | **off** | nothing | `test_util::ItemUpdateBuilder`, for building an `ItemUpdate` in *your* tests |

`json-patch` is off by default because it adds a JSON stack to a crate others
depend on. Leaving it off is safe rather than lossy: this client advertises
to the server exactly the diff formats it has compiled in
(`LS_supported_diffs`), so a server will never send a `^P` value that cannot
be decoded. Turn it on if your server's adapters emit JSON documents that
benefit from patch compression:

```toml
lightstreamer-rs = { version = "1.0.0-alpha.1", features = ["json-patch"] }
```

`test-util` exists because an `ItemUpdate` cannot be constructed from outside
this crate — forging one that no session produced is not something an
application should be able to do by accident. Your own parsing layer still
deserves unit tests, so the feature adds a builder and nothing else: no
dependency, no behaviour change, no other public item. Enable it under
`[dev-dependencies]`, where Cargo turns it on for your tests and leaves your
release build untouched:

```toml
[dev-dependencies]
lightstreamer-rs = { version = "1.0.0-alpha.1", features = ["test-util"] }
```

```rust,ignore
use lightstreamer_rs::test_util::ItemUpdateBuilder;
use lightstreamer_rs::FieldValue;

let update = ItemUpdateBuilder::new("CS.D.EURUSD.CFD.IP", ["BID", "OFFER"])
    .changed("BID", FieldValue::Text("1.0921"))
    .unchanged("OFFER", FieldValue::Null)
    .build();

assert_eq!(my_parser(&update).bid, Some("1.0921".to_owned()));
```

#### Requirements

- Rust 2024 edition, stable toolchain, MSRV **1.85**.
- A Lightstreamer server (7.4.0 or greater for TLCP 2.5.0), or an account with
  a provider that runs one.

### Quick start

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
                FieldSchema::from_fields(["stock_name", "last_price", "pct_change"])?,
            )
            .with_data_adapter("QUOTE_ADAPTER")
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

### Examples

Four run against `push.lightstreamer.com`, the public demo server the
specification's own transcripts use. No credentials, no setup.

| Command | What it shows |
|---|---|
| `cargo run --example demo_quotes` | MERGE mode: snapshot then live updates, printing which fields actually changed |
| `cargo run --example demo_portfolio` | COMMAND mode: maintains a table of rows from `ADD` / `UPDATE` / `DELETE` |
| `cargo run --example demo_chat` | Sending messages upstream and correlating each outcome to the message that caused it |
| `cargo run --example demo_resilience` | What a reconnection *meant* — break your network while it runs and watch it say so |
| `cargo run --example ig_stream` | **IG**: REST login for the CST / XST tokens, then live prices and account P&L. Needs an account; defaults to IG's demo gateway |

### Core concepts

#### Subscription modes

| Mode | Item state | Snapshot | Use it for |
|---|---|---|---|
| `Merge` | server merges each update into the current value | current value of every field | quotes, prices, any "latest value" feed |
| `Distinct` | none; each update is its own event | the most recent events | messages, alerts, tick-by-tick events |
| `Command` | a keyed table built from `ADD` / `UPDATE` / `DELETE` | the rows that already exist | portfolios, order books, any changing set of rows |
| `Raw` | none | none | raw pass-through, no filtering |

`Command` requires a `key` and a `command` field in the schema; without them
the server refuses the subscription.

#### Null is not empty

TLCP distinguishes *this field has no value* from *this field is the empty
string*, and so does this crate — all the way to your code. A field value is a
`FieldValue`, not a `&str`:

```rust,ignore
match update.field_by_name("close") {
    Some(FieldValue::Text(text)) => println!("closed at {text:?}"),  // may be ""
    Some(FieldValue::Null)       => println!("no closing price at all"),
    None                         => println!("no such field in this schema"),
}
```

Two levels of absence, and they are different questions: `None` means the
schema has no such field, while `Null` means the field exists and the server
says it has no value. The empty string is `Text("")` — a value, just an empty
one — with `is_empty_text()` to ask directly.

Convenience methods (`text()`, `text_or(default)`) exist for when the
distinction genuinely does not matter, but the type makes you decide rather
than deciding for you.

#### Reconnection tells you what it meant

Reconnection is automatic. What is *not* automatic is pretending nothing
happened. `SessionEvent::Connected` carries a `Continuity`:

| Continuity | What happened | Your derived state |
|---|---|---|
| `New` | first connection | build it |
| `Preserved` | same session, nothing missed | still valid |
| `Recovered { .. }` | same session, resumed from a known point | still valid |
| `Replaced { .. }` | a **new** session; subscriptions re-executed | **discard it** |

An application holding an order book, a portfolio or a cache needs that last
row. TLCP is one of the few protocols that provides the distinction, and
hiding it would be the single most expensive convenience this crate could
offer.

#### Errors carry the server's own code

TLCP has two catalogs of numeric codes and **they overlap**: fourteen numbers
appear in both with different meanings. Code `20` is "session not found on a
bind request" in one and "session not found" in the other; code `48` is
"maximum session duration reached" in one and "MPN device suspended" in the
other. A caller matching on a bare number would be guessing, so the variant
says which catalog it came from:

| Variant | Catalog | Arrives on |
|---|---|---|
| `Error::Session` | Session error codes | `CONERR`, `END` |
| `Error::Request` | Control error codes | `REQERR`, `ERROR`, `MSGFAIL` |

A code of `0` or below was supplied by the server's **Metadata Adapter**, not
by the protocol — `ServerError::is_adapter_defined()` reports exactly that. It
is the difference between "the protocol refused you" and "your broker refused
you". A request that failed before reaching any server is
`SessionEvent::RequestNotSent`, never a fabricated code.

### Architecture

Four layers, strictly one-directional at compile time:

```
client  ──▶  session  ──▶  { protocol,  transport PORT (trait) }
                                            ▲
                              transport ADAPTERS (ws, http)
```

- **`protocol`** is **pure**: bytes ↔ typed values, no I/O, no async, no
  `tokio`. That purity is what makes every wire behaviour testable from
  fixtures lifted out of the specification.
- **`transport`** is a port plus its adapters. An adapter moves frames; it
  never interprets what a notification means. Every line the server produces —
  stream notifications *and* control responses — surfaces through one channel,
  so the session layer never learns how many sockets are involved.
- **`session`** owns the lifecycle state machine, liveness and recovery.
- **`client`** is the public façade, and the only layer whose names are semver
  promises.

Data flows the other way at run time: transport task → session → client →
caller, over bounded channels. A lower layer never calls up into a higher one.

### Project structure

```
src/
├── protocol/          # PURE wire layer — no I/O, no async
│   ├── escaping.rs    # percent codec, second-level value markers
│   ├── request.rs     # every request, typed parameters
│   └── response.rs    # every notification, typed variants
├── subscription/      # per-subscription state — no I/O
│   ├── update.rs      # value-list decoding, unchanged runs, diffs
│   ├── manager.rs     # schema, item state, snapshot classification
│   └── item_update.rs # ItemUpdate — the public update type
├── transport/         # I/O
│   ├── mod.rs         # the Transport port
│   └── ws.rs          # WebSocket transport
├── session/           # the lifecycle state machine
│   ├── mod.rs         # create → bind → loop/rebind → recovery
│   ├── liveness.rs    # keepalive, reverse heartbeat
│   ├── backoff.rs     # bounded jittered reconnection
│   └── options.rs     # session-level tunables
├── client/            # the public façade
├── config/            # validated builder configuration
├── error.rs           # the public error taxonomy
└── lib.rs             # crate docs and the public surface
docs/
├── SPEC.md            # distilled TLCP reference; spec section → module map
├── spec/              # six chapters, every statement page-cited
└── adr/               # architecture decisions and their reasoning
examples/              # runnable demos
```

### Specification conformance

Every wire behaviour in `src/` cites the chapter of [`docs/spec/`](docs/spec/)
it derives from, and every chapter cites the page of the official
[TLCP Specification](https://www.lightstreamer.com/sdks/ls-generic-client/2.5.0/TLCP%20Specifications.pdf)
it came from. That trail is not documentation hygiene — it is what makes the
implementation auditable, and it ships with the crate.

Where the specification does not determine a behaviour, the source says so:
87 `SPEC-AMBIGUITY` comments mark points where the document is silent,
ambiguous, or contradicts its own examples. Each takes the defensive option
and names the gap instead of guessing. They are resolved empirically against a
real server as they are hit — never by intuition
([ADR-0006](docs/adr/0006-empirical-resolution-of-spec-ambiguities.md)).

Out of scope: Mobile Push Notifications (MPN), surveyed in
[`docs/spec/06-mpn.md`](docs/spec/06-mpn.md).

### Status

Under active development toward `1.0.0`.

| Area | State |
|---|---|
| Protocol layer (encode, parse, escaping, update decoding) | implemented, tested |
| WebSocket transport | implemented, verified against live servers |
| Session state machine, liveness, recovery | implemented, tested |
| Subscription state, all four modes | implemented, tested |
| Public API, config, error taxonomy | implemented |
| HTTP streaming and long polling | designed, not yet implemented ([ADR-0002](docs/adr/0002-all-three-transports-in-1-0-0.md)) |
| MPN | out of scope |

Around 500 tests, all hermetic — no network, no real timers. The examples are
the live verification, and four of them have been run against real servers.

### Development

```bash
cargo test                                              # unit tests and doctests
cargo test --features json-patch                        # with JSON Patch decoding
cargo clippy --all-targets --all-features -- -D warnings
cargo fmt --check
cargo run --example demo_quotes                         # live smoke test
```

### Version 1.0: a rewrite, and a licence change

**Versions up to and including 0.3.3 were GPL-3.0-only.** They contained code
derived from
[`daniloaz/lightstreamer-client`](https://github.com/daniloaz/lightstreamer-client),
which made the crate unusable for most consumers: copyleft propagates to
anything that links it.

**Version 1.0 is a complete, from-scratch rewrite under MIT.** It shares no
code with those versions. It was written from the official specification —
distilled into [`docs/spec/`](docs/spec/) with a page citation behind every
statement — without consulting the prior implementation in any form. The old
versions remain published under their own licence as legitimate history; they
are not yanked or relabeled. The reasoning and the discipline are recorded in
[ADR-0001](docs/adr/0001-mit-relicensing-clean-room.md).

**Upgrading from 0.3.x is not a drop-in replacement.** The API was designed
fresh from the specification's vocabulary and idiomatic Rust rather than
ported. Expect to rewrite an integration, not to adjust imports. The most
visible changes:

- delivery is a `Stream`, not listener traits
  ([ADR-0003](docs/adr/0003-typed-event-stream-as-delivery-surface.md));
- null and empty field values are distinct, through `FieldValue`;
- reconnection reports its consequences
  ([ADR-0005](docs/adr/0005-recovery-is-visible-in-the-event-stream.md)).

### Documentation

- [docs.rs/lightstreamer-rs](https://docs.rs/lightstreamer-rs) — API reference.
- [`docs/SPEC.md`](docs/SPEC.md) — the distilled TLCP reference and the map
  from specification section to module.
- [`docs/adr/`](docs/adr/) — every architecture decision and why it was made.

### Contributing

Contributions are welcome:

1. Fork the repository.
2. Create a feature branch: `git checkout -b feature/my-feature`.
3. Make your changes.
4. Run the checks: `cargo test`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo fmt --check`.
5. Push the branch and open a pull request.

**One rule is absolute.** This crate's MIT licence rests on the claim that no
code in it derives from the GPL lineage. Do not read, copy from, or describe
the `legacy-gpl` branch, any `v0.*` tag, or the upstream GPL project — not for
reference, not to check a corner case. Derive everything from the official
specification and from [`docs/spec/`](docs/spec/), and cite the chapter your
change comes from. A pull request whose provenance cannot be explained cannot
be merged, however good the code is.

### Contact

- **Author**: Joaquín Béjar García
- **Email**: <jb@taunais.com>
- **Telegram**: [@joaquin_bejar](https://t.me/joaquin_bejar)
- **Repository**: <https://github.com/joaquinbejar/lightstreamer-rs>
- **Documentation**: <https://docs.rs/lightstreamer-rs>

## ✍️ License

Licensed under the MIT license — see [LICENSE](LICENSE).

Versions 0.3.3 and earlier are GPL-3.0-only and are not covered by it.

## Disclaimer

This software is not officially associated with Lightstreamer Srl. It is an
independent client implementation of the publicly documented TLCP protocol.
If you use it to handle financial data, test thoroughly against a demo
environment before relying on it.
