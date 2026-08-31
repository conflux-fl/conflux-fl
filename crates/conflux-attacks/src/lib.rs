//! Simulated known attacks on federated learning, for validating
//! `conflux-core`'s defenses against real published adversaries — not
//! just ad hoc outliers. **Test/dev-only** (ADR 0010): never a
//! `conflux-server` dependency, and depends on `conflux-core` only as a
//! dev-dependency for this crate's own application-level tests
//! (`tests/attack_vs_defense.rs`).
//!
//! See `docs/phases/phase-12-attack-simulation.md` for the full source
//! list, and `docs/EXTENDING.md`'s "Adding a new attack" section for how
//! to add another one.

#![warn(missing_docs)]

mod attacks;
mod stats;

pub use attacks::{
    AdaptiveEvasionAttack, AlieAttack, CorrelatedSybilAttack, GaussianAttack,
    PersistentSybilAttack, ScalingAttack, SignFlippingAttack,
};

use conflux_proto::ClientDelta;

/// What a previous round actually did with an adaptive attacker's
/// submission — the feedback signal `AdaptiveEvasionAttack` (and any
/// future attack in this shape) reacts to. Deliberately built from
/// information available to *any* aggregator regardless of family
/// shape (every `Aggregator` produces a plain `Vec<f32>` result,
/// unlike, say, a `SelectionResult`, which only selection-based methods
/// have) — so this works uniformly across the whole catalog, not just
/// `UpdateFilter` members.
#[derive(Debug, Clone)]
pub struct RoundFeedback {
    /// What the attacker actually submitted last round (its own crafted
    /// update, for comparison against what came out).
    pub previous_submission: Vec<f32>,
    /// The round's actual aggregate result.
    pub previous_aggregate: Vec<f32>,
}

/// An "omniscient" attacker: sees the honest batch a round would
/// otherwise have received, then crafts `num_attackers` malicious
/// updates to inject alongside it — the strongest, most conservative
/// threat model this literature studies (a weaker, non-omniscient
/// attacker can only do worse).
pub trait Attack {
    /// Produces `num_attackers` malicious updates for this round.
    ///
    /// `honest_updates` is visible on purpose: this models an omniscient
    /// adversary that can see the round's honest submissions, which is the
    /// conservative threat model most robustness papers evaluate first.
    /// Returns an empty vector when `num_attackers` is zero.
    fn craft(&self, honest_updates: &[ClientDelta], num_attackers: usize) -> Vec<ClientDelta>;

    /// Like `craft`, but given the chance to react to how the
    /// *previous* round actually went (`None` on the first round, or
    /// whenever a caller doesn't track feedback). Only meaningful for
    /// attacks that adapt round to round; the default implementation
    /// ignores `feedback` and just calls `craft`, so every existing
    /// (stateless, single-round) attack needs no changes at all to keep
    /// working exactly as before — only `AdaptiveEvasionAttack`
    /// overrides this.
    fn craft_adaptive(
        &self,
        honest_updates: &[ClientDelta],
        num_attackers: usize,
        _feedback: Option<&RoundFeedback>,
    ) -> Vec<ClientDelta> {
        self.craft(honest_updates, num_attackers)
    }
}
