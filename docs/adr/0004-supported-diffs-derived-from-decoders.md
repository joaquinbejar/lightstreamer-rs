# ADR-0004 — `LS_supported_diffs` is derived from the implemented decoders

- **Status:** Accepted
- **Date:** 2026-07-20

## Context

A session-creation request may carry `LS_supported_diffs`, by which the client
**declares which diff (delta) encodings it can decode**
[`docs/spec/03-requests.md` §2]. The server then may send field values encoded
with any declared algorithm [`docs/spec/04-notifications.md` §2].

This creates a hard coupling between two modules that would otherwise be
independent: the request encoder (`src/protocol/request.rs`) advertises a
capability, and the update decoder (`src/subscription/update.rs`) must honor
it. If the advertised set ever exceeds the implemented set, the server is
entitled to send a value the client cannot decode — a protocol violation
originating entirely on our side, and one that would surface as corrupted data
rather than as a clean error.

The obvious implementation — a constant string in the request builder, and a
`match` in the decoder — puts the two halves of one fact in two files, where
they can drift silently.

## Decision

1. **Implement every diff algorithm the specification defines**
   [`docs/spec/04-notifications.md` §2]. Partial support buys nothing: the
   decoders are pure functions over the previous value and the diff, fully
   testable from the spec's worked examples.
2. **The advertised value is computed from the decoder registry, not written by
   hand.** A single source of truth enumerates the supported algorithms; the
   request encoder derives the `LS_supported_diffs` value from it, and the
   decoder dispatches from it. Adding an algorithm is one edit; forgetting to
   advertise it, or advertising it without implementing it, is not
   representable.
3. **An unknown diff algorithm on the wire is a typed error, never a silent
   passthrough.** If the server sends an encoding we did not advertise, that is
   a server-side violation and the caller is told, rather than being handed a
   value that is wrong in a way it cannot detect.

## Consequences

- The decoder set and the advertisement cannot drift. This removes an entire
  class of bug that would otherwise be invisible until a specific field on a
  specific adapter started producing wrong values in production.
- Every algorithm needs exhaustive tests before it may be advertised — which is
  the correct incentive, since advertising is a promise to the server.
- If a future spec version adds an algorithm, support is additive: implement
  the decoder, register it, and the advertisement updates itself.
- Slight cost: the advertised value is computed rather than a literal, so a
  test asserts its exact wire form against the specification's syntax to keep
  the derivation honest.
