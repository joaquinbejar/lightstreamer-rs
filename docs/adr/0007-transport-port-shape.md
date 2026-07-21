# ADR-0007 — The Transport port: one line stream, enum dispatch

- **Status:** Accepted
- **Date:** 2026-07-21

## Context

ADR-0002 commits to three transports — WebSocket, HTTP streaming, HTTP long
polling — behind one port, with the session state machine written against the
port and never against a concrete transport. The specification makes that
harder than it sounds:

- With WebSocket, **control requests travel on the stream connection itself**
  [`docs/spec/02-session-lifecycle.md` §1].
- With HTTP the stream connection is a response of almost infinite length and,
  HTTP being half-duplex, **each control request needs its own separate
  connection** [ibid.]. Its response arrives on that separate connection, not
  on the stream.
- After session creation, `CONOK` may carry a **control link** naming a
  different host that subsequent control requests must target
  [`docs/spec/02-session-lifecycle.md` §3.1].
- HTTP streaming ends when the response's content length is reached, forcing a
  rebind; long polling ends every cycle. WebSocket does neither
  [`docs/spec/02-session-lifecycle.md` §2.2, T3/T4].

A port that exposed these differences directly would push three sets of
branches into the session machine, which is exactly what ADR-0002 forbids.

There is also a Rust-specific question. `async fn` in a trait is not
object-safe, so a `dyn Transport` would need boxed futures — either an
`async-trait`-style macro dependency or hand-written `Pin<Box<dyn Future>>`
signatures. But the caller must still be able to *choose* a transport at
runtime, since forcing one is a configuration option.

## Decision

**1. The port exposes a single line stream, and control responses arrive on
it.**

```rust
trait Transport {
    fn properties(&self) -> TransportProperties;
    async fn open_stream(&mut self, request: StreamOpen) -> Result<()>;
    async fn next_line(&mut self) -> Option<Result<String>>;
    async fn send_control(&mut self, request: ControlRequest) -> Result<()>;
    async fn close(&mut self) -> Result<()>;
}
```

`send_control` does not return a response. Every line the server produces —
stream notifications *and* control responses — surfaces through `next_line`.
The HTTP transports own the separate control connection internally and forward
its response lines into the same channel; the WebSocket transport has nothing
to do because the server already multiplexes them.

This is the load-bearing choice. It means the session machine sees one ordered
sequence of lines and correlates responses by request id, exactly as the
protocol intends, with no knowledge of how many sockets are involved.

**2. Transport-dependent behavior is declared, not inferred.**
`TransportProperties` states whether control shares the stream connection,
whether the stream terminates on content length, and whether the transport
polls. The session machine branches on a declared property, never on which
transport is in use — so a fourth transport would need no change to it.

**3. Runtime selection is enum dispatch, not `dyn`.** A closed
`enum AnyTransport` implements `Transport` by delegation. Transports are a set
this crate owns and closes; external transport implementations are not a use
case (the *protocol* is the extension point users care about, and it is fixed).
This keeps `async fn` in the trait, avoids boxed futures, and adds no
dependency.

**4. Framing belongs to the transport.** CR-LF splitting, WebSocket message
boundaries, and chunked-transfer reassembly happen below the port.
`next_line` yields one complete line with its terminator stripped, ready for
`protocol::response::parse_line`.

## Consequences

- The session machine is testable **without a socket**: a mock transport that
  replays the verbatim transcripts in `docs/spec/02-session-lifecycle.md` §10
  satisfies the port. This is what keeps the state machine's tests hermetic,
  and it is worth more than any integration test we could write.
- Control-response correlation must be robust, since responses are interleaved
  with data on one stream. Request ids are the mechanism the protocol already
  provides; the session layer must sequence them across reconnects.
- The HTTP transports carry real hidden complexity — a second connection, its
  lifecycle, and the control-link host. That complexity is contained where it
  belongs instead of leaking upward.
- Adding a transport means implementing the trait and adding one enum variant;
  it is not a semver event, because the port is internal.
- If an external transport implementation ever becomes a genuine requirement,
  the enum becomes a `dyn`-based registry and this ADR is superseded. Nothing
  in the session machine would change, which is the point of the port.
