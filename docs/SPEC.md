# TLCP Reference — the implementation contract for `lightstreamer-rs` v1

This directory is the distilled reference for **TLCP 2.5.0** (Text
Lightstreamer Client Protocol), the protocol this crate implements. It is not
background reading: it is the **sole permitted source** for every line of wire
handling in `src/`, and the citation trail that backs this crate's relicensing
from GPL-3.0-only to MIT.

## Provenance and why this file exists

Versions ≤ 0.3.3 of `lightstreamer-rs` contained code derived from
`daniloaz/lightstreamer-client` under GPL-3.0-only. Version `1.0.0` is a
from-scratch, independently authored implementation under MIT. The claim the
relicensing rests on is narrow and checkable: **every wire behavior in this
crate is derived from the published specification, not from the prior work.**

These chapters were written directly from the official document —

> *TLCP Specification — Text Lightstreamer Client Protocol*, Version 2.5.0,
> target Lightstreamer Server 7.4.0+, last updated 19/3/2024.
> <https://www.lightstreamer.com/sdks/ls-generic-client/2.5.0/TLCP%20Specifications.pdf>

— and nothing else. The `legacy-gpl` branch, the `v0.*` tags, the old crate's
docs.rs pages, and the upstream GPL project were not consulted and must not be.

**The rule that makes this enforceable:** every implementation file cites the
chapter and section here that its behavior comes from, and every chapter here
cites the specification page it came from. An implementation behavior with no
citation is 🔴 in review — not because citations are tidy, but because an
uncited behavior is provenance-unexplained, and provenance is the whole
argument. `license-auditor` checks this before every merge.

## Chapters

| Chapter | Covers | Spec pages | Lines |
|---|---|---|---|
| [`spec/01-foundations.md`](spec/01-foundations.md) | Concepts, transports, subscription data model, request syntax (HTTP + WS), **the line format, percent-encoding, and the normative parsing algorithm** | 5–14 | 475 |
| [`spec/02-session-lifecycle.md`](spec/02-session-lifecycle.md) | **The state machine**, creation, rebind/loop, recovery, definitive loss, long polling, liveness, workflow sequences, verbatim cURL transcripts | 6–8, 15–21, 66–68, 69–99 | 1618 |
| [`spec/03-requests.md`](spec/03-requests.md) | Every request, with complete parameter tables — session creation/binding, the six control operations, message send, heartbeat, WS establishment check | 22–48 (less MPN) | 1089 |
| [`spec/04-notifications.md`](spec/04-notifications.md) | Every notification — **real-time update decoding, the pipe-separated value list, diff values**, COMMAND-mode semantics, subscription/message/session notifications | 49–65 (less MPN) | 977 |
| [`spec/05-error-codes.md`](spec/05-error-codes.md) | Cross-cutting catalog: **109 code rows**, per-context sub-tables, error-response shapes, and what the spec says about recoverability | whole document | 442 |
| [`spec/06-mpn.md`](spec/06-mpn.md) | Mobile Push Notifications — survey only, **out of scope for v1** | 36–42, 57–59 | 183 |

Total: **4784 lines**, **91 flagged ambiguities**.

## Spec → module ownership

Each row is the contract for one module: the module implements that spec
material, cites it, and is tested against the fixtures quoted there.

| Spec material | Module | Owner agent |
|---|---|---|
| §01 ch.6 request syntax, §03 all requests | `src/protocol/request.rs` | `protocol-expert` |
| §01 ch.7 line format + parsing algorithm, §04 all notifications | `src/protocol/response.rs` | `protocol-expert` |
| §01 §7.2 percent-encoding, §04 §2 second-level value syntax (`#`/`$`, unchanged-field runs) | `src/protocol/escaping.rs` | `protocol-expert` |
| §04 §2 update decoding, diff application, COMMAND key/command semantics | `src/subscription/update.rs` | `protocol-expert` |
| §01 ch.6 transport framing (HTTP paths/methods, WS subprotocol) | `src/transport/{ws,http}.rs` | `transport-expert` |
| §02 the state machine, rebind/loop, recovery, definitive loss, long polling | `src/session/mod.rs` | `transport-expert` |
| §02 ch.8 liveness — keepalive, `PROBE`, reverse heartbeat, content-length limits | `src/session/liveness.rs` | `transport-expert` |
| §03 control operations, subscription id allocation, resubscribe-on-recovery | `src/subscription/manager.rs` | `transport-expert` |
| §03 session-creation parameters as user-facing options | `src/config/*` | `api-expert` |
| §05 the code catalog and recoverability classification | `src/error.rs` | `api-expert` |
| §06 MPN | *(none — out of scope for v1)* | — |

## What the specification does not decide — now recorded as ADRs

The distillation surfaced five decisions the specification leaves to the
implementer. All are settled; see [`README.md`](README.md) for the full ADR
inventory.

1. **Which diff algorithms to advertise** — [ADR-0004](adr/0004-supported-diffs-derived-from-decoders.md).
   Implement every algorithm the spec defines, and *derive* the
   `LS_supported_diffs` value from the decoder registry so the advertisement
   and the implementation cannot drift.
2. **Transport scope for 1.0.0** — [ADR-0002](adr/0002-all-three-transports-in-1-0-0.md).
   WebSocket, HTTP streaming, and long polling all ship, behind one port.
3. **The event surface** — [ADR-0003](adr/0003-typed-event-stream-as-delivery-surface.md).
   A typed `Stream` of exhaustively-matchable events; no listener traits.
4. **How recovery is surfaced** — [ADR-0005](adr/0005-recovery-is-visible-in-the-event-stream.md).
   Reconnection is automatic, its consequences are explicit: continuity
   preserved vs session re-established vs definitively lost.
5. **Ambiguity policy** — [ADR-0006](adr/0006-empirical-resolution-of-spec-ambiguities.md).
   The 91 `⚠️ Spec unclear:` flags are resolved by experiment against a real
   server (`push.lightstreamer.com` with `LS_adapter_set=DEMO`, a local
   `WELCOME` instance, or an IG demo account) and the finding recorded next to
   the flag — never by intuition, never by consulting the old implementation.
   The flags are not defects in these chapters; they are the honest edge of
   what the document determines.

## Implementation order

Derived from the dependency structure above — each milestone is testable before
the next begins:

1. **`protocol` core** — line format, parsing algorithm, percent-encoding,
   request encoding. Pure, no I/O, fixtures lifted verbatim from §01/§03/§04.
2. **Update decoding** — the pipe-separated list, `#`/`$`, unchanged-field
   runs, diff application, COMMAND-mode item state. Still pure. This is the
   highest-risk decoding surface in the protocol; it earns exhaustive tests.
3. **Transport port + WebSocket** — framing only, no protocol semantics.
4. **Session state machine** — creation, bind, loop/rebind, recovery, liveness.
   Tested as a state machine against the §02 transcripts, not against a socket.
5. **Subscription manager** — control operations, id allocation, resubscribe.
6. **Public API** — client façade, config, error taxonomy, rustdoc, examples.
7. **HTTP streaming and long polling** — the second and third transports
   against the now-proven port (ADR-0002), each re-running the full lifecycle
   test suite from step 4.

Steps 1 and 2 are the foundation everything else rests on, and they are the two
that are entirely testable from this reference alone. Write them first, and
write them to a standard where the tests read as an executable copy of the
specification.
