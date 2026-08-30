# Phase 19 — SIMD aggregation

**Status: shipped 2026-08-30 — with the opposite result to the one it assumed. The shared-helper refactor landed; the SIMD did not, because it measured slower. See Outcome.**

## Scope

`conflux-core`'s weighted-sum accumulation is currently a plain scalar
loop (`docs/STATUS.md`'s own tracked deviation:
`accumulator.iter_mut().zip(weights)` in `crates/conflux-core/src/
averaging.rs`, and the structurally identical pattern repeated across
`robust.rs`'s coordinate-wise combine step and `temporal.rs`'s combine
steps). This phase vectorizes the one hot loop every aggregation family
member's combine step reduces to — accumulate-weighted-vectors-into-one
— without changing any aggregation *algorithm's* behavior or output
(bit-for-bit reproducibility across scalar and SIMD paths is the
correctness bar, not "close enough").

**Not in scope**: vectorizing decode/encode (`conflux-proto`'s
`decode_weights`/`encode_weights` — a separate, much smaller cost center,
not flagged as a deviation anywhere) or any of the `robust` family's
*selection* logic (Krum's pairwise distance matrix, DnC's power
iteration) — those are real optimization targets but a different scope
than "the weighted-sum accumulation," which is what every shipped
method's combine step actually shares.

## Why this is one shared change, not eleven per-aggregator ones

Per ADR 0002's family pattern, every shipped aggregation method already
funnels its final combine step through the same handful of accumulation
shapes:

- `averaging.rs`'s `WeightedAverageAggregator<W>` — one weighted-sum loop
  (`FedAvg` and any future `AveragingWeighting` member).
- `robust.rs`'s `CoordinateWiseAggregator<S>` — per-coordinate reduction
  across clients (Trimmed Mean, Median, Median-of-Means).
- `robust.rs`'s `FilteredAggregator<F, A>` — delegates to an inner
  `Aggregator` (usually `WeightedAverageAggregator`) after selection, so
  it inherits whatever the inner aggregator's own accumulation does.
- `temporal.rs`'s `DssAggregator`/`FoolsGoldAggregator` combine steps —
  the same weighted-sum shape as `averaging.rs`, duplicated rather than
  shared today.

A single vectorized `accumulate_weighted(acc: &mut [f32], src: &[f32],
weight: f32)` helper (or a `sum_weighted(sources: &[(&[f32], f32)]) ->
Vec<f32>` batch form) in a new or existing shared module
(`crates/conflux-core/src/weights.rs`, which already holds
`decode_and_validate` as the crate's existing "shared, not
per-family-member" home) covers every one of these call sites in one
change — consistent with this codebase's own stated preference (ADR
0002's doc comment: "common accumulation/selection logic... written
once").

## Inputs

- `crates/conflux-core/src/weights.rs` — the natural new home for the
  helper; already the crate's shared pre-aggregation logic module.
- Every family member's own combine loop (`averaging.rs`,
  `robust.rs`'s `CoordinateWiseAggregator`, `temporal.rs`'s two combine
  steps) — each becomes a caller of the new shared helper instead of its
  own inline loop; no aggregator's own algorithm (weighting scheme,
  filtering, robust statistic) changes.
- `cargo bench`/a criterion-based micro-benchmark harness (not present in
  the workspace today) — needed to actually demonstrate a speedup, since
  "SIMD" without a measured before/after is an unverified claim, and this
  project's own discipline (§5's real-data-only research findings) argues
  against shipping a performance change nobody measured.

## Design decision this brief makes explicit

**Library choice**: the `wide` crate (portable `f32x8`/`f32x4` SIMD
types, stable Rust — no nightly `std::simd`/`portable_simd` needed,
consistent with this workspace having no toolchain pin or nightly
dependency anywhere else) over hand-written `std::arch` intrinsics with
runtime `is_x86_feature_detected!` dispatch. `wide` is simpler to keep
correct (no unsafe blocks, no per-target-arch branching to test), and its
compile-time-selected backend (SSE2 baseline on x86_64, NEON on aarch64)
covers this workspace's realistic deployment targets without the extra
complexity of true runtime CPU-feature dispatch — a legitimate future
upgrade if profiling later shows AVX2's wider registers matter, but not
this phase's starting scope.

`accumulate_weighted` processes `dim` in `f32x8` chunks (`wide::f32x8`),
with a scalar loop over the remainder (`dim % 8` elements) — a standard
SIMD-with-scalar-tail shape, keeping correctness trivial to verify
(differential-test the SIMD path against a plain scalar reference
implementation on every input shape, not just multiples of 8).

## Deliverables

- `wide` added as a `conflux-core` dependency.
- `crates/conflux-core/src/weights.rs`: `pub(crate) fn
  accumulate_weighted(acc: &mut [f32], src: &[f32], weight: f32)` —
  `acc[i] += src[i] * weight` for every `i`, SIMD-chunked internally.
  Debug-asserts `acc.len() == src.len()` (an internal invariant every
  caller already guarantees via `decode_and_validate`, not a new
  user-facing error case).
- Every existing combine-loop call site (`averaging.rs`, `robust.rs`'s
  `CoordinateWiseAggregator`, `temporal.rs`'s two combine steps) rewritten
  to call `accumulate_weighted` in place of its own inline loop — a
  mechanical, behavior-preserving refactor per call site, not a design
  change to any of them.
- A `benches/` directory (criterion) with one benchmark comparing the old
  scalar loop against the new SIMD path at a couple of representative
  `dim` sizes (e.g. 10K and 1M — the range spanning a small logistic-
  regression model up to a small CNN's parameter count, matching the
  existing `e2e_numpy_logreg`/`e2e_pytorch_mnist` example scales) — the
  actual evidence this phase's premise (SIMD is faster here) holds,
  reported honestly even if the speedup turns out smaller than expected
  at small `dim` (SIMD overhead can lose to scalar below some size
  threshold — worth knowing, not worth hiding).

## Test plan

- Differential test: for a range of `dim` values including non-multiples
  of 8 (1, 7, 8, 9, 100, 1000), `accumulate_weighted`'s output is
  bit-identical to a plain scalar reference loop over the same inputs —
  the correctness bar this whole phase rests on.
- Every existing aggregator test in `conflux-core` (all 51+ currently
  passing) continues passing unmodified after the call-site rewrites —
  proof the refactor changed performance characteristics only, not any
  aggregator's actual output.
- Benchmark results recorded in this brief's own "Outcome" section once
  implemented (not asserted in advance) — including the small-`dim`
  case, honestly, even if SIMD doesn't help there.

## Definition of done

- [x] `cargo test -p conflux-core` passes, including the new differential
      tests.
- [x] `cargo bench -p conflux-core` runs and produces a real before/after
      comparison, recorded in this brief.
- [x] `cargo build --workspace` and `cargo clippy --workspace --all-targets`
      stay clean.
- [x] `docs/STATUS.md`'s SIMD deviation bullet updated with the measured
      result.

## Outcome

**The premise did not survive measurement.** This brief assumed
vectorizing the combine step would make it faster. It was built with
`wide` exactly as scoped, benchmarked against the scalar loop it
replaced, and found to be *slower* at every model dimension that matters.
The refactor was kept; the SIMD was removed.

### What was measured

`benches/accumulate.rs` (criterion), single `accumulate_weighted` call,
default target (SSE2 baseline):

| dim | scalar | explicit SIMD | `chunks_exact` SIMD |
|---|---|---|---|
| 8 | 10.1 ns | 5.2 ns | **3.6 ns** |
| 10,000 | **1.21 µs** | 1.35 µs | 1.23 µs |
| 1,000,000 | **145 µs** | 153 µs | 154 µs |

A whole round's combine (10 clients accumulated into one vector):

| dim | scalar | explicit SIMD |
|---|---|---|
| 10,000 | **10.37 µs** | 12.25 µs |
| 1,000,000 | 2.93 ms | 2.82 ms (overlapping intervals) |

SIMD wins only at `dim = 8`, which is not a size any model has. At
10,000 (a logistic-regression-scale model) it is a wash at best; at
1,000,000 (a small CNN) scalar is ~6% ahead.

### Why

Two reasons, both checked rather than assumed:

1. **The loop is memory-bandwidth-bound, not compute-bound.** At
   `dim = 1M` it moves ~12 MB (two reads, one write of 4 MB each) in
   ~145 µs — around 83 GB/s, already at the memory subsystem's limit.
   No amount of arithmetic width helps a loop that is waiting on memory.
2. **LLVM already auto-vectorizes the scalar loop.** Rebuilding with
   `-C target-cpu=native` (AVX2 *and* AVX-512 available on the test
   machine) made the **scalar** loop 2.5× faster at `dim = 8`
   (10.1 ns → 4.1 ns) while leaving the large-`dim` comparison
   unchanged. If explicit SIMD were adding width the compiler wasn't
   already using, that is not the pattern the numbers would show.

The first `wide` implementation was also genuinely inefficient
(`copy_from_slice` into stack arrays per chunk); a `chunks_exact`
rewrite recovered most of that gap. It still did not beat scalar at
realistic sizes, which is what ruled out "the implementation was just
bad" as the explanation.

### What shipped

- **The shared-helper refactor, which was always the other half of this
  phase's value.** `accumulate_weighted(acc, src, weight)` and
  `accumulate_scaled_difference(acc, src, reference, scale)` now live
  once in `weights.rs`, replacing **eight** near-identical inline loops
  across `averaging.rs` (1), `robust.rs` (2), and `temporal.rs` (5).
  That is ADR 0002's "common accumulation logic written once" applied to
  the one thing every family member's combine step shares, and it stands
  on its own regardless of how the loop body is implemented.
- **The benchmark**, kept along with its explicit-SIMD variants — as the
  standing answer to "should we SIMD this?", so the next person to ask
  can run `cargo bench -p conflux-core` instead of re-deriving it.
  `wide` moved from a dependency to a dev-dependency; nothing in `src/`
  uses it.

### Scope correction

The brief listed `robust.rs`'s `CoordinateWiseAggregator` as a call site.
It isn't one: Trimmed Mean, Median, and Median-of-Means gather each
coordinate's values across clients and apply a statistic (a sort, a
selection) per coordinate. That is not a weighted-sum shape and
`accumulate_weighted` does not apply to it. Its two *other* accumulation
loops (FABA's running mean, Divide-and-Conquer's centering mean) are
real call sites and were converted.

### Correctness

The refactor's bar was that no aggregator's output changed. All 62
`conflux-core` tests pass **unmodified**, and `weights.rs` carries
differential tests asserting each helper is *bit-identical* (compared via
`to_bits`, so `+0.0`/`-0.0` and NaN are not glossed over) to the exact
loop it replaced, across lengths 0–1023 and a range of weights. 335 →
338 workspace-wide; `cargo fmt` and `cargo clippy --workspace
--all-targets` clean.
