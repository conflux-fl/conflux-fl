# Phase 2c — `conflux-privacy`

## Scope
Local DP (clip + noise) and epsilon accounting. Ships
`GaussianClippingPrivacy` and an in-memory `RdpAccountant`, `Global` scope
only (ADR 0006 — `PerClient` is Phase 8). Does **not** build tight
subsampled-RDP amplification (see the documented simplification below), nor
`PerClient` accounting itself, nor a durable/Postgres-backed accountant
(Phase 7).

## Inputs (what must already exist)
- Spec §5's exact struct:
  ```rust
  pub struct GaussianClippingPrivacy { pub clip_norm: f32, pub noise_multiplier: f32 }
  ```
  References: Abadi et al. (2016), *Deep Learning with Differential
  Privacy*, ACM CCS; Geyer, Klein & Nabi (2017), *Differentially Private
  Federated Learning: A Client Level Perspective*. Defaults `clip_norm =
  1.0`, `noise_multiplier = 1.0`.
- Spec §6's exact trait and struct:
  ```rust
  pub trait PrivacyAccountant: Send + Sync {
      fn record_round(&mut self, noise_multiplier: f32, sample_rate: f32);
      fn current_epsilon(&self, delta: f64) -> f64;
      fn budget_exhausted(&self, target_epsilon: f64, delta: f64) -> bool;
  }
  pub struct RdpAccountant { rounds: Vec<(f32, f32)> }
  ```
  References: Mironov (2017), *Rényi Differential Privacy*, IEEE CSF; Wang,
  Balle & Kasiviswanathan (2019), *Subsampled Rényi Differential Privacy and
  Analytical Moments Accountant*, AISTATS. Defaults `target_epsilon = 8.0`,
  `delta = 1e-5`.
- ADR [0006](../adr/0006-global-epsilon-accounting.md) — `Global` scope
  only in v1.

## Deliverables
- `GaussianClippingPrivacy::transform(&self, weights: &mut [f32])` (or
  equivalent) — clips `weights` to L2 norm `clip_norm`, then adds i.i.d.
  Gaussian noise with standard deviation `noise_multiplier * clip_norm` to
  each element.
- `RdpAccountant` implementing `PrivacyAccountant`:
  - `record_round` appends `(noise_multiplier, sample_rate)`.
  - `current_epsilon(delta)` composes RDP across all recorded rounds over a
    grid of Rényi orders α, then converts to (ε, δ)-DP via
    `ε(α) = RDP(α) + ln(1/δ)/(α − 1)`, minimized over the grid (both
    formulas from Mironov, 2017).
  - `budget_exhausted(target_epsilon, delta)` = `current_epsilon(delta) >=
    target_epsilon`.
  - **Documented simplification**: per-round RDP is computed for the
    *non-subsampled* Gaussian mechanism (`α / (2·noise_multiplier²)`),
    ignoring `sample_rate`'s privacy-amplification-by-subsampling effect
    from Wang/Balle/Kasiviswanathan (2019) — subsampling only ever
    *tightens* (lowers) true epsilon for `sample_rate < 1`, so this
    accountant reports a conservative upper bound on the true epsilon, never
    an underestimate. Exact subsampled RDP requires numerical-integration
    machinery out of scope for this phase; flag in `docs/STATUS.md`.

## Test plan
- `GaussianClippingPrivacy::transform`: a vector with L2 norm above
  `clip_norm` is scaled down to exactly `clip_norm`; a vector already under
  `clip_norm` is left unscaled (up to the noise added); zero
  `noise_multiplier` output is deterministic (clipping only, no noise) —
  useful to isolate clip-correctness from noise in tests.
- `RdpAccountant`: `current_epsilon` after zero rounds is `0.0` (or
  effectively negligible); more rounds strictly increases epsilon for fixed
  noise; higher `noise_multiplier` strictly decreases epsilon for a fixed
  round count; `budget_exhausted` flips `true` once enough rounds are
  recorded against a small `target_epsilon`.

## Definition of done
- [x] `cargo test -p conflux-privacy` passes.
- [x] `cargo build --workspace` and `cargo clippy --workspace --all-targets`
      stay clean.
- [x] `docs/STATUS.md` updated, including the subsampling-amplification
      simplification above.
