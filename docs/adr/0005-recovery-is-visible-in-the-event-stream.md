# ADR-0005 — Recovery and session loss are visible in the event stream

- **Status:** Accepted
- **Date:** 2026-07-20

## Context

TLCP distinguishes an **interrupted stream that can be recovered** — the
session survives, missed data may be re-delivered from a known progressive —
from a session that is **definitively lost** and must be created anew
[`docs/spec/02-session-lifecycle.md` §5, §6]. On re-subscription the server may
or may not re-deliver a snapshot, and the notifications say which happened
[`docs/spec/04-notifications.md` §3].

An application holding derived state (an order book, a chain, a cache) needs
this distinction. After a recovery that preserved continuity it may keep its
state; after a definitive loss followed by a fresh session it must discard and
rebuild. A client that hides both behind a silent "it reconnected" forces every
application to either rebuild state defensively on every hiccup, or to be
subtly wrong after a real one.

## Decision

**Reconnection is automatic; its consequences are not hidden.** The event
stream (ADR-0003) carries explicit variants distinguishing at least:

- the stream was interrupted and **recovered with continuity preserved** — the
  subscription set and the data progressive survived;
- the session was **lost and re-established** — a new session, subscriptions
  re-created, prior continuity gone;
- a subscription's data **restarted from a snapshot**, versus resumed without
  one;
- the session is **definitively gone and will not be retried**, carrying the
  server's cause code and message [`docs/spec/05-error-codes.md`].

The client performs the recovery; the application is told what kind of recovery
it was, in time to act on it.

## Consequences

- Applications can hold derived state correctly, which is the point of the
  distinction existing in the protocol at all.
- The event enum is larger, and callers matching exhaustively must handle
  variants they may not care about. Acceptable: the alternative is a caller
  that is silently wrong after a rebind.
- The session layer must **track and expose** the continuity facts (data
  progressive, subscription identity across recovery) rather than merely using
  them internally — a real constraint on `src/session/*`, not just a naming
  choice.
- Several of the flagged ambiguities in §02 concern exactly when the server
  considers a session recoverable. Those must be resolved empirically
  (ADR-0006) before this surface can be called stable, because getting the
  boundary wrong means telling the application "continuity preserved" when it
  was not — worse than saying nothing.
