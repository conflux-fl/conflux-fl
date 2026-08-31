//! Shared helpers for the research runners in this directory.
//!
//! Extracted from `run_experiment.rs` when `run_fairness_experiment.rs`
//! needed the same aggregator-name dispatch. Before that,
//! `run_fairness_experiment.rs` called `build_aggregator` directly, which
//! meant **Experiment 2.3 had never measured DSS at all** — every
//! `dss_`/`dsscoll_`/`flanders_` name it might have been given would have
//! failed to resolve. That is why §6.5's fairness question stayed open
//! across three sessions: not because the measurement was hard, but
//! because the harness could not name the thing being measured.
//!
//! A `mod.rs` in a subdirectory rather than a fourth example file: cargo
//! builds `examples/*.rs` and `examples/*/main.rs`, so a directory
//! containing only `mod.rs` is shared code and not a binary of its own.

use conflux_core::{
    Aggregator, AggregatorParams, DssAggregator, FlandersAggregator, build_aggregator,
};

/// `DssAggregator` is deliberately not in `build_aggregator`'s
/// `inventory`-backed catalog (§6.2's doc comment: a research hypothesis,
/// never a framework default) — so a `--aggregator dss_<base>` name (e.g.
/// `dss_fedavg`, `dss_krum`) is handled here instead, wrapping whatever
/// `build_aggregator` constructs for `<base>` in a `DssAggregator`.
///
/// A `dssraw_<base>` name builds DSS with its **original** combine step
/// (`combine_through_base = false`) — the one Finding 3 identified as
/// discarding the base method's own selection. It exists so the fix and
/// the defect can appear as two rows of one experiment.
///
/// Two more prefixes build **ablated** variants of the same wrapper, for
/// Experiment 2.5 (`docs/research/temporal-consistency-aggregation.md`
/// §5.6 — the stability/collusion mechanism ablation §7.3 called for):
/// `dssstab_<base>` sets `collusion_threshold` below any real cosine
/// similarity (`-2.0`, since cosine ∈ [-1, 1]) so the "colluding" half of
/// the AND-gate is always true — a client is penalized on **stability
/// alone**. `dsscoll_<base>` sets `stability_threshold` above any real
/// stability score (`1.5`, since stability ∈ (0, 1]) so the "unstable"
/// half is always true — a client is penalized on **collusion alone**.
/// Both reuse `DssAggregator`'s already-`pub` threshold fields; no change
/// to `DssAggregator` itself was needed to add these two variants.
///
/// Anything without a `dss`-prefixed name is passed straight through to
/// `build_aggregator`.
pub fn build_experiment_aggregator(
    name: &str,
    byzantine_fraction: f32,
    clip_radius: f32,
) -> Box<dyn Aggregator> {
    let build_base = |base_name: &str| {
        build_aggregator(
            base_name,
            AggregatorParams {
                byzantine_fraction,
                clip_radius,
                ..Default::default()
            },
        )
        .unwrap_or_else(|e| panic!("{e}"))
    };
    // `dssraw_<base>` is the pre-Finding-3 combine: DSS uses the base
    // only as a deviation reference and then combines the raw batch
    // itself. Kept selectable so the fix can be measured against what it
    // replaced inside one sweep, rather than by diffing two runs of two
    // different binaries.
    if let Some(base_name) = name.strip_prefix("dssraw_") {
        let mut dss = DssAggregator::new(build_base(base_name));
        dss.combine_through_base = false;
        return Box::new(dss);
    }
    if let Some(base_name) = name.strip_prefix("dssstab_") {
        let mut dss = DssAggregator::new(build_base(base_name));
        dss.collusion_threshold = -2.0;
        return Box::new(dss);
    }
    if let Some(base_name) = name.strip_prefix("dsscoll_") {
        let mut dss = DssAggregator::new(build_base(base_name));
        dss.stability_threshold = 1.5;
        return Box::new(dss);
    }
    // `flanders_<base>`: the closest published prior art to DSS
    // (Gabrielli et al., 2024) over the same base, so the two can be
    // compared on identical batches within one sweep rather than across
    // two runs. Both are cross-round temporal defenses that wrap a base
    // method; that shared shape is exactly what makes the comparison
    // meaningful and exactly what §6.5 has to position against.
    if let Some(base_name) = name.strip_prefix("flanders_") {
        return Box::new(FlandersAggregator::new(build_base(base_name)));
    }
    match name.strip_prefix("dss_") {
        Some(base_name) => Box::new(DssAggregator::new(build_base(base_name))),
        None => build_base(name),
    }
}
