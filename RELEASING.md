# Releasing

This is the checklist for cutting a release of `lightstreamer-rs`, most
importantly the first stable `1.0.0`. It exists because `1.0.0` is not just a
version bump: it is the first release under the new MIT licence
(`docs/adr/0001-mit-relicensing-clean-room.md`), and getting the release
mechanics wrong would undermine the clean-room claim the whole rewrite rests
on. Read `CLAUDE.md`'s clean-room section first if you have not.

## 1. Every push already gates on this

`make pre-push` runs everything `.github/workflows/ci.yml` runs: formatting,
Clippy, the full feature-matrix test suite, a release build, rustdoc with
warnings denied, `cargo deny check licenses advisories`, an MSRV build/test
against the `rust-version` declared in `Cargo.toml`, the packaged-file-list
check, and the English-only comment check. If `make pre-push` is not green,
nothing below matters yet — fix that first.

## 2. Automated release-time gate

```
make release-check
```

Checks, from the current `Cargo.toml`/`LICENSE`/`CHANGELOG.md`/package list:

- the version is `1.x` with no pre-release suffix (`-alpha`, `-beta`, `-rc`);
- `Cargo.toml` declares `license = "MIT"`;
- `LICENSE` exists, contains MIT text, and is present in
  `cargo package --list`;
- `CHANGELOG.md` states the relicensing (mentions both MIT and GPL);
- `cargo deny check licenses advisories` and `make package-check` (no
  `.claude/`, `.agents/`, `.codex/`, or `rules/` in the package) both pass.

This target is deliberately **not** part of `pre-push` or CI: asserting "no
pre-release suffix" would fail on every ordinary alpha-stage push. Run it only
when you are actually about to tag.

## 3. Manual gates — not automatable, do not skip

These come from `docs/V1_CODE_REVIEW.md`'s release-only blockers and final
acceptance checklist. None of them are things a script can decide; each needs
a person or the relevant owning agent to sign off.

- [ ] **`license-auditor` reports CLEAN.** Every wire behavior in `src/`
      carries a TLCP specification citation (section 3 of
      `docs/V1_CODE_REVIEW.md`'s classification), no code derives from the
      `legacy-gpl` lineage, and dependency licenses match `deny.toml`'s
      policy. This is a provenance judgment, not a lint.
- [ ] **`architect` gives final validation** on layering and the public
      surface — R-01 through R-04 in the review (protocol/session/client
      boundaries, no transport-adapter type in a public signature).
- [ ] **HTTP streaming and HTTP long polling are implemented and pass the same
      conformance suite as WebSocket, or `docs/adr/0002-all-three-transports-in-1-0-0.md`
      is superseded to rescope `1.0.0` to WebSocket only** (review finding
      L-06). Publishing WebSocket-only against an unsuperseded ADR-0002 is a
      release blocker, not a warning.
- [ ] **Every runtime dependency has a recorded approval** — purpose, selected
      features, MSRV impact, and license conclusion, per `architect` decision
      (review finding L-09). `cargo deny` passing tells you the resolved tree
      is license-compatible *today*; it does not tell you anyone decided the
      dependency belongs here.
- [ ] **The functional, protocol, API, and architecture checklists in
      `docs/V1_CODE_REVIEW.md` section 11 are addressed** (or explicitly
      deferred with a stated reason) — these are the C-xx/P-xx/A-xx/R-xx
      findings, most of them 🔴 Critical.

## 4. Release procedure

Once every gate above is green:

1. Bump `version` in the root `Cargo.toml` to the target `1.x.y`. This crate
   has no other workspace members to update — there is nothing analogous to
   examples-as-separate-crates here.
2. Finalize the `CHANGELOG.md` `[Unreleased]` section: give it the release
   version and date, confirm the licence section still accurately describes
   what shipped.
3. **`make clean` first**, then `make release-check` and `make pre-push`
   against the bumped version (a version bump touches `Cargo.lock`; re-run the
   full gate, not just `release-check`). Verify from a clean build, not an
   incremental one: an incremental `target/` can hide a real failure behind a
   stale artifact, or manufacture one — switching toolchains mid-investigation
   (`cargo +1.85` vs `cargo +1.88` against the same `target/`) produced a
   false-positive error while building the MSRV gate, and it only went away
   after `cargo clean`. Do not let that happen at release time, when it is
   much more expensive to notice.
4. Commit the version bump and changelog finalization.
5. Tag the release: `git tag -a v1.x.y -m "v1.x.y"`.
6. Publish: `cargo login <token>` (never commit or echo the token), then
   `cargo publish --dry-run` first, inspect its output, then `cargo publish`.
7. Push the tag: `git push origin v1.x.y`.
8. Create the GitHub Release from the tag, with the `CHANGELOG.md` section for
   this version as the release body. Call out the MIT relicensing explicitly
   in the release notes, not just in the CHANGELOG.
9. Notify `@daniloaz` that the crate has been rewritten and relicensed
   (courtesy, not obligation — see the note at the end of
   `docs/adr/0001-mit-relicensing-clean-room.md`).

## 5. Never do this

- Never yank or relabel versions ≤ 0.3.3 — they are legitimate GPL-3.0-only
  history, not a mistake to correct.
- Never add IG account credentials, a Lightstreamer demo-server password, or
  any other live-service secret as a CI secret, and never run the
  credentialed examples (`examples/ig_stream.rs` and friends) in CI — they are
  for a human to run locally against a real account.
- Never weaken `deny.toml`'s license allow-list or Clippy's `-D warnings` to
  make a release gate green — fix the underlying issue instead.
