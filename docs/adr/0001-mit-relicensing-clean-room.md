# ADR-0001 — MIT relicensing by clean-room rewrite

- **Status:** Accepted
- **Date:** 2026-07-20

## Context

`lightstreamer-rs` up to version 0.3.3 was published under **GPL-3.0-only**,
because it contained code derived from `daniloaz/lightstreamer-client`
(GPL-3.0-only, February 2024). The copyleft obligation propagates to anything
that links the crate, which makes it unusable for most consumers — including
proprietary and permissively-licensed downstream projects.

The crate name and the GitHub repository are worth keeping: they carry the
existing users, the crates.io ownership, and the documentation links. What has
to change is the licence, and a licence cannot be changed on code whose
copyright is partly held by someone else.

## Decision

**Rewrite the implementation from scratch and publish it as `1.0.0` under
MIT.** Specifically:

1. **The crate name, the repository, and the crates.io ownership are kept.**
   These are not licensed artifacts; keeping them is unambiguously permitted.
2. **Versions ≤ 0.3.3 remain GPL-3.0-only on crates.io, forever, and are never
   yanked or relabeled.** They were GPL, they were published as GPL, and that
   is legitimate history. `license` is a per-version field on crates.io, so a
   new version may declare a different licence.
3. **The v1 tree starts from an orphan git root commit** with no GPL ancestor.
   The old history is preserved on the `legacy-gpl` branch and the `v0.*` tags,
   so the GPL obligation to keep offering the corresponding source to anyone
   who received a binary is honored rather than erased.
4. **The only admissible sources for the new implementation** are the official
   TLCP Specification v2.5.0, the public Lightstreamer documentation, the
   distilled reference in `docs/spec/`, and observed behavior of a real
   Lightstreamer server.
5. **The `legacy-gpl` branch, the `v0.*` tags, the old crate's docs.rs pages,
   and the upstream GPL project are inadmissible** — not to be read, `git
   show`n, diffed, grepped, or described, by anyone, for any reason, including
   "just to check a corner case" or "just to find a URL". This is enforced by
   a deny list in `.claude/settings.local.json`, restated in every agent
   definition, and audited by `license-auditor` before every merge.
6. **Every wire behavior in `src/` cites the section of `docs/spec/` it derives
   from**, and every chapter there cites the specification page it came from.

## Consequences

- The v1 public API is a deliberate break, not a port. Reproducing the previous
  surface from memory would undermine the very claim this ADR rests on, so the
  API is designed fresh from the specification's vocabulary and idiomatic Rust
  (see ADR-0003).
- The citation trail is load-bearing, not documentation hygiene. An uncited
  wire behavior is 🔴 in review because it is provenance-unexplained — the
  distinction between "derived from the spec" and "remembered from the GPL
  code" is exactly what cannot be reconstructed after the fact.
- Upgrading from 0.3.x to 1.0.0 is not a drop-in replacement for existing
  users. The CHANGELOG must say so plainly, alongside the licence change.
- Contributors must be told the rule before their first PR, because a
  well-meaning "I looked at how it used to work" contaminates the record.
- This ADR records a technical provenance discipline. It is not legal advice
  and does not itself establish that the relicensing is sound; that judgement
  belongs to the copyright holder.

## Notes

Courtesy, not obligation: notifying the original author (@daniloaz) that the
crate has been rewritten and relicensed costs nothing and removes a plausible
source of future friction.
