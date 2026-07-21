---
name: write-adr
description: Procedural workflow for authoring a new Architecture Decision Record (ADR) for lightstreamer-rs in the Context / Decision / Consequences format. Trigger phrases include "write an ADR", "new ADR for X", "document this decision", "add ADR-NNNN". ADRs about wire behavior defer to the official TLCP specification as the owning authority, and never cite the legacy GPL implementation as prior art.
allowed-tools: Read, Write, Edit, Grep, Glob, Bash
---

# Skill: write-adr

Walk through the standard procedure for authoring a new ADR under
`docs/adr/`. Every non-obvious technical decision needs one.

## When to author an ADR

Per `AGENTS.md` and `rules/global_rules.md`:

- The decision is **non-obvious**: another reasonable engineer could
  plausibly choose differently.
- The decision **shapes the codebase** for more than one feature cycle
  (transport abstraction, session state machine, subscription model,
  listener vs channel delivery, error taxonomy).
- The decision **affects multiple concerns** (protocol, session,
  subscriptions, transport, public API).
- The decision **interprets the TLCP spec** where the spec leaves room —
  record the reading and the section it rests on.
- The decision **touches licensing or clean-room provenance** (MIT
  relicensing, a dependency's license, what counts as an admissible
  source).
- The decision **supersedes** an earlier ADR.

Trivial decisions (a private helper's name, file layout within a module)
do not need ADRs.

## Step 1 — pick the next number

```bash
ls docs/adr/ | sort
```

Pick the next sequential number. ADRs are immutable once accepted; never
re-number. The set is currently empty — the first ADR is expected to be
`0001-mit-relicensing-clean-room.md`, recording why the crate is being
rewritten from scratch and what sources are admissible.

## Step 2 — pick a slug

`NNNN-short-kebab-case-title.md`. The title should describe the
*decision*, not the *problem*:

- ✅ `0002-websocket-first-with-http-streaming-fallback.md`
- ✅ `0003-typed-notification-enum-over-string-dispatch.md`
- ❌ `0002-transports-are-confusing.md`

## Step 3 — write it

Follow the exact template used by the existing ADRs (status, date,
Context / Decision / Consequences). Ground every claim about the wire in
the **TLCP Specifications**
(<https://www.lightstreamer.com/sdks/ls-generic-client/2.5.0/TLCP%20Specifications.pdf>)
and the official Lightstreamer documentation, citing the section number —
never from memory, and never from another client implementation.

**Clean-room constraint (absolute).** The pre-1.0 GPL implementation on
the `legacy-gpl` branch and the `v0.*` tags is not admissible evidence
and must not be read, quoted, or described. Do not open, `grep`, `diff`,
or `git show` those refs while researching an ADR; scope history commands
to the current branch (`git log v1`). If the honest Context is "the old
crate did something here", write instead what the *spec* requires and
what this crate will do.

## Step 4 — wire it in

- Add the ADR to the inventory in `docs/README.md`.
- Link it from the design doc(s) it constrains.
- If it touches **wire behavior**: the TLCP specification owns the
  protocol — this repo's ADR records how `lightstreamer-rs` implements or
  interprets it, never a competing definition. Cite the spec section
  first, then state the interpretation and why.
- If it changes the **public API**, note the semver consequence: before
  `1.0.0` the surface is still being frozen; after it, a rename or
  removal is a major bump.
- If it touches **licensing**, restate the boundary explicitly: `1.0.0`
  onwards is MIT; versions ≤ 0.3.3 remain GPL-3.0-only on crates.io and
  are never re-published or relabeled.
- If it closes an open question recorded in a `docs/` page, update that
  page in the same change.
