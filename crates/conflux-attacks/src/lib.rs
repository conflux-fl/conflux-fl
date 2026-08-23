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

mod attacks;
mod stats;

pub use attacks::{AlieAttack, GaussianAttack, ScalingAttack, SignFlippingAttack};

use conflux_proto::ClientDelta;

/// An "omniscient" attacker: sees the honest batch a round would
/// otherwise have received, then crafts `num_attackers` malicious
/// updates to inject alongside it — the strongest, most conservative
/// threat model this literature studies (a weaker, non-omniscient
/// attacker can only do worse).
pub trait Attack {
    fn craft(&self, honest_updates: &[ClientDelta], num_attackers: usize) -> Vec<ClientDelta>;
}
