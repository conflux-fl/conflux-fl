# Phase 4b — `conflux-core`

## Scope
The `averaging` aggregation family with its one shipped member, `FedAvg`;
the `robust` family's trait and shared distance machinery, with zero
members shipped (Phase 8 ships Krum/Multi-Krum/Trimmed Mean/Median — ADR
0002). Does **not** build SIMD intrinsics themselves — "SIMD aggregation
algorithms" in the crate's one-line description (spec §2) is the *reason*
this logic lives in Rust rather than Python, not a requirement that Phase 4
hand-write `std::simd`/intrinsics; the weighted-sum accumulation here is
written as a plain, auto-vectorizable loop, and explicit SIMD is a
performance follow-up if profiling ever shows it's needed.

## Inputs (what must already exist)
- Spec §5's exact trait/struct signatures:
  ```rust
  pub trait Aggregator: Send + Sync {
      fn aggregate(&self, updates: &[ClientDelta]) -> Result<Vec<f32>, AggregatorError>;
  }
  pub trait AveragingWeighting: Send + Sync {
      fn weight_for(&self, update: &ClientDelta, batch: &[ClientDelta]) -> f32;
  }
  pub struct WeightedAverageAggregator<W: AveragingWeighting> { weighting: W }
  pub struct SampleCountWeighting; // McMahan et al., 2017 — FedAvg's weighting rule
  pub type FedAvg = WeightedAverageAggregator<SampleCountWeighting>;

  pub trait RobustSelection: Send + Sync {
      fn select(&self, distances: &DistanceMatrix, updates: &[ClientDelta]) -> SelectionResult;
  }
  ```
- `conflux_proto::ClientDelta`'s `weights: Vec<u8>` field — little-endian
  packed `f32`, the same convention `conflux-store`'s `FileStore` already
  uses (Phase 2a) and this phase reuses rather than reinventing.
- ADR [0002](../adr/0002-family-pattern.md) — the family pattern: shared
  accumulation written once, per-member behavior captured in a small
  trait.

## Deliverables
- `AggregatorError` (thiserror): empty batch, and mismatched weight-vector
  lengths across a batch (can't average vectors of different dimension).
- `WeightedAverageAggregator<W: AveragingWeighting>` implementing
  `Aggregator`: decode each update's weights, call `W::weight_for` per
  update, normalize weights to sum to 1 across the batch, accumulate the
  weighted sum per dimension.
- `SampleCountWeighting` (McMahan et al., 2017): `weight_for` returns
  `update.num_samples as f32` — the aggregator's own normalization step
  turns this into the `n_k / Σn_i` FedAvg actually uses, so this impl
  itself stays the ~10-line trait implementation ADR 0002 promises.
- `FedAvg = WeightedAverageAggregator<SampleCountWeighting>`.
- `DistanceMatrix`: pairwise L2 distances between a batch's decoded weight
  vectors — the shared machinery every `robust` family member (Phase 8)
  will consume.
- `RobustSelection` trait and `SelectionResult` (selected indices into the
  batch) — trait only, no member implementation yet.

## Test plan
- `FedAvg`: two updates with equal `num_samples` average to the elementwise
  mean; unequal `num_samples` weight the larger-sample update more heavily
  (assert the result is closer to the larger update than a plain mean
  would be); a single-update batch returns that update's weights
  unchanged.
- `AggregatorError`: empty batch and mismatched-length batches both error
  rather than panicking or silently truncating.
- `DistanceMatrix`: distance from an update to itself is `0.0`; the matrix
  is symmetric; a known 2-point example matches a hand-computed L2
  distance.

## Definition of done
- [x] `cargo test -p conflux-core` passes.
- [x] `cargo build --workspace` and `cargo clippy --workspace --all-targets`
      stay clean.
- [x] `docs/STATUS.md` updated.
