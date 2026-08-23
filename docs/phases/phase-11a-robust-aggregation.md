# Phase 11a — Redesigned aggregation architecture + the `robust` family

## Scope

Ships spec §5/§10's four `robust` family members — **Krum, Multi-Krum,
Trimmed Mean, Median** — and, per explicit direction, first **redesigns**
`conflux-core`'s aggregation architecture so it composes cleanly for
methods beyond these four, rather than bolting four one-off
implementations onto the existing scaffold.

## Why the existing scaffold needed rethinking, not just filling in

The Phase 4b scaffold's `RobustSelection::select(&self, distances,
updates) -> SelectionResult` assumes every member picks a **subset of
whole client updates**, then something else averages them. That's right
for Krum/Multi-Krum, but Trimmed Mean and Median are inherently
**coordinate-wise** (per weight-vector index, look at every client's
value at that index, trim/median *that*) — there's no "selected whole
update" per client. An earlier draft of this plan proposed two unrelated
generic wrappers, one per shape. That works for exactly these four
methods and stops there — a future method needing *both* shapes at once
(the literature's own next step: **Bulyan**, El Mhamdi, Guerraoui &
Rouault, 2018, *The Hidden Vulnerability of Distributed Learning in
Byzantine Settings*, ICML — which filters via Krum-style scoring, then
combines the survivors with a coordinate-wise trimmed mean) wouldn't fit
either wrapper without new plumbing. That's the concrete signal the
scaffold's shape was wrong, not just incomplete.

## The redesign

Two composable pieces instead of two parallel dead-end ones:

- **`UpdateFilter`** (renamed from `RobustSelection` — the old name
  collided in spirit with `conflux-selector::ClientSelector`, which
  selects *which clients train*, a different pipeline stage entirely
  from *which submitted results to trust*; the rename makes that
  distinction explicit rather than relying on readers to infer it):
  `fn filter(&self, updates: &[ClientDelta]) -> Result<SelectionResult, AggregatorError>`.
  Krum/Multi-Krum implement this; a filter is free to compute its own
  `DistanceMatrix` internally (only the members that need one ever pay
  for it).
- **`FilteredAggregator<F: UpdateFilter, C: Aggregator>`**: filters via
  `F`, hands the survivors to `C` — **any existing `Aggregator`**,
  including `FedAvg` itself — to combine. Krum = `FilteredAggregator<KrumFilter,
  FedAvg>` (filtering to 1 survivor, then "averaging" 1 item is a
  no-op — exactly Krum's own definition: use the single lowest-scoring
  update directly). Multi-Krum = `FilteredAggregator<MultiKrumFilter,
  FedAvg>`. This is the concrete proof the redesign is a real
  generalization, not speculative: **Bulyan becomes
  `FilteredAggregator<BulyanFilter, TrimmedMean>` with zero new
  plumbing**, whenever it's prioritized — nothing here is built *for*
  Bulyan today, but nothing would need to change *in* this module to add
  it later.
- **`CoordinateWiseRobustStatistic`** (new trait) + **`CoordinateWiseAggregator<S>`**
  (new shared accumulator, mirroring `WeightedAverageAggregator<W>`'s own
  shape exactly — ADR 0002's pattern applied a second time within the
  same family): `fn combine(&self, values_at_one_coordinate: &mut [f32]) -> f32`.
  `TrimmedMeanStatistic`/`MedianStatistic` implement this.
- `conflux-core/src/weights.rs` gains `decode_and_validate(updates) ->
  Result<Vec<Vec<f32>>, AggregatorError>` — the decode-every-update +
  check-equal-lengths logic `WeightedAverageAggregator` already had,
  factored out so `CoordinateWiseAggregator` doesn't duplicate it a third
  time; `averaging.rs` is refactored to call it too.

`conflux-selector::ClientSelector` (pre-round client sampling) is
deliberately left untouched and unrelated — it answers "who trains this
round," a decision made before any update exists to filter. `UpdateFilter`
answers "which of the updates that came back do we trust," after the
fact. Keeping them as two distinct traits (rather than merging, or
sharing a name) is the resolution to that naming tension, not an
oversight.

## Methods and their source publications

| Aggregator name | Method | Source |
|---|---|---|
| `krum` | Krum | Blanchard, El Mhamdi, Guerraoui & Stainer (2017), *Machine Learning with Adversaries: Byzantine Tolerant Gradient Descent*, NeurIPS 2017. |
| `multi_krum` | Multi-Krum | Same paper — Multi-Krum keeps the *m* lowest-scoring updates instead of just one. |
| `trimmed_mean` | Coordinate-wise trimmed mean | Yin, Chen, Ramchandran & Bartlett (2018), *Byzantine-Robust Distributed Learning: Towards Optimal Statistical Rates*, ICML 2018, PMLR 80. |
| `median` | Coordinate-wise median | Same paper (Yin et al., 2018) — covers both statistics with matching optimality analysis. |

**Documented modeling choice**: Multi-Krum's combine step uses this
codebase's existing sample-count-weighted mean (`FedAvg`) over the
survivors, not an unweighted arithmetic mean some presentations of
Multi-Krum use — consistent with every other aggregator in this
codebase already weighting by `num_samples`, and worth re-justifying
later only if a specific deployment's data shows it matters (ADR 0008's
"changing a default means re-justifying against the literature," applied
to a choice within a method, not just between methods).

## A new config parameter

**`robust_byzantine_fraction: f32`** (builtin fallback `0.2`; `Overrides`-only,
no topology/mode ownership — an algorithm-tuning value, same category as
`clip_norm`/`noise_multiplier`, not a research-vs-production posture).
Feeds Krum's *f* (`f = floor(fraction * n)`, clamped to `< n`),
Multi-Krum's *m* (`m = n - f`), and Trimmed Mean's per-end trim count
(same `f`, clamped so at least one value survives per coordinate).
Median needs no parameter. Every clamp degrades toward plain averaging
for small batches (documented, tested) rather than erroring — a
`cross_silo` round with 2 active clients can't meaningfully assume "20%
Byzantine," and shouldn't crash over it.

## Deliverables

- `conflux-core/src/weights.rs`: `decode_and_validate`.
- `conflux-core/src/averaging.rs`: refactored to use it.
- `conflux-core/src/robust.rs`: `UpdateFilter`, `FilteredAggregator<F,
  C>`, `KrumFilter`, `MultiKrumFilter`, `CoordinateWiseRobustStatistic`,
  `CoordinateWiseAggregator<S>`, `TrimmedMeanStatistic`,
  `MedianStatistic`. Krum's score sums **squared** distances
  (`distance(i, j).powi(2)`) over the nearest neighbors, matching
  Blanchard et al. exactly — `DistanceMatrix` itself keeps storing plain
  L2 (unchanged, already tested elsewhere).
- `conflux-core::build_aggregator` gains a `byzantine_fraction: f32`
  parameter and four new match arms/`inventory::submit!` pairs
  (`"krum"`, `"multi_krum"`, `"trimmed_mean"`, `"median"`) — a signature
  change from Phase 10b's version, so its callers
  (`conflux-server::app_state.rs`, Phase 10b's own tests) update too.
- `conflux-config`: `robust_byzantine_fraction` added to `Overrides`/
  `ResolvedConfig`/`resolve()`/`to_log_lines()`.

## Test plan

- Numeric tests per method against hand-computable small examples (2–5
  clients, 1–3 dimensions): Krum with one obvious outlier excludes it;
  Trimmed Mean matches a hand-trimmed-and-averaged result; Median matches
  a known middle value; Multi-Krum with `byzantine_fraction = 0`
  degenerates toward plain FedAvg.
- A "poison test" per method: *n* honest updates clustered together plus
  *f* adversarial updates far away (large-magnitude or sign-flipped) —
  assert the aggregate stays close to the honest cluster, not pulled
  toward the attackers. The property actually being bought, tested
  directly, not just the arithmetic.
- Small-batch clamping (`n = 1`, `n = 2`) behaves as documented for every
  method — no panics, degrades toward plain averaging.
- `FilteredAggregator<KrumFilter, FedAvg>` composition itself: a direct
  test proving the generic composes correctly (not just that `"krum"`
  the string happens to work), since that's the redesign's actual claim.
- `conflux-server`: an explicit `aggregator` override per method resolves
  through the registry and completes a real round end-to-end — same
  shape as Phase 10b's `strategy_registry.rs` tests.
- `robust_byzantine_fraction`: standard `resolve()` precedence tests.

## Definition of done
- [x] `cargo test -p conflux-core -p conflux-config -p conflux-server`
      passes, including the poison tests.
- [x] `cargo build --workspace` and `cargo clippy --workspace --all-targets`
      stay clean.
- [x] Every pre-Phase-11 test still passes, updated only where Phase
      10b's `build_aggregator` signature itself changed (documented
      above), never silently.
- [x] `docs/STATUS.md` updated to reflect these as shipped.
      (`docs/ARCHITECTURE.md`/spec's "future" listing left as historical
      record — `STATUS.md` is this project's live source of truth for
      current state, per its own stated role.)

## Outcome

Implemented exactly as redesigned. `conflux-core::weights::decode_and_validate`
factored out and reused by both `averaging.rs` (refactored) and
`robust.rs`'s `CoordinateWiseAggregator`. `UpdateFilter`/
`FilteredAggregator<F, C>` (Krum, Multi-Krum) and
`CoordinateWiseRobustStatistic`/`CoordinateWiseAggregator<S>` (Trimmed
Mean, Median) implemented, each with real numeric tests, poison tests
(honest cluster vs. large-magnitude/sign-flipped attackers — all four
methods keep the aggregate near the honest cluster), and small-batch
(`n=1`, `n=2`) clamping tests. `build_aggregator` gained a
`byzantine_fraction` parameter and four new match arms/`inventory::submit!`
pairs; `conflux-server::app_state.rs`'s one call site updated.

The composability claim — a future method needing both a distance-based
filter and a coordinate-wise combiner composes with zero new plumbing —
is proven directly, not just asserted: `filtered_aggregator_composes_with_a_non_fedavg_combiner`
combines `MultiKrumFilter` with `CoordinateWiseAggregator<MedianStatistic>`
(a combination nothing ships as a named strategy) and confirms it
correctly filters an attacker out before the median combiner ever runs.

`crates/conflux-server/tests/robust_aggregation.rs`: all four new names
resolve through the registry and complete a real round end-to-end,
plus a test confirming `robust_byzantine_fraction` actually reaches
`AppState` via the standard config path.

175 tests passing workspace-wide (was 155 at the end of Phase 10),
stable; `cargo fmt --check` and `cargo clippy --workspace --all-targets`
both clean (one clippy `doc_lazy_continuation` false-positive on a
module doc comment's markdown-like formatting, fixed by rewording, not
suppressed).
