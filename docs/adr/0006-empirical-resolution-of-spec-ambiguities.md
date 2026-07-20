# ADR-0006 — Spec ambiguities are resolved empirically, never by intuition

- **Status:** Accepted
- **Date:** 2026-07-20

## Context

Distilling the TLCP 2.5.0 specification produced **91 flagged points** where
the document is silent, ambiguous, or self-inconsistent — recorded as
`⚠️ Spec unclear:` entries with a per-chapter index in every file under
`docs/spec/`. Examples: whether a receiver must reject a bare `LF` line
terminator; which characters the server percent-encodes; whether a combo
request's failure correlates to the failing control request.

Some are harmless. Others determine observable behavior, and an implementer
must decide something. There are three ways to decide, and only one of them is
admissible here:

- **From the old GPL implementation** — inadmissible. This is precisely the
  contamination ADR-0001 exists to prevent, and an ambiguity is exactly where
  the temptation is strongest, because the old code "already solved it".
- **From intuition** — cheap, and produces confident code whose provenance is
  unexplainable and whose correctness is unverified.
- **From observation of a real server** — verifiable, and citable.

## Decision

1. **Any flagged ambiguity that affects observable behavior is resolved by
   experiment against a real Lightstreamer server**, and the finding is
   recorded in the relevant `docs/spec/` chapter next to the flag: what was
   tested, what the server did, and the date and server version.
2. **Test targets**, both named in the specification's own examples
   [`docs/spec/02-session-lifecycle.md` §10] and therefore admissible sources:
   - the public demo server `push.lightstreamer.com` with
     `LS_adapter_set=DEMO`;
   - a local Lightstreamer Server with the `WELCOME` adapter set for
     scenarios the public server cannot produce (forced rebind, content-length
     limits, error paths);
   - the user's IG demo account, for behavior of a real production deployment.
3. **An unresolved ambiguity is handled defensively and documented**, never
   guessed: accept what the spec permits, reject what it forbids, and surface a
   typed error for what it does not determine — with a code comment pointing at
   the flag.
4. **A resolution by observation is a fact about one server version**, not a
   protocol guarantee. It is recorded as such, so a future behavior change is
   diagnosable rather than mysterious.

## Consequences

- Integration tests need a reachable server, so they are gated behind a feature
  flag or an environment check and cannot run in a bare CI sandbox. The unit
  tests — which cover all of `src/protocol/*` — stay hermetic and always run.
- Resolving 91 flags is real work, and most of it is not needed for 1.0.0. The
  flags are prioritized by whether they block a module: escaping and update
  decoding first, since everything downstream depends on them.
- The chapters under `docs/spec/` become living documents: distilled from the
  specification, then annotated with observed behavior. Both kinds of statement
  are cited, and the two are never conflated.
- This ADR is what makes "we did not look at the old code" survive contact with
  the hard cases. The rule only holds if there is a legitimate alternative
  available at the exact moment it is tested.
