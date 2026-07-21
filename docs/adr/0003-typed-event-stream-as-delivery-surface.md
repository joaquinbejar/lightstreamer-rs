# ADR-0003 — A typed `Stream` is the delivery surface, not listener traits

- **Status:** Accepted; amended 2026-07-21 (see *Amendment: the stop lane*)
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

## Amendment: the stop lane (2026-07-21)

The v1 review (finding C-02) showed that "bounded, and nothing is dropped" as
originally stated could not be kept without giving up something worse. A caller
that stops polling a stream blocks the router, which blocks the session driver,
which then stops polling the socket, the command channel, and its own liveness
timers. The consequence was not merely a stalled session: **the client became
impossible to shut down**. `disconnect()` could not be delivered, dropping the
client could not be observed, and the task and its socket lived until the
process did.

The promise is therefore qualified, in one place only:

> Delivery is bounded and lossless **while the client is running**. An ordered
> stop — `Client::disconnect`, or dropping the `Client` — is exempt from
> backpressure: it is signalled out of band, every blocking delivery races it,
> and anything not yet delivered when it fires is discarded along with the
> streams it was going to.

Why this is the right cut:

- **It costs nothing a caller can observe.** The events discarded are events
  for streams that are ending in the same breath. A caller that is reading
  loses nothing, because a reading caller frees room and the delivery
  completes; a caller that is not reading has, by asking to disconnect, said it
  wants no more.
- **The alternative is worse in kind, not degree.** Dropping data *during
  normal operation* would corrupt the recovery progressive
  [`docs/spec/02-session-lifecycle.md` §5.2] and hide loss the protocol goes
  out of its way to report [`docs/spec/04-notifications.md` §3.7]. Leaking a
  task and a socket is not a data-fidelity trade at all; it is a bug.
- **It is the smallest exemption that works.** Nothing else is exempt.
  Liveness in particular is *not*: a client stalled on a full stream still does
  not send heartbeats, and deliberately so. `LS_inactivity_millis` is how the
  client "declares that it is still listening to the stream connection"
  [`docs/spec/02-session-lifecycle.md` §8.4], and a client that has stopped
  reading its socket is not listening. Sending one would be a false statement
  that also denies the server its only means of shedding a client that has
  stopped consuming. The documented consequence stands: stall past the
  keepalive budget and the connection is declared stalled and recovered, with
  the notification count intact.

`Updates` and `SessionEvents` carry this qualification in their own
documentation, since that is where a caller meets it.
