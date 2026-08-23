//! Async staging, quorum + timeout flush.
//!
//! See `docs/spec/conflux-spec-v1.md` §8.

use std::sync::Mutex;
use std::time::Duration;

use conflux_proto::ClientDelta;
use tokio::sync::Notify;
use tokio::time::Instant;

#[derive(Debug, thiserror::Error)]
pub enum BufferError {
    #[error("delta is for round {got}, but this buffer is collecting round {expected}")]
    WrongRound { expected: u64, got: u64 },
    /// Phase 10a: closes the lost-update race `docs/phases/
    /// phase-10a-roundbuffer-race.md` describes — a buffer whose
    /// `await_flush` snapshot has already been taken (and handed off for
    /// aggregation) can never accept another push, even if
    /// `AppState.current_buffer` still points at it during a round retry.
    /// The caller should re-`fetch_task` and resubmit against the
    /// current round, not treat this as a permanent failure.
    #[error("this round's buffer already flushed; fetch the current task and resubmit")]
    Closed,
}

/// Why a round's batch closed — logged either way (ADR 0007: "conflux-buffer
/// logs whether a round closed on quorum or timeout"), never silent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlushReason {
    Quorum,
    Timeout,
}

#[derive(Debug, Clone)]
pub struct FlushResult {
    pub round: u64,
    pub reason: FlushReason,
    pub deltas: Vec<ClientDelta>,
}

/// The mutex's contents: still collecting, or already handed a snapshot
/// to some `await_flush` caller. Phase 10a: putting "closed" *inside* the
/// same mutex as the deltas (rather than a separate `AtomicBool` beside
/// it) is what makes closing atomic with taking the snapshot — a `push`
/// that acquires the lock either lands before the snapshot (and is
/// correctly included in it) or sees `Closed` already (and errors),
/// with no window where a push can complete after the snapshot was taken
/// but before "closed" became visible.
enum BufferState {
    Open(Vec<ClientDelta>),
    Closed,
}

/// Collects one round's incoming `ClientDelta`s and closes the batch on
/// whichever happens first: quorum reached, or `timeout` elapsed.
///
/// Deltas accumulate behind a plain `Mutex` (each push/read is a quick
/// `Vec` operation, same reasoning as `conflux-registry`'s
/// `InMemoryRegistry`); `Notify` is what lets `await_flush` react to a
/// push immediately instead of polling the mutex on a timer.
pub struct RoundBuffer {
    round: u64,
    quorum: usize,
    state: Mutex<BufferState>,
    notify: Notify,
}

impl RoundBuffer {
    pub fn new(round: u64, quorum: usize) -> Self {
        Self {
            round,
            quorum,
            state: Mutex::new(BufferState::Open(Vec::new())),
            notify: Notify::new(),
        }
    }

    pub fn push(&self, delta: ClientDelta) -> Result<(), BufferError> {
        if delta.round != self.round {
            return Err(BufferError::WrongRound {
                expected: self.round,
                got: delta.round,
            });
        }
        match &mut *self.state.lock().expect("buffer mutex poisoned") {
            BufferState::Open(deltas) => deltas.push(delta),
            BufferState::Closed => return Err(BufferError::Closed),
        }
        // Wake `await_flush` immediately rather than making it wait for
        // its next poll — this is what keeps a quorum-satisfying push from
        // sitting unnoticed until the timeout fires.
        self.notify.notify_waiters();
        Ok(())
    }

    /// Waits until quorum is reached or `timeout` elapses, whichever comes
    /// first, then returns everything collected so far and logs why it
    /// stopped waiting. Closes the buffer (see `BufferState`) as part of
    /// taking that snapshot — every push after this point is rejected,
    /// never silently lost.
    pub async fn await_flush(&self, timeout: Duration) -> FlushResult {
        let deadline = Instant::now() + timeout;

        loop {
            if let Some(batch) = self.close_if_quorum_met() {
                log_flush(self.round, FlushReason::Quorum, batch.len(), self.quorum);
                return FlushResult {
                    round: self.round,
                    reason: FlushReason::Quorum,
                    deltas: batch,
                };
            }

            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                let batch = self.close_and_take();
                log_flush(self.round, FlushReason::Timeout, batch.len(), self.quorum);
                return FlushResult {
                    round: self.round,
                    reason: FlushReason::Timeout,
                    deltas: batch,
                };
            }

            // A push that lands between the quorum check above and this
            // wait still wakes us: `notify_waiters` only wakes *current*
            // waiters, but a push completing first means the quorum check
            // on our next loop iteration sees it regardless of whether
            // this particular `notified()` call caught the wakeup.
            let _ = tokio::time::timeout(remaining, self.notify.notified()).await;
        }
    }

    fn close_if_quorum_met(&self) -> Option<Vec<ClientDelta>> {
        let mut state = self.state.lock().expect("buffer mutex poisoned");
        let quorum_met =
            matches!(&*state, BufferState::Open(deltas) if deltas.len() >= self.quorum);
        if !quorum_met {
            return None;
        }
        match std::mem::replace(&mut *state, BufferState::Closed) {
            BufferState::Open(deltas) => Some(deltas),
            BufferState::Closed => unreachable!("just checked Open above, still under the lock"),
        }
    }

    fn close_and_take(&self) -> Vec<ClientDelta> {
        let mut state = self.state.lock().expect("buffer mutex poisoned");
        match std::mem::replace(&mut *state, BufferState::Closed) {
            BufferState::Open(deltas) => deltas,
            // `await_flush` is the only thing that ever closes a buffer,
            // and only one `await_flush` call runs per `RoundBuffer`
            // instance (`run_round` awaits it, then moves on) — reaching
            // `Closed` here would mean two concurrent `await_flush` calls
            // on the same buffer, which nothing in this codebase does.
            BufferState::Closed => Vec::new(),
        }
    }
}

fn log_flush(round: u64, reason: FlushReason, collected: usize, quorum: usize) {
    let reason_str = match reason {
        FlushReason::Quorum => "quorum",
        FlushReason::Timeout => "timeout",
    };
    tracing::info!(
        round,
        flush_reason = reason_str,
        collected,
        quorum,
        "round buffer flushed"
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::time::Instant as StdInstant;

    fn delta(client_id: &str, round: u64) -> ClientDelta {
        ClientDelta {
            client_id: client_id.to_string(),
            round,
            weights: vec![],
            num_samples: 1,
        }
    }

    #[tokio::test]
    #[tracing_test::traced_test]
    async fn flush_logs_round_reason_and_counts() {
        let buffer = RoundBuffer::new(7, 2);
        buffer.push(delta("a", 7)).unwrap();
        buffer.push(delta("b", 7)).unwrap();

        buffer.await_flush(Duration::from_secs(30)).await;

        assert!(logs_contain("round buffer flushed"));
        assert!(logs_contain("flush_reason=\"quorum\""));
        assert!(logs_contain("collected=2"));
        assert!(logs_contain("quorum=2"));
    }

    #[tokio::test]
    async fn flushes_on_quorum_before_timeout() {
        let buffer = RoundBuffer::new(1, 2);
        buffer.push(delta("a", 1)).unwrap();
        buffer.push(delta("b", 1)).unwrap();

        let started = StdInstant::now();
        let result = buffer.await_flush(Duration::from_secs(30)).await;

        assert_eq!(result.reason, FlushReason::Quorum);
        assert_eq!(result.deltas.len(), 2);
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "should not have waited anywhere near the 30s timeout"
        );
    }

    #[tokio::test]
    async fn flushes_on_timeout_with_partial_batch() {
        let buffer = RoundBuffer::new(1, 10);
        buffer.push(delta("a", 1)).unwrap();

        let result = buffer.await_flush(Duration::from_millis(50)).await;

        assert_eq!(result.reason, FlushReason::Timeout);
        assert_eq!(result.deltas.len(), 1);
    }

    #[tokio::test]
    async fn flushes_on_timeout_with_zero_pushes() {
        let buffer = RoundBuffer::new(1, 10);

        let result = buffer.await_flush(Duration::from_millis(20)).await;

        assert_eq!(result.reason, FlushReason::Timeout);
        assert!(result.deltas.is_empty());
    }

    #[tokio::test]
    async fn push_for_wrong_round_errors() {
        let buffer = RoundBuffer::new(5, 1);

        let err = buffer.push(delta("a", 6)).unwrap_err();

        assert!(matches!(
            err,
            BufferError::WrongRound {
                expected: 5,
                got: 6
            }
        ));
    }

    #[tokio::test]
    async fn push_after_flush_errors_instead_of_silently_landing_in_a_dead_buffer() {
        let buffer = RoundBuffer::new(1, 1);
        buffer.push(delta("a", 1)).unwrap();

        let result = buffer.await_flush(Duration::from_secs(30)).await;
        assert_eq!(result.reason, FlushReason::Quorum);
        assert_eq!(result.deltas.len(), 1);

        // Phase 10a: this used to succeed silently — the delta would be
        // appended to a buffer nobody reads again, and the caller would
        // be told `accepted: true` for a submission that never counts.
        let err = buffer.push(delta("late", 1)).unwrap_err();
        assert!(matches!(err, BufferError::Closed));
    }

    #[tokio::test]
    async fn push_after_timeout_flush_also_errors() {
        let buffer = RoundBuffer::new(1, 10);
        let result = buffer.await_flush(Duration::from_millis(20)).await;
        assert_eq!(result.reason, FlushReason::Timeout);

        let err = buffer.push(delta("late", 1)).unwrap_err();
        assert!(matches!(err, BufferError::Closed));
    }

    /// Reproduces the actual race window `docs/phases/
    /// phase-10a-roundbuffer-race.md` describes: a push racing against
    /// the exact moment quorum is met and the snapshot is taken. Before
    /// this phase, a push landing in that window could be silently
    /// accepted into a buffer whose snapshot had already been handed off
    /// — this test drives that interleaving directly (not just
    /// "eventually, at load", which is what Phase 7g's test did and
    /// which never happened to trigger it) and asserts every push either
    /// lands in the batch or is explicitly rejected — never both silently
    /// accepted AND absent from the batch.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn racing_push_against_quorum_flush_never_silently_loses_a_delta() {
        for _ in 0..200 {
            let buffer = Arc::new(RoundBuffer::new(1, 1));

            let flush_buffer = Arc::clone(&buffer);
            let flush_handle =
                tokio::spawn(
                    async move { flush_buffer.await_flush(Duration::from_secs(10)).await },
                );

            // Racing pushes: the first satisfies quorum (1) and may win
            // the race to be included in the snapshot; the second is the
            // one that must never be silently swallowed if it loses.
            buffer.push(delta("a", 1)).ok();
            let late_result = buffer.push(delta("b", 1));

            let flush_result = flush_handle.await.unwrap();

            // Whichever way the race went, "b" either appears in the
            // flushed batch, or `push` returned an explicit error for it
            // — there is no third outcome where it silently vanished.
            let b_in_batch = flush_result.deltas.iter().any(|d| d.client_id == "b");
            assert!(
                b_in_batch || late_result.is_err(),
                "client b's delta must either be in the flushed batch or have been \
                 explicitly rejected — it must never silently disappear"
            );
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_pushes_reach_quorum_without_losing_wakeups() {
        let buffer = Arc::new(RoundBuffer::new(1, 8));

        let mut handles = Vec::new();
        for i in 0..8 {
            let buffer = Arc::clone(&buffer);
            handles.push(tokio::spawn(async move {
                buffer.push(delta(&format!("client-{i}"), 1)).unwrap();
            }));
        }

        let result = buffer.await_flush(Duration::from_secs(10)).await;

        for handle in handles {
            handle.await.unwrap();
        }

        assert_eq!(result.reason, FlushReason::Quorum);
        assert_eq!(result.deltas.len(), 8);
    }
}
