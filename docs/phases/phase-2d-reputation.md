# Phase 2d — `conflux-reputation`

## Scope
Contribution scoring, the input to Byzantine-resilience filtering. Ships
`CosineScorer` (plan §10's Phase 2 row). Does **not** build actual Byzantine
*detection*/rejection thresholds tied to a live pipeline (that's
`conflux-server`'s round loop, Phase 5, consuming this crate's scores
alongside `conflux-config`'s `min_reputation_score`), and does not depend on
`conflux-proto::ClientDelta` — per spec §2's dependency graph,
`conflux-reputation` has no internal crate dependency, so scoring works on
plain `&[f32]` weight slices, not the network-level proto type.

## Inputs
- Spec §8 step table: "4. Aggregate | `conflux-privacy` (server-side) →
  `conflux-reputation` → `conflux-core` → `conflux-store`" — reputation
  scoring/filtering happens after server-side privacy transform, before
  aggregation.
- Spec §8 sequence diagram: `Rep.score_update()` is called once per delta in
  the batch, inside the same loop as `Priv.transform_client_delta()`.
- Plan §10, Phase 2 row: "`conflux-reputation` (`CosineScorer`)".
- Explainability principle (ADR
  [0007](../adr/0007-explainable-config-resolution.md)): "`conflux-reputation`
  logs every rejected update with its score and threshold" — this phase
  should log rejections the same way, even though the actual
  threshold-driven rejection loop lives in `conflux-server` (Phase 5); this
  phase provides the `score()` call and a `filter_by_threshold` helper that
  does the logging.

## Deliverables
- `ContributionScorer` trait: `score(&self, update: &[f32], reference: &[f32])
  -> f32`.
- `CosineScorer` — cosine similarity between `update` and `reference` (e.g.
  the mean of the round's updates), in `[-1.0, 1.0]`; a Byzantine/outlier
  update points in a very different direction from the consensus and scores
  low.
- `filter_by_threshold(updates: &[(String, Vec<f32>)], reference: &[f32],
  scorer: &dyn ContributionScorer, min_score: f32) -> Vec<String>` — returns
  the ids that pass, logging each rejection (`client_id`, score, threshold)
  per ADR 0007.

## Test plan
- `CosineScorer`: identical vectors score `1.0`; opposite vectors score
  `-1.0`; orthogonal vectors score `0.0` (up to floating-point tolerance).
- `filter_by_threshold`: updates below `min_score` are excluded from the
  returned ids; updates at/above are included; an update identical to the
  reference always passes a threshold `< 1.0`.

## Definition of done
- [x] `cargo test -p conflux-reputation` passes.
- [x] `cargo build --workspace` and `cargo clippy --workspace --all-targets`
      stay clean.
- [x] `docs/STATUS.md` updated.
