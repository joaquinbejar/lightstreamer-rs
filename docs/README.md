# Documentation

Design documentation for `lightstreamer-rs` v1 — a clean-room Rust client for
the Lightstreamer TLCP protocol, relicensed from GPL-3.0-only to MIT (ADR-0001).

## The implementation contract

- **[`SPEC.md`](SPEC.md)** — start here. The distilled TLCP 2.5.0 reference,
  the map from spec section to Rust module, and the implementation order.
- **[`spec/`](spec/)** — six chapters written directly from the official
  specification, every statement carrying a page citation:

  | | |
  |---|---|
  | [`01-foundations.md`](spec/01-foundations.md) | Concepts, transports, request syntax, line format, escaping, parsing algorithm |
  | [`02-session-lifecycle.md`](spec/02-session-lifecycle.md) | State machine, rebind/loop, recovery, long polling, liveness, transcripts |
  | [`03-requests.md`](spec/03-requests.md) | Every request with complete parameter tables |
  | [`04-notifications.md`](spec/04-notifications.md) | Every notification, update decoding, diff values, COMMAND mode |
  | [`05-error-codes.md`](spec/05-error-codes.md) | Cross-cutting error/cause code catalog |
  | [`06-mpn.md`](spec/06-mpn.md) | Mobile Push Notifications — survey, out of scope for v1 |

## The current state of the implementation

Steps 1–6 of [`SPEC.md`](SPEC.md)'s implementation order are written: the pure
protocol layer, update decoding, the Transport port and its WebSocket adapter,
the session state machine, the subscription manager, and the public client.
Step 7 — HTTP streaming and long polling — is not, which is why ADR-0002 below
is marked as not yet met.

Written is not finished. A repository-wide review is open against this tree
(`docs/V1_CODE_REVIEW.md`, kept out of version control alongside the other
working documents), and its verdict is that the tree is not yet a release
candidate. It is diagnostic only: citing it is not a clean-room provenance
trail, and a fix still has to cite the specification.

## Architecture Decision Records

Every ADR in `adr/` appears here. Adding one without adding its row is a
documentation defect: the index is how a decision is found, and a decision
nobody can find is not governing anything.

| ADR | Decision | Status |
|---|---|---|
| [0001](adr/0001-mit-relicensing-clean-room.md) | MIT relicensing by clean-room rewrite; `legacy-gpl` is inadmissible | Accepted |
| [0002](adr/0002-all-three-transports-in-1-0-0.md) | WebSocket, HTTP streaming, and long polling all ship in 1.0.0 | Accepted — **met**: all three implemented and verified live |
| [0003](adr/0003-typed-event-stream-as-delivery-surface.md) | A typed `Stream` is the delivery surface, not listener traits | Accepted; **amended 2026-07-21** — delivery is bounded and lossless *while the client is running*, with an ordered stop as the single exemption |
| [0004](adr/0004-supported-diffs-derived-from-decoders.md) | `LS_supported_diffs` is derived from the implemented decoders | Accepted |
| [0005](adr/0005-recovery-is-visible-in-the-event-stream.md) | Recovery and session loss are visible in the event stream | Accepted |
| [0006](adr/0006-empirical-resolution-of-spec-ambiguities.md) | Spec ambiguities are resolved empirically, never by intuition | Accepted |
| [0007](adr/0007-transport-port-shape.md) | The Transport port: one line stream, control responses on it, enum dispatch | Accepted |

## The rule that governs all of it

Every wire behavior in `src/` cites the chapter of `docs/spec/` it derives
from; every chapter cites the specification page it came from. The pre-1.0 GPL
implementation on the `legacy-gpl` branch and the `v0.*` tags is **not an
admissible source** and must not be read, for any reason. See ADR-0001.
