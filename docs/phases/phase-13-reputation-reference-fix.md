# Phase 13 (draft) — Reputation filtering becomes opt-in

**Status: scoping draft, not started.** Superseded its own first draft
(see "Revision history" at the bottom) after project-owner guidance
clarified Conflux's actual purpose — this version reflects that
correction. Written for review before implementation begins.

## The governing principle (why this draft looks different from the first)

Conflux-fl exists to give researchers **every published aggregation
method implemented faithfully as its own paper defines it** — never
modified or "improved" by the framework — so they're usable as literal
baselines for comparing against that paper's own reported results and
against other published work. The architecture's priority is simplicity
and ease of adding more methods (the family pattern, ADR 0002), not
building the single most defended system possible. Whether a method has
a reputation/trust/filtering component is a property of *that specific
method's own design* (FLTrust and Zeno literally are "aggregation + a
trust mechanism," so that's part of implementing them faithfully) — it
is not something Conflux should impose generically in front of every
aggregator. At deployment time, the user chooses which method — with
whatever robustness properties it actually has — fits their product;
the framework's job is accurate, faithful choices, not a "safe default"
picked on the user's behalf.

**This changes the fix.** The first draft of this brief treated
`conflux-reputation`'s `CosineScorer` as load-bearing default security
infrastructure and tried to make its reference computation more robust.
Under the actual project goal, that framing was wrong: `CosineScorer`
applied as a **mandatory, universal pre-aggregation gate in front of
every aggregator** is itself a Conflux-specific invention no cited paper
(Krum, Trimmed Mean, Median, FedAvg, ...) asks for — and it was
silently modifying every method's behavior by design, which is exactly
what this project doesn't want. A user selecting `krum` should get
literal Krum, matching Blanchard et al.'s own definition, not "Krum
filtered through an uncited heuristic first."

## What was found (still true, reframed)

Both findings from `docs/E2E_TESTING.md`'s "Real findings" are real bugs
in `conflux-reputation`'s current design — the reframing changes *what
kind* of fix they call for, not whether they're real:

1. **Large-magnitude outlier skews the shared batch mean**
   (`round.rs:72`'s `mean_vector`) enough that every honest client gets
   rejected, leaving whichever aggregator is configured nothing honest
   to work with. Reproduced: `krum` and `fedavg` collapsed to identical
   accuracy under the same attack, because reputation had already
   discarded the input either way.
2. **A single non-finite (`NaN`) submission poisons that same mean for
   every client**, not just the degenerate one — found via this
   session's Dirichlet non-IID testing (a zero-sample client shard, no
   attacker involved). Accuracy froze at random-guessing level for the
   entire run.

Neither finding is about Krum, Trimmed Mean, or Median being broken —
Phase 12's application-level tests already proved each of those work
correctly in isolation, and this session's own harness confirmed it
again (`--no-reputation` isolates them and they perform exactly as
their papers claim). **Both findings are entirely `conflux-reputation`'s
own bug**, in a component that shouldn't have been mandatory in front of
those methods to begin with.

## Recommended scope

### 1. Reputation filtering becomes opt-in, off by default

Every aggregator's default behavior should match its cited paper with
zero interference. Concretely: `confluo-config`'s `ResolvedConfig` gains
an explicit `reputation_filter_enabled: bool` (builtin fallback
`false`), independent of the existing `min_reputation_score: f32`
(which keeps its current meaning, used only when the flag is on).
`round.rs` skips the `mean_vector`/`filter_by_threshold` call entirely
when the flag is off — every submitted update goes straight to the
configured aggregator, unmodified, matching that method's own paper.

This alone fixes finding 1 for anyone using Conflux with its actual
defaults: `krum`'s own selection logic (Blanchard et al.'s definition)
runs on the real batch, no upstream filter able to starve it first. It
does *not* claim reputation filtering is bad or should be removed —
deployers who want it for their own product get to opt in explicitly,
same as choosing which aggregator to use. It stays available, just not
imposed.

### 2. Fix the NaN-propagation bug for when reputation *is* enabled

Independent of whether it's on by default: if a deployer opts into
reputation filtering, it shouldn't be crashable-into-uselessness by one
degenerate client. `decode_flushed_deltas` (`round.rs:136`) gains a
non-finite check — a `NaN`/`Inf`-containing update is excluded and
logged (ADR 0007's "say so, out loud" pattern) before it ever reaches
`mean_vector` or `filter_by_threshold`, whether or not the reputation
flag is on. This is a plain correctness bug, not a robustness policy
choice, so it's in scope regardless of finding 1's fix.

**Not doing**: replacing `mean_vector` with a coordinate-wise median
(the first draft's other recommendation). That was solving "make the
mandatory filter more robust" — a problem that stops existing once the
filter stops being mandatory. Keep `mean_vector` as-is; it's simple, and
simple is what this project wants for optional, non-load-bearing
components.

### 3. Methods with their own published trust/filtering mechanism are self-contained aggregators, not `conflux-reputation` extensions

If FLTrust or Zeno are ever prioritized (both still unbuilt), each
implements its *own* cited trust mechanism as part of that specific
aggregator — following the same family-pattern extension process as
every other method (ADR 0002) — not as a change to the shared
`conflux-reputation` module every other aggregator also passes through.
This is smaller-scoped than the first draft's framing (which treated
FLTrust's server-side training requirement as a blocking ADR-0004
conflict needing its own resolution before *anything* in this phase
could proceed): under the corrected scope, `conflux-reputation`'s fix
doesn't need FLTrust resolved at all, since FLTrust was never going to
be built by modifying `conflux-reputation` in the first place. FLTrust's
server-side-training question stays real and stays deferred, but it's
now correctly recognized as *that specific future method's own* scoping
question, not a blocker for this phase.

## Deliverables (once scoping is confirmed)

- `conflux-config`: new `reputation_filter_enabled: bool` field on
  `Overrides`/`ResolvedConfig`, builtin fallback `false`, standard
  `resolve()`/`to_log_lines()` wiring (ADR 0007).
- `conflux-server/src/round.rs`: skip the reputation stage entirely when
  the flag is off; non-finite rejection in `decode_flushed_deltas`
  regardless of the flag.
- `conflux-reputation`: no change to `CosineScorer`/`ContributionScorer`
  — they're already correct for what they do; the bug was in how
  `round.rs` used them unconditionally, not in their own logic.
- `main.rs`: new `CONFLUX_REPUTATION_FILTER_ENABLED` env var (matching
  the existing `overrides_from_env()` pattern).
- `docs/E2E_TESTING.md`'s `run_demo.sh` scripts: the existing
  `--no-reputation` flag (which currently works around the bug by
  setting `min_reputation_score=-1.0`) becomes unnecessary once the
  flag defaults to off — worth simplifying once this lands, not before.

## Test plan (once scoping is confirmed)

- Unit: with `reputation_filter_enabled = false` (the new default), an
  aggregation round with a mix of honest and attacking clients hands
  every submission straight to the configured aggregator — no rejection
  logging at all, confirming the stage is genuinely skipped, not just
  permissive.
- Unit: with the flag explicitly `true`, existing `conflux-reputation`
  tests keep passing unchanged (nothing about `CosineScorer`'s own logic
  changes).
- Unit: a `NaN`/`Inf`-valued update is excluded in `decode_flushed_deltas`
  regardless of the flag's value — reproduces finding 3 as a regression
  test, failing before the fix, passing after.
- Integration (reusing `docs/E2E_TESTING.md`'s harnesses): re-run
  finding 1's scenario (`krum`, `--poison`) against the new default
  (flag off) and confirm accuracy matches the undefended baseline —
  Krum defending correctly with zero special flags needed, unlike the
  first draft's design which still needed `--no-reputation` even after
  a "fix."
- `conflux-config`: standard `resolve()` precedence tests for the new
  field, matching every other `Overrides`-backed parameter's test
  pattern.

## Open questions to resolve before implementation starts

1. Is `reputation_filter_enabled` a topology/mode-profile-owned
   parameter (like `min_reputation_score` currently is, sourced from the
   `cross_device` profile today) or purely an `Overrides`-level toggle
   with no profile ownership (like `robust_byzantine_fraction`)?
   Recommendation: `Overrides`-only — whether to layer an extra,
   uncited filter in front of a chosen method is a deployment policy
   choice, not something a topology (cross-silo vs. cross-device) should
   presume either way.
2. Should enabling `reputation_filter_enabled = true` alongside a
   `robust`-family aggregator log a warning about the known interaction
   risk (finding 1, even after the NaN fix, since a robust batch
   statistic wasn't adopted)? Leaning yes — cheap, and keeps ADR 0007's
   "say so, out loud" principle honest about a real, documented
   limitation, without blocking the deployer's explicit choice.
3. Does this reframing change how `docs/AGGREGATION_LANDSCAPE.md` should
   read? Partially — see its own "Update" section, added alongside this
   revision.

## Revision history

- **2026-08-23, first draft**: scoped around making `conflux-reputation`'s
  reference computation robust (coordinate-wise median) and separately
  fixing non-finite propagation, treating the filter as load-bearing
  default infrastructure. Correct on the mechanics, wrong on the framing.
- **2026-08-23, this revision**: reframed after project-owner guidance
  that Conflux's purpose is a faithful, extensible catalog of published
  methods, not a defended-by-default platform — reputation filtering
  becomes opt-in rather than mandatory-but-more-robust. Smaller change,
  more aligned with the project's actual goal.
