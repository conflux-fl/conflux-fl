//! What the round loop is actually doing, published so `/health` can
//! report it.
//!
//! The failure this prevents: the round loop runs in its own task, so a
//! `/health` that returned a hardcoded `"ok"` would keep answering
//! affirmatively after the loop stopped — the gRPC and HTTP servers keep
//! serving, the process stays up, and an orchestrator sees a healthy pod
//! doing no work indefinitely, with a single log line as the only
//! evidence anywhere.
//!
//! A health endpoint that cannot report the failure of the thing the
//! process exists to do is not a health endpoint. This module is the shared
//! state that fixes that: the loop writes it, the HTTP handler reads it.

use std::sync::Mutex;
use std::sync::atomic::{AtomicU8, AtomicU32, AtomicU64, Ordering};

/// What the round loop is doing right now.
///
/// Ordered by increasing severity, which is what makes the `u8`
/// representation below meaningful rather than arbitrary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoundLoopState {
    /// Spawned, but no round has completed yet. The normal state of a
    /// server that just booted and is waiting for its first clients.
    Starting,
    /// A round completed successfully most recently.
    Running,
    /// Rounds are failing with retryable errors and the loop is backing
    /// off between attempts. Still alive, still trying — this is the state
    /// a transient backend outage produces.
    Degraded,
    /// The loop has exited and will not run another round without a
    /// process restart. Either a fatal error (an exhausted privacy budget)
    /// or a graceful shutdown.
    Stopped,
}

impl RoundLoopState {
    fn as_u8(self) -> u8 {
        match self {
            RoundLoopState::Starting => 0,
            RoundLoopState::Running => 1,
            RoundLoopState::Degraded => 2,
            RoundLoopState::Stopped => 3,
        }
    }

    fn from_u8(v: u8) -> Self {
        match v {
            0 => RoundLoopState::Starting,
            1 => RoundLoopState::Running,
            2 => RoundLoopState::Degraded,
            _ => RoundLoopState::Stopped,
        }
    }

    /// The lowercase string `/health` reports.
    pub fn as_str(self) -> &'static str {
        match self {
            RoundLoopState::Starting => "starting",
            RoundLoopState::Running => "running",
            RoundLoopState::Degraded => "degraded",
            RoundLoopState::Stopped => "stopped",
        }
    }

    /// Whether a health check should treat this as a live process.
    ///
    /// `Degraded` is deliberately **healthy**: the loop is retrying a
    /// transient failure, and restarting the process would not fix an
    /// unreachable Redis — it would only add a cold start to the outage.
    /// `Stopped` is the state worth acting on, because nothing short of a
    /// restart (or a config change) changes it.
    pub fn is_healthy(self) -> bool {
        !matches!(self, RoundLoopState::Stopped)
    }
}

/// Shared, lock-light health state for the round loop.
///
/// `AtomicU8`/`AtomicU32`/`AtomicU64` rather than one `Mutex` around a
/// struct: `/health` may be polled far more often than rounds complete, and
/// a health check that can be blocked by the thing it is checking is worse
/// than useless. Only `last_error`, which is a `String` and cannot be
/// atomic, takes a lock — and a poisoned lock there degrades to "no detail
/// available" rather than propagating a panic into the health endpoint.
#[derive(Debug)]
pub struct RoundLoopHealth {
    state: AtomicU8,
    consecutive_failures: AtomicU32,
    last_completed_round: AtomicU64,
    last_error: Mutex<Option<String>>,
}

impl Default for RoundLoopHealth {
    fn default() -> Self {
        Self::new()
    }
}

impl RoundLoopHealth {
    /// A freshly-booted loop: `Starting`, no failures, no completed round.
    pub fn new() -> Self {
        Self {
            state: AtomicU8::new(RoundLoopState::Starting.as_u8()),
            consecutive_failures: AtomicU32::new(0),
            last_completed_round: AtomicU64::new(0),
            last_error: Mutex::new(None),
        }
    }

    /// Records a round that completed. Clears the failure streak — the
    /// point of "consecutive" is that one success resets it.
    pub fn record_success(&self, round: u64) {
        self.state
            .store(RoundLoopState::Running.as_u8(), Ordering::SeqCst);
        self.consecutive_failures.store(0, Ordering::SeqCst);
        self.last_completed_round.store(round, Ordering::SeqCst);
        if let Ok(mut slot) = self.last_error.lock() {
            *slot = None;
        }
    }

    /// Records a retryable failure and returns the new consecutive count.
    ///
    /// The count is what the caller turns into a backoff delay, so it is
    /// returned rather than requiring a second read.
    pub fn record_transient_failure(&self, error: &str) -> u32 {
        self.state
            .store(RoundLoopState::Degraded.as_u8(), Ordering::SeqCst);
        if let Ok(mut slot) = self.last_error.lock() {
            *slot = Some(error.to_string());
        }
        self.consecutive_failures.fetch_add(1, Ordering::SeqCst) + 1
    }

    /// Records that the loop has exited for good. `reason` is `None` for a
    /// deliberate shutdown, `Some` for a fatal error.
    pub fn record_stopped(&self, reason: Option<&str>) {
        self.state
            .store(RoundLoopState::Stopped.as_u8(), Ordering::SeqCst);
        if let Ok(mut slot) = self.last_error.lock() {
            *slot = reason.map(str::to_string);
        }
    }

    /// The current state.
    pub fn state(&self) -> RoundLoopState {
        RoundLoopState::from_u8(self.state.load(Ordering::SeqCst))
    }

    /// How many rounds have failed in a row since the last success.
    pub fn consecutive_failures(&self) -> u32 {
        self.consecutive_failures.load(Ordering::SeqCst)
    }

    /// The last round that completed, or `0` if none has.
    pub fn last_completed_round(&self) -> u64 {
        self.last_completed_round.load(Ordering::SeqCst)
    }

    /// The most recent error, if the loop is degraded or stopped because
    /// of one. `None` once a round succeeds again.
    pub fn last_error(&self) -> Option<String> {
        self.last_error.lock().ok().and_then(|slot| slot.clone())
    }
}

/// How long to wait before retrying after `consecutive_failures` in a row.
///
/// Exponential from a 2-second base, capped at 60 seconds. The cap matters
/// more than the curve: an unbounded backoff would eventually stop retrying
/// in practice while still claiming to be trying, which is the same class
/// of dishonesty as a hardcoded `"ok"` health check.
///
/// The shift is bounded before it is applied — `1u64 << 64` is undefined
/// behavior territory in most languages and a panic in debug Rust, and a
/// server that has been failing for a day would otherwise reach it.
pub fn backoff_secs(consecutive_failures: u32) -> u64 {
    const BASE_SECS: u64 = 2;
    const MAX_SECS: u64 = 60;
    let exponent = consecutive_failures.saturating_sub(1).min(8);
    (BASE_SECS << exponent).min(MAX_SECS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fresh_health_is_starting_and_healthy() {
        let health = RoundLoopHealth::new();
        assert_eq!(health.state(), RoundLoopState::Starting);
        assert!(health.state().is_healthy());
        assert_eq!(health.last_completed_round(), 0);
        assert_eq!(health.last_error(), None);
    }

    #[test]
    fn a_success_clears_a_failure_streak() {
        let health = RoundLoopHealth::new();
        health.record_transient_failure("redis unreachable");
        health.record_transient_failure("redis unreachable");
        assert_eq!(health.consecutive_failures(), 2);
        assert_eq!(health.state(), RoundLoopState::Degraded);

        health.record_success(7);
        assert_eq!(health.consecutive_failures(), 0);
        assert_eq!(health.state(), RoundLoopState::Running);
        assert_eq!(health.last_completed_round(), 7);
        assert_eq!(
            health.last_error(),
            None,
            "a recovered loop must not keep reporting the error it recovered from"
        );
    }

    /// The distinction the whole module exists for: retrying is healthy,
    /// having given up is not.
    #[test]
    fn degraded_is_healthy_but_stopped_is_not() {
        let health = RoundLoopHealth::new();
        health.record_transient_failure("postgres reconnecting");
        assert!(
            health.state().is_healthy(),
            "a loop that is still retrying must not be reported as dead — \
             restarting the process would not fix an unreachable backend"
        );

        health.record_stopped(Some("privacy budget exhausted"));
        assert!(!health.state().is_healthy());
        assert_eq!(
            health.last_error().as_deref(),
            Some("privacy budget exhausted")
        );
    }

    #[test]
    fn backoff_grows_then_caps() {
        assert_eq!(backoff_secs(1), 2);
        assert_eq!(backoff_secs(2), 4);
        assert_eq!(backoff_secs(3), 8);
        assert_eq!(backoff_secs(6), 60, "capped, not 64");
        assert_eq!(backoff_secs(100), 60);
        // The overflow case the `.min(8)` clamp exists for.
        assert_eq!(backoff_secs(u32::MAX), 60);
    }

    #[test]
    fn backoff_at_zero_failures_is_still_a_real_delay() {
        // Not reached in the loop (it only backs off after a failure), but
        // a zero here would mean a hot spin if it ever were.
        assert_eq!(backoff_secs(0), 2);
    }
}
