//! Collects one round's incoming client updates and decides the instant
//! the round is "done" — either a quorum of submissions has arrived, or a
//! timeout has elapsed, whichever comes first — then hands the collected
//! batch off to aggregation.
//!
//! A round's clients submit concurrently, from independent connections, at
//! unpredictable times, so [`RoundBuffer`] has to stay correct under
//! contention: many tasks pushing at once, and exactly one task waiting to
//! close the batch and read it back out. It also has to make closing the
//! batch airtight — once a snapshot has been handed to an `await_flush`
//! caller, every later push must be rejected explicitly rather than
//! silently landing in a batch nobody will ever read again.

//! # Example
//!
//! A round closes on whichever comes first — quorum or timeout — and
//! says which, because that difference is operationally significant.
//!
//! ```
//! use conflux_buffer::{FlushReason, RoundBuffer};
//! use conflux_proto::{ClientDelta, encode_weights};
//! use std::time::Duration;
//!
//! fn delta(id: &str, round: u64) -> ClientDelta {
//!     ClientDelta {
//!         client_id: id.to_string(),
//!         round,
//!         weights: encode_weights(&[1.0, 2.0]),
//!         num_samples: 10,
//!         ..Default::default()
//!     }
//! }
//!
//! # fn block<F: std::future::Future>(f: F) -> F::Output {
//! #     tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(f)
//! # }
//! # block(async {
//! // Quorum of 2, and 2 arrive: the round closes without waiting out
//! // its timeout, even though the timeout is generous.
//! let buffer = RoundBuffer::new(1, 2);
//! buffer.push(delta("a", 1)).unwrap();
//! buffer.push(delta("b", 1)).unwrap();
//!
//! let flushed = buffer.await_flush(Duration::from_secs(60)).await;
//! assert_eq!(flushed.reason, FlushReason::Quorum);
//! assert_eq!(flushed.deltas.len(), 2);
//! assert_eq!(flushed.round, 1);
//!
//! // Quorum of 5, only 1 arrives: the round still closes, on the
//! // timeout, with a partial batch rather than blocking forever.
//! let buffer = RoundBuffer::new(2, 5);
//! buffer.push(delta("a", 2)).unwrap();
//!
//! // A submission for a round that has moved on is refused rather than
//! // folded into the current one — a slow client resubmitting last
//! // round's work must not corrupt this round's batch.
//! assert!(buffer.push(delta("late", 1)).is_err());
//!
//! let flushed = buffer.await_flush(Duration::from_millis(20)).await;
//! assert_eq!(flushed.reason, FlushReason::Timeout);
//! assert_eq!(flushed.deltas.len(), 1);
//!
//! // A buffer that has already flushed rejects late arrivals rather
//! // than folding them into a round that is over.
//! assert!(buffer.push(delta("straggler", 2)).is_err());
//! # });
//! ```

#![warn(missing_docs)]

use std::sync::Mutex;
use std::time::Duration;

use conflux_proto::ClientDelta;
use tokio::sync::Notify;
use tokio::time::Instant;

#[derive(Debug, thiserror::Error)]
/// Why a push into the round's buffer was refused.
pub enum BufferError {
    #[error("delta is for round {got}, but this buffer is collecting round {expected}")]
    /// The delta is for a different round than this buffer is collecting.
    /// A stale client that trained through a round boundary, most often.
    WrongRound {
        /// The round this buffer is collecting.
        expected: u64,
        /// The round the rejected delta claimed.
        got: u64,
    },
    /// Returned when a push arrives after this buffer has already flushed
    /// — whether because quorum was met or the timeout fired. A buffer
    /// whose snapshot has already been taken and handed to aggregation can
    /// never accept another push, even if a caller elsewhere still holds a
    /// reference to it (for example, across a round retry that hasn't yet
    /// swapped in a fresh buffer for the next attempt). The round itself
    /// isn't necessarily over — the caller should re-fetch the current
    /// task and resubmit against whatever buffer is live now, rather than
    /// treating this as a permanent failure.
    #[error("this round's buffer already flushed; fetch the current task and resubmit")]
    Closed,
}

/// Why a round's batch closed. Worth logging either way — whether a round
/// wrapped up because enough clients responded or because the wait ran out
/// with a partial batch is operationally significant, not an implementation
/// detail to discard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlushReason {
    /// Enough clients submitted; the round closed early rather than waiting out its timeout.
    Quorum,
    /// The wait elapsed with a partial batch. Aggregation proceeds on whoever arrived.
    Timeout,
}

#[derive(Debug, Clone)]
/// One round's collected batch, and why collection stopped.
pub struct FlushResult {
    /// Which round these deltas belong to.
    pub round: u64,
    /// Quorum or timeout. Worth logging either way: whether a round closed
    /// because enough clients responded or because the wait ran out is
    /// operationally significant, not an implementation detail.
    pub reason: FlushReason,
    /// Everything that arrived before the buffer closed. May be shorter
    /// than the quorum when `reason` is `Timeout`, and may be empty —
    /// what to do with an empty round is the caller's decision.
    pub deltas: Vec<ClientDelta>,
}

/// The mutex's contents: still collecting, or already handed a snapshot
/// to some `await_flush` caller. Putting "closed" *inside* the same mutex
/// as the deltas — rather than tracking it with a separate flag next to
/// the `Mutex` — is what makes closing atomic with taking the snapshot: a
/// `push` that acquires the lock either lands before the snapshot (and is
/// correctly included in it) or sees `Closed` already (and errors), with
/// no window where a push can complete after the snapshot was taken but
/// before "closed" became visible to it. A separate flag checked before
/// the lock would leave exactly that window open — a push could pass the
/// flag check, then still race the lock against the snapshot being taken.
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
    /// A buffer collecting for `round`, closing as soon as `quorum`
    /// deltas arrive.
    pub fn new(round: u64, quorum: usize) -> Self {
        Self {
            round,
            quorum,
            state: Mutex::new(BufferState::Open(Vec::new())),
            notify: Notify::new(),
        }
    }

    /// Adds one client's delta to the round.
    ///
    /// `&self`, not `&mut self`: clients submit concurrently from
    /// independent connections, so every caller holds an `Arc` to the same
    /// buffer. Returns `Closed` once a snapshot has been taken, which the
    /// caller should treat as "resubmit against the current round" rather
    /// than a permanent failure.
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
        // sitting unnoticed until the timeout fires. `notify_one`, not
        // `notify_waiters`: the latter only wakes a task that is *already*
        // parked, so a push landing between `await_flush`'s quorum check
        // and its wait would be missed and the round would sit until the
        // timeout. `notify_one` stores a permit when nobody is waiting yet,
        // and the single flusher consumes it on its next `notified()`.
        self.notify.notify_one();
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
            // wait still wakes us: `push` uses `notify_one`, which leaves
            // a permit behind when no waiter is parked yet, so this
            // `notified()` completes immediately and the loop re-checks
            // quorum. A stale permit from a pre-check push costs one
            // spurious iteration, nothing more.
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
            ..Default::default()
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

        // A push landing after the flush must be rejected explicitly —
        // if it were appended to a buffer nobody reads again, the caller
        // would be told `accepted: true` for a submission that silently
        // never counts toward anything.
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

    /// Drives the exact race window that matters for this type: a push
    /// racing against the precise moment quorum is met and the snapshot is
    /// taken. A simpler design (a plain `AtomicBool` "closed" flag
    /// checked before locking, rather than folding "closed" into the
    /// same mutex as the deltas) would let a push in this window land
    /// after the snapshot was already handed off — the client would be
    /// told its submission was accepted, but it would never be read
    /// again. Running many iterations under a real multi-threaded runtime
    /// (rather than relying on load alone, which can pass by luck without
    /// ever actually hitting the interleaving) directly forces that
    /// window and asserts every push either lands in the batch or is
    /// explicitly rejected — never both silently accepted AND absent from
    /// the batch.
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

    /// A quorum-satisfying push that lands *between* `await_flush`'s
    /// quorum check and its wait must still wake it promptly. With a
    /// wakeup that only reaches an already-parked waiter, that push would
    /// go unnoticed and the round would sit until the timeout — a
    /// correctness-preserving but latency-destroying stall. Many
    /// iterations, each with a timeout far longer than the assertion
    /// allows, so a lost wakeup shows up as a stall rather than by luck.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_push_racing_the_wait_does_not_stall_until_the_timeout() {
        for _ in 0..100 {
            let buffer = Arc::new(RoundBuffer::new(1, 1));
            let flush_buffer = Arc::clone(&buffer);
            let flush =
                tokio::spawn(async move { flush_buffer.await_flush(Duration::from_secs(5)).await });
            buffer.push(delta("a", 1)).ok();

            let started = StdInstant::now();
            let result = flush.await.unwrap();
            assert_eq!(result.reason, FlushReason::Quorum);
            assert!(
                started.elapsed() < Duration::from_secs(1),
                "quorum was met, yet the flush waited on the timeout"
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

    /// `RoundBuffer` has no concept of "one submission per client" — it
    /// only checks the round number. Documenting this honestly: a client
    /// that submits twice in the same round (a buggy retry, a duplicate
    /// network delivery, or a client deliberately trying to inflate its
    /// influence over the aggregate) has both deltas land in the batch and
    /// both count toward quorum. Nothing at this layer deduplicates by
    /// `client_id` — a caller that needs "exactly one counted delta per
    /// client" has to enforce that itself, one layer up.
    #[tokio::test]
    async fn push_does_not_deduplicate_repeated_submissions_from_the_same_client() {
        let buffer = RoundBuffer::new(1, 2);

        buffer.push(delta("a", 1)).unwrap();
        buffer.push(delta("a", 1)).unwrap();

        let result = buffer.await_flush(Duration::from_secs(30)).await;

        assert_eq!(result.reason, FlushReason::Quorum);
        assert_eq!(result.deltas.len(), 2);
        assert!(result.deltas.iter().all(|d| d.client_id == "a"));
    }

    /// The same gap under real concurrency rather than sequential calls:
    /// a single client racing itself (duplicate resubmission from two
    /// concurrent tasks) can satisfy quorum entirely on its own, with no
    /// other client ever participating. This isn't a torn read or a lost
    /// update — the mutex is doing its job, every push lands cleanly — it's
    /// the honest limit of what "quorum" means at this layer: a count of
    /// pushes, not a count of distinct clients.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_duplicate_submissions_from_one_client_can_satisfy_quorum_alone() {
        let buffer = Arc::new(RoundBuffer::new(1, 2));

        let mut handles = Vec::new();
        for _ in 0..2 {
            let buffer = Arc::clone(&buffer);
            handles.push(tokio::spawn(async move {
                buffer.push(delta("solo-client", 1)).unwrap();
            }));
        }

        let result = buffer.await_flush(Duration::from_secs(10)).await;

        for handle in handles {
            handle.await.unwrap();
        }

        assert_eq!(result.reason, FlushReason::Quorum);
        assert_eq!(result.deltas.len(), 2);
        assert!(result.deltas.iter().all(|d| d.client_id == "solo-client"));
    }
}
