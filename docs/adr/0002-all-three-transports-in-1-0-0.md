# ADR-0002 — Ship WebSocket, HTTP streaming, and long polling in 1.0.0

- **Status:** Accepted
- **Date:** 2026-07-20

## Context

TLCP defines two transports and, within them, more than one streaming mode
[`docs/spec/01-foundations.md` §3; `docs/spec/02-session-lifecycle.md` §7]:

- **WebSocket** — full duplex; control requests are multiplexed over the same
  connection as the data stream.
- **HTTP streaming** — a long-lived response body carrying the notifications,
  with control requests issued on separate connections, and a rebind forced
  when the response reaches its content-length limit.
- **HTTP long polling** — the same request/response shape, but the server
  closes the stream after each batch or timeout and the client re-binds.

Each is a genuinely distinct code path: the liveness rules, the rebind
triggers, and the recovery behavior differ between them, not just the socket
type. WebSocket alone would cover the common case and ship sooner; the HTTP
paths exist because WebSocket does not survive every corporate proxy.

## Decision

**All three ship in 1.0.0**, behind a single `Transport` port, with WebSocket
as the default and an explicit option to force a specific transport.

The session state machine is written **against the port, never against a
concrete transport**. Transport-specific behavior (when a rebind is forced,
what liveness means, how control requests are routed) is expressed as
properties the port exposes, so the state machine branches on a declared
property rather than on which transport is in use.

## Consequences

- **Scope and schedule.** 1.0.0 is bigger and later than a WebSocket-only
  release. Accepted deliberately: the transport seam is the hardest thing to
  retrofit, and the HTTP paths are precisely what stress it. Designing the port
  with only one implementation in hand tends to produce a port shaped like that
  implementation.
- **Test matrix triples** for everything downstream of the session layer.
  Every lifecycle test — creation, rebind, recovery, liveness timeout,
  resubscribe — must run against all three. The §02 transcripts give concrete
  fixtures per transport, so this is mechanical rather than speculative.
- **The `Transport` port becomes an internal semver seam** worth reviewing as
  carefully as the public API, since three implementations must satisfy it.
- The implementation order in `docs/SPEC.md` puts WebSocket first anyway: it is
  the simplest to get correct, and it proves the port before the HTTP paths
  stress it. Shipping all three does not mean building all three at once.
- If schedule pressure appears later, the fallback is to release 1.0.0 with
  WebSocket and land the HTTP transports in 1.1 — a minor bump, since adding a
  transport behind an existing port is additive. This ADR would then be
  superseded rather than quietly ignored.
