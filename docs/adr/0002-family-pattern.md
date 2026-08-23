# 0002 — Family pattern for aggregation and privacy

## Context
Published FL research produces many variants of the same underlying
mechanism — e.g. FedAvg, FedAvgM, and inverse-loss weighting are all "weighted
averaging" with a different weight function; Krum, Multi-Krum, Trimmed Mean,
and Median are all "robust selection" over a distance matrix. Implementing
each as an independent, unrelated `Aggregator`/`PrivacyEngine` impl would
duplicate the shared accumulation/selection machinery every time.

## Decision
A **family** is shared accumulation/selection logic plus a small trait
capturing only what varies between members of that family:

- `averaging` family: `WeightedAverageAggregator<W: AveragingWeighting>` owns
  the shared accumulation; `AveragingWeighting` captures the weighting rule.
  `FedAvg = WeightedAverageAggregator<SampleCountWeighting>` ships as the one
  member.
- `robust` family: `RobustSelection` trait + shared distance-matrix machinery
  is specified now; zero members ship until Phase 8.

New methods register into `conflux-config`'s compile-time strategy registry
via `inventory::submit!`, selected by config (`aggregator = "fedavg"`)
without any change to `conflux-server`.

## Consequences
- A new averaging variant (e.g. `FedAvgM`) is a ~10-line trait impl, not a
  new `Aggregator` from scratch.
- The `robust` family's shared machinery must be designed before any member
  ships, even though no member ships in v1 — see Phase 8 in spec §10.
- Every new algorithm impl must register via `inventory::submit!` (see
  `CLAUDE.md` conventions) rather than being wired in manually.

## Update (Phase 11a)

When Krum/Multi-Krum/Trimmed Mean/Median actually shipped, the `robust`
family turned out to need **two** shared-accumulator shapes, not one —
`RobustSelection` (as speced above) fits Krum/Multi-Krum (pick a subset
of whole updates, then average them) but misrepresents Trimmed Mean and
Median, which are coordinate-wise and have no "selected whole update"
per client at all. The trait was renamed `UpdateFilter` (paired with
`FilteredAggregator<F: UpdateFilter, C: Aggregator>`) and a second pair —
`CoordinateWiseRobustStatistic` + `CoordinateWiseAggregator<S>` — was
added alongside it, following this ADR's own pattern a second time
within the same family rather than forcing every member through one
shape. See `docs/phases/phase-11a-robust-aggregation.md` for the full
rationale. The registry-registration mechanism itself (`inventory::submit!`,
`build_aggregator`/`build_selector`) was built in Phase 10b, also later
than this ADR anticipated ("Phase 8" in the original text) — and Phase
11b extended it to the `dp` privacy family too, so all three families
this ADR names are now registry-wired, not left as a future concern.
