---
name: feature-checklist
description: Enforces the lightstreamer-rs definition of done — every new TLCP message, request parameter, session state transition, subscription mode behavior, transport, or public API item must ship with (1) a TLCP spec citation plus a clean-room attestation, (2) unit tests covering parsing/serialization on happy + malformed paths, and (3) rustdoc, CHANGELOG and clean lint/fmt gates. Trigger whenever the user requests a new feature or modifies the protocol, session, subscription, transport, or public surface in this repo.
---

# lightstreamer-rs feature checklist

Every new piece of functionality is incomplete until all three blocks are
present and green. Treat this checklist as the **definition of done** —
don't mark a task as completed, don't open a PR, don't tell the user
"ready" until everything below is satisfied.

**Design phase note:** while the repo is pre-code (the `v1` branch has an
empty orphan history — no `src/`, no `Cargo.toml`), a "feature" is a
`docs/` change — blocks 1–2 collapse into: the design page is consistent
with its neighbors, `AGENTS.md`, and the ADRs; every wire-format claim
cites the TLCP section it comes from; and the clean-room attestation in
block 1 still applies verbatim.

## 1. Spec conformance & clean-room provenance

- **Cite the spec.** Any change to the wire format — a request parameter,
  a notification, an encoding rule, a timing rule — names the **TLCP
  Specifications section number** it implements, in the code comment or
  rustdoc *and* in the PR body. Source of truth:
  <https://www.lightstreamer.com/sdks/ls-generic-client/2.5.0/TLCP%20Specifications.pdf>
  plus the official Lightstreamer documentation. An uncited wire behavior
  is not done; a wire behavior with no spec basis at all does not ship.
- **Forward compatibility.** Unknown notifications, unknown fields, and
  unknown error codes are ignored or surfaced as a typed "unknown"
  variant — never a panic and never a hard failure. Public enums that the
  protocol may extend are `#[non_exhaustive]`.
- **Clean-room attestation.** Every change carries the statement, in the
  PR body: *"Derived from the TLCP specification and official
  Lightstreamer documentation only; the `legacy-gpl` branch and the
  `v0.*` tags were not consulted."* This is literal, not ceremonial —
  do not read, check out, `grep`, `diff`, or `git show` those refs, and
  do not ask another agent to do it for you. Scope history commands to
  the current branch (`git log v1`); never pass `--all`, `legacy-gpl`,
  or a `v0.*` ref.
- **License hygiene.** No GPL/AGPL/SSPL dependency enters the tree, and
  no file carries a non-MIT header. `Cargo.toml` says `license = "MIT"`.
  Versions ≤ 0.3.3 stay GPL on crates.io and are never touched.

## 2. Unit tests

Scope: TLCP request serialization, notification parsing, field-value
decoding, the session state machine, subscription mode semantics,
transport framing, and config parsing.

- **Where:** alongside the code, inside `#[cfg(test)] mod tests`.
- **Coverage bar (judgment, not metrics):**
  - Every notification variant: a byte-for-byte sample line from the spec
    parsed into the typed value — plus the **malformed** path (truncated
    line, bad arity, non-numeric where a number is required, unknown
    message name) asserted to return a typed error, never a panic.
  - Every request builder: the encoded line matches the spec, including
    parameter names, ordering constraints, and percent-encoding of
    values that contain reserved characters.
  - Field-value decoding: the null / empty-string / unchanged-field
    placeholders and delta delivery, including a round-trip against a
    multi-update sequence.
  - Session state machine: create → bind → rebind → recovery → close, the
    keepalive and reverse-heartbeat timeouts, and the illegal transitions.
  - Subscription modes: `MERGE` accumulation, `DISTINCT` delivery,
    `COMMAND` add/update/delete keying, `RAW` passthrough; plus snapshot
    end (`EOS`) and overflow (`OV`) handling.
- **No live network in unit tests** — drive the parser and the state
  machine from recorded byte fixtures. Integration tests that need a
  server are feature-gated or `#[ignore]`d by default, and their absence
  never substitutes for unit coverage.
- **No `.unwrap()` in library code, ever** — `.get()` + `.ok_or_else()`.
  In test code it is fine.

## 3. API, docs & release surface

- **Panic-free and unsafe-free:** `#![forbid(unsafe_code)]` holds at the
  crate root; no `.unwrap()` / `.expect()` / unchecked indexing or
  slicing on any library path. A server payload must not be able to
  panic the caller's process.
- **Rustdoc on every public item** — `///` on each `pub` item, `# Errors`
  on every fallible `pub fn`, a doctest on the entry points. Module-level
  `//!` docs explain which TLCP concept the module implements.
- **Public surface reviewed:** any change to `src/lib.rs` re-exports is
  called out in the PR. After `1.0.0` it is a semver event and needs the
  matching bump; before it, the addition must be justified — the surface
  is being frozen for `1.0.0`.
- **Credentials never leak:** `LS_user` / `LS_password`, session ids, and
  tokens stay out of `tracing` output, `Debug` impls, and error messages.
- **Gates clean, zero warnings:**
  ```bash
  cargo clippy --all-targets --all-features -- -D warnings
  cargo fmt --all --check
  cargo test
  cargo build --release
  ```
- **CHANGELOG entry** added under the `1.0.0` heading, describing the
  user-visible change (and, for wire changes, the spec section).
- Refresh `src/lib.rs` `//!` docs and the affected `docs/` page; add an
  ADR under `docs/adr/` for a transport choice, a spec-interpretation
  call, a public-API shape, a new dependency, or a licensing decision.
