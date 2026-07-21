# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project
adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased] — 1.0.0

### Licence

**This release is licensed under MIT.** Versions up to and including 0.3.3 were
**GPL-3.0-only** and remain published under that licence; they are not yanked,
relabeled, or re-published. They are legitimate history.

Version 1.0.0 shares **no code** with them. It is a from-scratch
implementation written from the official TLCP 2.5.0 specification, distilled
into `docs/spec/` with a page citation behind every statement, and each
implementation file cites the chapter it derives from. The prior GPL
implementation was not consulted in any form; the reasoning and the discipline
are recorded in `docs/adr/0001-mit-relicensing-clean-room.md`.

### Breaking

**Upgrading from 0.3.x is not a drop-in replacement.** The public API was
designed fresh from the specification's vocabulary and idiomatic Rust rather
than ported, so expect to rewrite an integration rather than adjust imports.
The most visible differences:

- **Delivery is a `Stream`, not listener traits.** Subscribing yields an
  update stream; the client yields a session-event stream
  (`docs/adr/0003-typed-event-stream-as-delivery-surface.md`).
- **Null and empty field values are distinct** and stay distinct all the way to
  the caller, through `FieldValue`.
- **Reconnection reports its consequences.** The caller can tell a recovered
  session from a replaced one from a definitively lost one
  (`docs/adr/0005-recovery-is-visible-in-the-event-stream.md`).

### Added

- The pure TLCP protocol layer: percent codec, request encoding for every
  request in the specification, a typed parser for every notification, and
  real-time update decoding including unchanged-field runs and both diff
  formats — TLCP-diff in pure Rust, and RFC 6902 JSON Patch behind the
  off-by-default `json-patch` feature.
- The WebSocket transport, behind a `Transport` port shaped so the session
  layer needs no socket to test
  (`docs/adr/0007-transport-port-shape.md`).
- The session state machine: the specification's normative transitions,
  request-id sequencing across reconnects, keepalive and reverse-heartbeat
  liveness, bounded jittered backoff, and resubscribe-on-recovery.
- Per-subscription state with the snapshot-versus-real-time classification and
  COMMAND-mode key/command semantics.
- The public client: validated builder configuration, typed event streams,
  message sending, and an error taxonomy that preserves the server's codes as
  structured fields — including which of the protocol's two overlapping code
  catalogs a code belongs to.
- The off-by-default `test-util` feature: a `test_util` module that builds the
  event payloads a crate depending on this one receives, with no session
  behind them, so its own parsing, reconnection and message-handling logic is
  unit-testable. `ItemUpdateBuilder`, `ConnectedBuilder`,
  `MessageOutcomeBuilder`, `ResubscribedBuilder`, and the `recovery`,
  `subscription_id` and `command_fields` functions. It exists because those
  payloads are otherwise unconstructible from outside — `ItemUpdate` is
  assembled from private state and the others are `#[non_exhaustive]` — and
  keeping them that way costs a consumer nothing once a test-only feature
  supplies the builders. It pulls in no dependency and adds nothing to the
  default public surface; enable it under `[dev-dependencies]`.

### Known gaps

- HTTP streaming and HTTP long polling are specified and designed for but not
  yet implemented; WebSocket is the working transport
  (`docs/adr/0002-all-three-transports-in-1-0-0.md`).
- Mobile Push Notifications (MPN) are out of scope (`docs/spec/06-mpn.md`).
- Points where the specification does not determine behaviour are handled
  defensively and marked `SPEC-AMBIGUITY` in the source, pending empirical
  resolution against a real server
  (`docs/adr/0006-empirical-resolution-of-spec-ambiguities.md`).

---

## Versions 0.3.3 and earlier

Published under **GPL-3.0-only**. See the repository's `legacy-gpl` branch and
the `v0.*` tags for their source.
