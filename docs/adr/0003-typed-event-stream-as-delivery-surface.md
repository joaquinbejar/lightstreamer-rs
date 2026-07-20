# ADR-0003 — A typed `Stream` is the delivery surface, not listener traits

- **Status:** Accepted
- **Date:** 2026-07-20

## Context

TLCP constrains what must be *delivered* to the application — real-time
updates, snapshot boundaries, subscription outcomes, session events
[`docs/spec/04-notifications.md`] — but says nothing about how a client library
should hand them over.

The official Lightstreamer SDKs (Java, JS, and their ports) use **listener
interfaces**: the application implements a callback object and registers it.
That shape is idiomatic in those languages and unidiomatic in Rust, where it
forces `Arc<dyn Listener>`, pushes the borrow checker into user callbacks, and
inverts control away from the caller's `async` context. It is also, notably,
the shape a Rust port of those SDKs inherits by default.

## Decision

**The caller consumes a typed `Stream` of events.**

```rust
let mut updates = client.subscribe(sub).await?;
while let Some(update) = updates.next().await {
    println!("{}: {:?}", update.item_name(), update.changed_fields());
}
```

- Subscribing yields a stream bound to that subscription; session-level events
  are available as a separate stream from the client.
- Events are a **closed, exhaustively-matchable enum** per stream, so adding a
  variant is a compile-time event for callers (and a semver major).
- **No listener traits in 1.0.0.** Not as a second surface, not as an optional
  layer. A caller who wants callback ergonomics can spawn a task that owns the
  stream and dispatches — that is three lines of user code and needs no API
  surface from us.

## Consequences

- **One public surface to document, version, and keep honest.** Offering both
  would double the semver surface and create two subtly different orderings and
  error paths to reason about.
- **Backpressure becomes explicit and visible.** A stream the caller stops
  polling must not grow without bound, so the channel policy — bounded, and
  what happens to a slow consumer — is a documented part of the contract rather
  than an implementation detail hidden behind a callback. The specification's
  own overflow notification [`docs/spec/04-notifications.md` §3, `OV`] gives a
  precedent for surfacing loss rather than silently absorbing it.
- **Cancellation is natural.** Dropping the stream unsubscribes; dropping the
  client shuts the session down. No deregistration dance.
- This is the point where the v1 API most visibly departs from the pre-1.0
  crate. That is intentional and is part of what makes the rewrite a rewrite
  (ADR-0001) rather than a rename.
- Migration for existing 0.3.x users is real work, not a search-and-replace.
  The CHANGELOG and README must show the new shape prominently.
