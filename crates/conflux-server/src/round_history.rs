//! The last N rounds, so a stopped run can be asked what led up to it.
//!
//! `/round/status` answers "what is happening now", which is the wrong
//! tense for the question people actually have when a run halts. A batch
//! that fell below its method's cited requirement at round 312 stops the
//! server and says so — but whether clients bled away gradually or
//! dropped at once is the difference between a capacity problem and an
//! outage, and nothing recorded it.
//!
//! Bounded on purpose. An unbounded history would make a long run's
//! memory a function of how long it has been running, which is a
//! liability in exactly the deployments that run longest. The bound is
//! configurable (`round_history_len`) because how far back is useful
//! depends on how fast rounds go: 128 rounds is hours at cross-silo
//! pace and minutes at cross-device.
//!
//! Nothing here is on the round's critical path — a push under a lock
//! that no other round contends for — but it is deliberately a
//! fixed-size structure rather than a growing one, so the cost stays
//! flat as a run gets long.

use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use conflux_buffer::FlushReason;
use serde::Serialize;

use crate::RoundSummary;

/// One completed round, as it looked when it closed.
#[derive(Debug, Clone, Serialize)]
pub struct RoundRecord {
    /// Which round this describes.
    pub round: u64,
    /// Seconds since the Unix epoch when the round completed. Present so
    /// a reader can see the *rate* rounds were closing at, which is what
    /// distinguishes a slow federation from a stalling one.
    pub completed_at: u64,
    /// `"quorum"` or `"timeout"`. A run that quietly switches from one
    /// to the other has lost participants without failing.
    pub flush_reason: &'static str,
    /// How many clients the selector picked.
    pub selected: usize,
    /// How many submitted before the buffer closed.
    pub submitted: usize,
    /// How many survived reputation and privacy filtering to reach
    /// aggregation.
    pub passed: usize,
    /// Whether this batch satisfied the method's cited requirement.
    ///
    /// `null` when the method states no batch minimum — the same
    /// distinction `/round/status` makes, and for the same reason: "no
    /// claim was made" is not "the claim failed".
    pub cited_requirement_satisfied: Option<bool>,
}

/// A bounded, newest-last record of completed rounds.
pub struct RoundHistory {
    records: Mutex<VecDeque<RoundRecord>>,
    capacity: usize,
}

impl RoundHistory {
    /// A history holding at most `capacity` rounds.
    ///
    /// A capacity of zero disables recording entirely rather than
    /// panicking or silently keeping one — "I do not want this" is a
    /// reasonable thing to configure, and it should cost nothing.
    pub fn new(capacity: usize) -> Self {
        Self {
            records: Mutex::new(VecDeque::with_capacity(capacity.min(1024))),
            capacity,
        }
    }

    /// Records a completed round, evicting the oldest if full.
    pub fn record(&self, summary: &RoundSummary, cited_requirement_satisfied: Option<bool>) {
        if self.capacity == 0 {
            return;
        }
        let record = RoundRecord {
            round: summary.round,
            completed_at: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                // A clock before the epoch is not worth a panic in a
                // logging path; zero reads as "unknown" and the round
                // itself is still recorded.
                .unwrap_or(0),
            flush_reason: match summary.flush_reason {
                FlushReason::Quorum => "quorum",
                FlushReason::Timeout => "timeout",
            },
            selected: summary.num_selected,
            submitted: summary.num_submitted,
            passed: summary.num_passed,
            cited_requirement_satisfied,
        };
        let mut records = self.records.lock().expect("round history mutex poisoned");
        if records.len() == self.capacity {
            records.pop_front();
        }
        records.push_back(record);
    }

    /// The most recent rounds, newest first, at most `limit` of them.
    ///
    /// Newest first because a caller asking about a halt wants the rounds
    /// nearest to it, and should not have to know how many there are to
    /// find them.
    pub fn recent(&self, limit: usize) -> Vec<RoundRecord> {
        self.records
            .lock()
            .expect("round history mutex poisoned")
            .iter()
            .rev()
            .take(limit)
            .cloned()
            .collect()
    }

    /// How many rounds are held.
    pub fn len(&self) -> usize {
        self.records
            .lock()
            .expect("round history mutex poisoned")
            .len()
    }

    /// Whether nothing has been recorded yet.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary(round: u64, submitted: usize) -> RoundSummary {
        RoundSummary {
            round,
            flush_reason: FlushReason::Quorum,
            num_selected: 8,
            num_submitted: submitted,
            num_passed: submitted,
        }
    }

    #[test]
    fn the_oldest_round_is_evicted_once_full() {
        // The bound is the point: a long run's memory must not be a
        // function of how long it has been running.
        let history = RoundHistory::new(3);
        for round in 1..=5 {
            history.record(&summary(round, 8), Some(true));
        }
        assert_eq!(history.len(), 3);
        let rounds: Vec<u64> = history.recent(10).iter().map(|r| r.round).collect();
        // Newest first, and rounds 1 and 2 are gone.
        assert_eq!(rounds, vec![5, 4, 3]);
    }

    #[test]
    fn a_capacity_of_zero_records_nothing() {
        let history = RoundHistory::new(0);
        history.record(&summary(1, 8), Some(true));
        assert!(history.is_empty());
        assert!(history.recent(10).is_empty());
    }

    #[test]
    fn a_limit_larger_than_the_history_is_not_an_error() {
        let history = RoundHistory::new(10);
        history.record(&summary(1, 8), None);
        assert_eq!(history.recent(100).len(), 1);
    }

    #[test]
    fn the_verdict_distinguishes_no_claim_from_a_failed_one() {
        // `None` has to survive the round trip: a method stating no
        // batch minimum must not read as one whose requirement failed.
        let history = RoundHistory::new(4);
        history.record(&summary(1, 8), None);
        history.record(&summary(2, 8), Some(false));
        let recent = history.recent(2);
        assert_eq!(recent[0].cited_requirement_satisfied, Some(false));
        assert_eq!(recent[1].cited_requirement_satisfied, None);
    }
}
