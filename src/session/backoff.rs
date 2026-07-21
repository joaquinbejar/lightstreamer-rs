//! Bounded, jittered reconnection backoff.
//!
//! # Why the numbers here are choices, not protocol
//!
//! The specification gives **no** timing guidance for retries. It says "retry
//! later" for session error codes `5` and `6`, "immediately" for `48`, and
//! nothing at all for anything else; `docs/spec/02-session-lifecycle.md` §6.2
//! flags this explicitly (ambiguity A9), as does
//! `docs/spec/05-error-codes.md` §5.5. Every duration in this module is
//! therefore a defensive default of this crate, documented as such and
//! configurable by the caller — not a value the protocol asks for.
//!
//! Two properties are non-negotiable regardless of the numbers:
//!
//! - **Bounded.** The delay grows to a ceiling and stops there, and the number
//!   of consecutive attempts is capped by default, so a server that is down
//!   does not face an unbounded retry that never surfaces a typed outcome to
//!   the caller.
//! - **Jittered.** Delays are randomized so that a fleet of clients that lost
//!   the same server does not return in lockstep.

use std::num::NonZeroU32;
use std::time::Duration;

/// The default first delay after a failed attempt.
///
/// A choice of this crate, not a spec value: short enough that a transient
/// blip is invisible to the caller, long enough not to hammer a server that
/// dropped the connection deliberately.
const DEFAULT_INITIAL: Duration = Duration::from_millis(500);

/// The default ceiling on the delay.
///
/// A choice of this crate, not a spec value.
const DEFAULT_MAX: Duration = Duration::from_secs(30);

/// The default cap on consecutive failed attempts before the session is
/// reported definitively lost.
///
/// A choice of this crate, not a spec value. With the defaults above, eight
/// attempts span roughly two minutes of retrying before the caller is told the
/// session is gone.
const DEFAULT_MAX_ATTEMPTS: u32 = 8;

/// How reconnection attempts are spaced and how many are allowed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BackoffPolicy {
    /// Delay before the first retry.
    pub(crate) initial: Duration,
    /// Ceiling the delay grows to and does not exceed.
    pub(crate) max: Duration,
    /// How many consecutive failed attempts are allowed before the session is
    /// declared definitively lost. `None` retries indefinitely — permitted,
    /// but then only the per-attempt events tell the caller anything is wrong.
    pub(crate) max_attempts: Option<NonZeroU32>,
}

impl Default for BackoffPolicy {
    fn default() -> Self {
        Self {
            initial: DEFAULT_INITIAL,
            max: DEFAULT_MAX,
            max_attempts: NonZeroU32::new(DEFAULT_MAX_ATTEMPTS),
        }
    }
}

/// A tiny xorshift64\* generator, used only to jitter reconnection delays.
///
/// It exists so that jitter costs no dependency. It is seeded from the
/// standard library's randomized hasher state in production and from a fixed
/// value in tests, which keeps every test in this crate deterministic.
///
/// # Provenance
///
/// This is written from the published academic description of the algorithm,
/// not from any implementation:
///
/// - the xorshift recurrence and the shift triple `(12, 25, 27)` are
///   Marsaglia's, *Xorshift RNGs*, Journal of Statistical Software 8(14), 2003,
///   <https://doi.org/10.18637/jss.v008.i14>;
/// - the `*` variant — multiplying the state by the constant
///   `0x2545F4914F6CDD1D` on output — is Vigna's, *An experimental exploration
///   of Marsaglia's xorshift generators, scrambled*, ACM Transactions on
///   Mathematical Software 42(4), 2016, <https://doi.org/10.1145/2845077>, and
///   its preprint <https://arxiv.org/abs/1402.6246>.
///
/// Both are papers describing a recurrence, published in venues that place no
/// licence on reimplementing what they describe, and neither is a source this
/// crate is forbidden to consult (`docs/adr/0001-mit-relicensing-clean-room.md`
/// governs the *Lightstreamer* implementation, not general numerical
/// literature). The five lines below are the recurrence transcribed from those
/// descriptions; there is no third-party code in them, and the value they
/// produce is used for nothing but choosing a delay inside an interval this
/// module has already decided on.
#[derive(Debug, Clone, Copy)]
struct Jitter(u64);

impl Jitter {
    /// Seeds from the standard library's per-process random state.
    #[must_use]
    fn from_entropy() -> Self {
        use std::hash::{BuildHasher, Hasher, RandomState};
        let seed = RandomState::new().build_hasher().finish();
        Self::from_seed(seed)
    }

    /// Seeds deterministically. Zero is not a valid xorshift state, so it is
    /// mapped to a fixed non-zero constant.
    #[must_use]
    const fn from_seed(seed: u64) -> Self {
        Self(if seed == 0 {
            0x9E37_79B9_7F4A_7C15
        } else {
            seed
        })
    }

    /// Draws the next 64-bit value.
    ///
    /// The shifts and the multiply are the published xorshift64\* recurrence;
    /// see the provenance note on [`Jitter`] for where each is published.
    /// `wrapping_mul` is deliberate here and is not the "wrapping arithmetic"
    /// the coding rules forbid: this is a bit mixer whose modular arithmetic is
    /// the algorithm, not a numeric calculation whose overflow would be a bug.
    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// Draws a duration uniformly in `[0, bound]`.
    fn below(&mut self, bound: Duration) -> Duration {
        let nanos = u64::try_from(bound.as_nanos()).unwrap_or(u64::MAX);
        match nanos.checked_add(1) {
            Some(range) => Duration::from_nanos(self.next_u64() % range),
            None => bound,
        }
    }
}

/// The reconnection schedule for one session.
///
/// Delays follow full jitter: the base doubles up to the policy ceiling, and
/// the actual delay is drawn uniformly from `[0, base]`. A successful bind
/// resets the schedule, so a session that reconnects cleanly never accumulates
/// delay from an unrelated earlier failure.
#[derive(Debug)]
pub(crate) struct Backoff {
    policy: BackoffPolicy,
    /// Consecutive failed attempts since the last reset.
    attempts: u32,
    /// The undithered base delay for the next attempt.
    base: Duration,
    jitter: Jitter,
}

impl Backoff {
    /// Creates a schedule from a policy, seeded from process entropy.
    #[must_use]
    pub(crate) fn new(policy: BackoffPolicy) -> Self {
        Self {
            policy,
            attempts: 0,
            base: policy.initial,
            jitter: Jitter::from_entropy(),
        }
    }

    /// Creates a schedule with a fixed jitter seed, for deterministic tests.
    #[must_use]
    #[cfg(test)]
    pub(crate) fn with_seed(policy: BackoffPolicy, seed: u64) -> Self {
        Self {
            policy,
            attempts: 0,
            base: policy.initial,
            jitter: Jitter::from_seed(seed),
        }
    }

    /// How many consecutive attempts have failed since the last reset.
    #[must_use]
    #[inline]
    pub(crate) const fn attempts(&self) -> u32 {
        self.attempts
    }

    /// Records a failed attempt and returns how long to wait before the next
    /// one, or `None` when the attempt budget is spent and the session must be
    /// reported definitively lost.
    #[must_use]
    pub(crate) fn next_delay(&mut self) -> Option<Duration> {
        self.attempts = self.attempts.checked_add(1)?;
        if let Some(max) = self.policy.max_attempts
            && self.attempts > max.get()
        {
            return None;
        }
        let delay = self.jitter.below(self.base);
        // Double the base for next time, stopping at the ceiling. `checked_mul`
        // rather than `saturating_mul` so an overflow becomes the ceiling
        // explicitly instead of silently.
        self.base = self
            .base
            .checked_mul(2)
            .unwrap_or(self.policy.max)
            .min(self.policy.max);
        Some(delay)
    }

    /// Clears the schedule after a successful bind.
    pub(crate) fn reset(&mut self) {
        self.attempts = 0;
        self.base = self.policy.initial;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy(max_attempts: Option<u32>) -> BackoffPolicy {
        BackoffPolicy {
            initial: Duration::from_millis(500),
            max: Duration::from_secs(4),
            max_attempts: max_attempts.and_then(NonZeroU32::new),
        }
    }

    #[test]
    fn test_backoff_delays_stay_within_the_doubling_base() {
        let mut backoff = Backoff::with_seed(policy(None), 42);
        let bases = [
            Duration::from_millis(500),
            Duration::from_secs(1),
            Duration::from_secs(2),
            Duration::from_secs(4),
            Duration::from_secs(4),
            Duration::from_secs(4),
        ];
        for base in bases {
            match backoff.next_delay() {
                Some(delay) => assert!(delay <= base, "{delay:?} exceeds base {base:?}"),
                None => panic!("an unbounded policy must never exhaust"),
            }
        }
    }

    #[test]
    fn test_backoff_is_capped_by_max_attempts() {
        let mut backoff = Backoff::with_seed(policy(Some(3)), 7);
        assert!(backoff.next_delay().is_some());
        assert!(backoff.next_delay().is_some());
        assert!(backoff.next_delay().is_some());
        assert_eq!(backoff.next_delay(), None);
        assert_eq!(backoff.attempts(), 4);
    }

    #[test]
    fn test_backoff_reset_restores_the_first_delay_and_the_budget() {
        let mut backoff = Backoff::with_seed(policy(Some(2)), 7);
        assert!(backoff.next_delay().is_some());
        assert!(backoff.next_delay().is_some());
        backoff.reset();
        assert_eq!(backoff.attempts(), 0);
        match backoff.next_delay() {
            Some(delay) => assert!(delay <= Duration::from_millis(500)),
            None => panic!("a reset schedule must have budget again"),
        }
    }

    #[test]
    fn test_backoff_is_jittered_not_constant() {
        // Full jitter must actually vary; a schedule that always returned the
        // base would reconnect a fleet in lockstep.
        let mut backoff = Backoff::with_seed(policy(None), 12345);
        let mut seen = std::collections::BTreeSet::new();
        for _ in 0..8 {
            if let Some(delay) = backoff.next_delay() {
                seen.insert(delay);
            }
        }
        assert!(seen.len() > 1, "delays did not vary: {seen:?}");
    }

    #[test]
    fn test_backoff_same_seed_gives_same_schedule() {
        let mut a = Backoff::with_seed(policy(None), 99);
        let mut b = Backoff::with_seed(policy(None), 99);
        for _ in 0..5 {
            assert_eq!(a.next_delay(), b.next_delay());
        }
    }
}
