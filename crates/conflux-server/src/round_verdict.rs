//! Remembers whether the last round's batch satisfied its method's
//! citation, so a healthy run says so once instead of every round.
//!
//! The naive version of this remembers the last batch size and speaks
//! when it moves. That is exactly wrong at the scale it matters: with a
//! hundred participants the batch oscillates 100, 99, 100, 98 and every
//! round produces a line — the noise this exists to prevent, wearing the
//! costume of change detection.
//!
//! So it remembers the *verdict*, which is a boolean function of the
//! batch size. A long healthy run logs once, at the first round, and
//! then stays quiet. A line appearing at round 312 means something
//! happened: enough clients left that the batch fell below what the
//! citation covers. Rare, and therefore worth reading.
//!
//! A violation is not governed by any of this — it halts the run at
//! every setting, including `quiet`. What is configurable is how loudly
//! the framework says everything is fine.

use std::sync::Mutex;

use conflux_config::{BatchRequirement, RoundLogDetail};

use crate::AppState;

/// The last verdict this run reported, and the round it was reported at.
#[derive(Default)]
pub struct RoundVerdict {
    /// `None` until the first round that has a requirement to check.
    last: Mutex<Option<Verdict>>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Verdict {
    satisfied: bool,
}

impl RoundVerdict {
    /// Whether the last reported verdict said the guarantee held.
    ///
    /// `None` when nothing has been checked yet — a run whose method
    /// states no batch minimum never records one, and reporting "true"
    /// there would claim a guarantee nobody made.
    pub fn satisfied(&self) -> Option<bool> {
        self.last
            .lock()
            .expect("round verdict mutex poisoned")
            .map(|v| v.satisfied)
    }

    /// Records that `batch` satisfied `req`, logging per the configured
    /// detail level.
    pub fn record_satisfied(
        &self,
        state: &AppState,
        round: u64,
        batch: u32,
        req: &BatchRequirement,
    ) {
        let verdict = Verdict { satisfied: true };
        let previous = {
            let mut last = self.last.lock().expect("round verdict mutex poisoned");
            let previous = *last;
            *last = Some(verdict);
            previous
        };

        match state.config.round_log_detail.value {
            RoundLogDetail::Quiet => {}
            // Only on a transition — including the first round, where
            // there is no previous verdict to be the same as.
            RoundLogDetail::Changes => {
                if previous != Some(verdict) {
                    tracing::info!(
                        round,
                        batch,
                        excluded = req.excluded,
                        required = req.required,
                        citation = req.citation,
                        "batch satisfies the method's cited requirement"
                    );
                }
            }
            RoundLogDetail::Every => {
                tracing::info!(
                    round,
                    batch,
                    excluded = req.excluded,
                    required = req.required,
                    citation = req.citation,
                    "batch satisfies the method's cited requirement"
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_checked_yet_is_not_a_claim_that_the_guarantee_holds() {
        // `fedavg` states no batch minimum, so no verdict is ever
        // recorded — and `/round/status` must not then report `true`,
        // which would assert a guarantee nobody made.
        assert_eq!(RoundVerdict::default().satisfied(), None);
    }
}
