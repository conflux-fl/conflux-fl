//! Aggregation algorithms (family-based): turns one round's batch of
//! client-submitted weight updates into the new global model weights.
//! Every shipped method belongs to one of three families — `averaging`,
//! `robust` (Byzantine-resilient), and `temporal` (cross-round,
//! history-aware) — each built around one small trait capturing what a
//! family member varies, so a new method is typically a short trait impl
//! reusing the family's existing accumulation logic rather than a whole
//! new implementation written from scratch.
//!
//! # Example
//!
//! Config names a method; this crate builds it. `conflux-server` never
//! learns what "krum" is — that is the whole point of the registry
//! (ADR 0002).
//!
//! ```
//! use conflux_core::{AggregatorParams, build_aggregator};
//! use conflux_proto::{ClientDelta, encode_weights};
//!
//! fn delta(id: &str, w: &[f32]) -> ClientDelta {
//!     ClientDelta {
//!         client_id: id.to_string(),
//!         round: 1,
//!         weights: encode_weights(w),
//!         num_samples: 10,
//!         ..Default::default()
//!     }
//! }
//!
//! // Three clients agreeing, and one that is not.
//! let batch = [
//!     delta("a", &[1.0, 1.0, 1.0]),
//!     delta("b", &[1.1, 0.9, 1.0]),
//!     delta("c", &[0.9, 1.1, 1.0]),
//!     delta("attacker", &[50.0, 50.0, 50.0]),
//! ];
//!
//! // FedAvg averages everyone, so the attacker pulls the result.
//! let fedavg = build_aggregator("fedavg", AggregatorParams::default()).unwrap();
//! let out = fedavg.aggregate(&batch).unwrap();
//! assert!(out[0] > 10.0);
//!
//! // Krum selects the single most representative update instead.
//! let krum = build_aggregator("krum", AggregatorParams::default()).unwrap();
//! let out = krum.aggregate(&batch).unwrap();
//! assert!(out[0] < 2.0);
//! ```
//!
//! Hostile input is a typed `Err`, never a panic and never a `NaN` in
//! the checkpoint — see `tests/adversarial_input.rs`:
//!
//! ```
//! # use conflux_core::{AggregatorError, AggregatorParams, build_aggregator};
//! # use conflux_proto::{ClientDelta, encode_weights};
//! # fn delta(id: &str, w: &[f32], n: u64) -> ClientDelta {
//! #     ClientDelta { client_id: id.into(), round: 1, weights: encode_weights(w),
//! #         num_samples: n, ..Default::default() }
//! # }
//! let fedavg = build_aggregator("fedavg", AggregatorParams::default()).unwrap();
//!
//! // A client submitting NaN is named, along with the coordinate.
//! let err = fedavg
//!     .aggregate(&[delta("a", &[1.0], 10), delta("hostile", &[f32::NAN], 10)])
//!     .unwrap_err();
//! assert!(matches!(err, AggregatorError::NonFiniteWeights { .. }));
//!
//! // So is one claiming an impossible sample count. `num_samples` is
//! // unauthenticated, and FedAvg weights by it.
//! let err = fedavg
//!     .aggregate(&[delta("a", &[1.0], 10), delta("liar", &[99.0], u64::MAX)])
//!     .unwrap_err();
//! assert!(matches!(err, AggregatorError::ImplausibleSampleCount { .. }));
//! ```

#![warn(missing_docs)]

mod averaging;
mod flanders;
mod optimization;
mod robust;
mod temporal;
mod trusted;
mod weights;

pub use averaging::{AveragingWeighting, FedAvg, SampleCountWeighting, WeightedAverageAggregator};
pub use flanders::{ClientFlandersDiagnostic, FlandersAggregator};
pub use optimization::{FedOptAggregator, FedOptParams, FedOptVariant};
pub use robust::{
    BulyanFilter, CoordinateWiseAggregator, CoordinateWiseRobustStatistic, DistanceMatrix,
    DivideAndConquerFilter, FabaFilter, FilteredAggregator, GeometricMedianStatistic, KrumFilter,
    MedianOfMeansStatistic, MedianStatistic, MultiKrumFilter, RobustVectorStatistic,
    SelectionResult, TrimmedMeanStatistic, UpdateFilter, VectorRobustAggregator,
};
pub use temporal::{
    CenteredClippingAggregator, ClientDssDiagnostic, DssAggregator, FoolsGoldAggregator,
};
pub use trusted::{FlTrustAggregator, TrustedReference};

use conflux_config::{StrategyEntry, StrategyKind};
use conflux_proto::ClientDelta;

/// Turns one round's batch of client updates into new global weights.
/// Every aggregation family member implements this: `FedAvg` directly;
/// selection-based `robust` members (`Krum`, `Multi-Krum`, `FABA`,
/// `Bulyan`, `Divide-and-Conquer`) via `FilteredAggregator`;
/// coordinate-wise `robust` members (`Trimmed Mean`, `Median`,
/// `Median-of-Means`) via `CoordinateWiseAggregator`; whole-vector
/// `robust` members (`Geometric Median`) via `VectorRobustAggregator`;
/// and history-aware `temporal` members (`FoolsGold`, `Centered
/// Clipping`, and the research-only `DssAggregator`) with their own
/// internal state. Conflux's aim is a
/// faithful, extensible catalog of published methods for researchers to
/// compare against — see each type's own doc comment for its citation;
/// adding another method is a new small trait impl composed with the
/// existing shared accumulators, not a change to this trait or
/// `Aggregator` itself.
pub trait Aggregator: Send + Sync {
    /// Turns one round's batch into the new global weights.
    ///
    /// `&self`, not `&mut self`: one aggregator serves every round behind
    /// an `Arc`, and methods that carry state across rounds (the
    /// `temporal` family) use interior mutability rather than changing
    /// this signature for everyone.
    ///
    /// # Cross-round state: the standing pattern (ADR 0012)
    ///
    /// **A method that needs memory across rounds holds it in a `Mutex`
    /// field on itself. It does not get a `&mut self` signature.** This
    /// was decided rather than defaulted into: `&mut self` would force
    /// every *existing* stateless aggregator behind a
    /// `Box<dyn Aggregator>` to be called through exclusive access too,
    /// which is a change to `conflux-server`'s whole round pipeline (it
    /// treats a boxed aggregator as freely shareable) in exchange for a
    /// capability a minority of methods need.
    ///
    /// Four shipped methods already follow it, and they are the worked
    /// examples to copy from:
    ///
    /// | Method | State it keeps |
    /// |---|---|
    /// | [`FoolsGoldAggregator`] | per-client update history |
    /// | [`CenteredClippingAggregator`] | the running reference vector |
    /// | [`DssAggregator`] | per-client deviation traces |
    /// | (future) FedOpt | first/second-moment estimates |
    ///
    /// Two obligations come with it, both learned the hard way in Tier 6
    /// rather than anticipated:
    ///
    /// - **Validate what you store, not just what you receive.**
    ///   `decode_and_validate` guards the batch in front of you; nothing
    ///   re-checks the state you derived from an earlier one. A finite,
    ///   accepted update drove `CenteredClippingAggregator`'s reference
    ///   to `NaN` permanently, and every later round with it.
    /// - **A stateful method needs cross-round tests.** Single-batch
    ///   tests cannot express "round N poisons round N+1", which is the
    ///   entire failure class statefulness introduces. See
    ///   `tests/stateful_adversarial_input.rs`.
    fn aggregate(&self, updates: &[ClientDelta]) -> Result<Vec<f32>, AggregatorError>;

    /// Whether this method needs a server-computed trusted reference
    /// each round (ADR 0011).
    ///
    /// `false` for every method that reads only the batch, which is all
    /// of them except the `trusted` family. The round pipeline calls
    /// this to decide whether to contact a sidecar at all — so a
    /// deployment running `fedavg` opens no sidecar connection even if
    /// one happens to be configured, and a deployment running `fltrust`
    /// without one fails loudly rather than quietly averaging.
    fn requires_trusted_reference(&self) -> bool {
        false
    }

    /// Supplies this round's trusted reference, before [`Self::aggregate`]
    /// is called.
    ///
    /// A default no-op, so adding the `trusted` family changed nothing
    /// for the twelve methods that ignore it — the additive-extension
    /// rule ADR 0002 exists to protect. `&self` for the same reason
    /// `aggregate` takes it: the aggregator is shared behind an `Arc`,
    /// and a member that stores this uses interior mutability (ADR
    /// 0012), exactly as `FlTrustAggregator` does.
    ///
    /// Split from `aggregate` rather than passed as an argument because
    /// the reference arrives over the network: fetching it is `async`
    /// and `aggregate` is not, so the server does the I/O first and
    /// hands the result across.
    fn set_trusted_reference(&self, _reference: TrustedReference) {}
}

// Registers every shipped, config-selectable family member into
// `conflux-config`'s compile-time strategy registry — `build_aggregator`
// is what actually turns a config-resolved name into a constructed
// `Aggregator`; these submissions are what let `conflux_config::lookup`
// find each name at all, independent of whether anything ever calls
// `build_aggregator`. `DssAggregator` (see `temporal.rs`) deliberately
// has no entry here — it's an unvalidated research method, constructed
// directly by whoever wants to run it, not selectable via a config
// string.
inventory::submit! {
    StrategyEntry { kind: StrategyKind::Aggregator, name: "fedavg" }
}
inventory::submit! {
    StrategyEntry { kind: StrategyKind::Aggregator, name: "fltrust" }
}
inventory::submit! {
    StrategyEntry { kind: StrategyKind::Aggregator, name: "flanders" }
}
inventory::submit! {
    StrategyEntry { kind: StrategyKind::Aggregator, name: "fedyogi" }
}
inventory::submit! {
    StrategyEntry { kind: StrategyKind::Aggregator, name: "fedadam" }
}
inventory::submit! {
    StrategyEntry { kind: StrategyKind::Aggregator, name: "fedadagrad" }
}
inventory::submit! {
    StrategyEntry { kind: StrategyKind::Aggregator, name: "krum" }
}
inventory::submit! {
    StrategyEntry { kind: StrategyKind::Aggregator, name: "multi_krum" }
}
inventory::submit! {
    StrategyEntry { kind: StrategyKind::Aggregator, name: "trimmed_mean" }
}
inventory::submit! {
    StrategyEntry { kind: StrategyKind::Aggregator, name: "median" }
}
inventory::submit! {
    StrategyEntry { kind: StrategyKind::Aggregator, name: "faba" }
}
inventory::submit! {
    StrategyEntry { kind: StrategyKind::Aggregator, name: "bulyan" }
}
inventory::submit! {
    StrategyEntry { kind: StrategyKind::Aggregator, name: "geometric_median" }
}
inventory::submit! {
    StrategyEntry { kind: StrategyKind::Aggregator, name: "median_of_means" }
}
inventory::submit! {
    StrategyEntry { kind: StrategyKind::Aggregator, name: "divide_and_conquer" }
}
inventory::submit! {
    StrategyEntry { kind: StrategyKind::Aggregator, name: "foolsgold" }
}
inventory::submit! {
    StrategyEntry { kind: StrategyKind::Aggregator, name: "centered_clipping" }
}

#[derive(Debug, thiserror::Error)]
/// Why an aggregator name couldn't be turned into an `Aggregator`.
pub enum AggregatorBuildError {
    #[error(
        "unknown aggregator \"{0}\" — not a registered conflux-core strategy \
         (known: \"fedavg\", \"krum\", \"multi_krum\", \"trimmed_mean\", \"median\", \
         \"faba\", \"bulyan\", \"geometric_median\", \"median_of_means\", \
         \"divide_and_conquer\", \"foolsgold\", \"centered_clipping\")"
    )]
    /// The name isn't in the catalog — almost always a typo in a resolved
    /// `aggregator` config value, since the set of names is fixed at
    /// compile time.
    Unknown(String),
}

/// The algorithm-tuning values `build_aggregator` needs, gathered into
/// one struct rather than a growing list of positional `f32`s.
///
/// Each field is read by exactly one family and ignored by the others,
/// so most call sites care about one of them — hence `Default` plus
/// struct-update syntax (`AggregatorParams { clip_radius: 2.0,
/// ..Default::default() }`), which also keeps two same-typed knobs from
/// being silently transposable at a call site the way two bare `f32`
/// arguments would be.
///
/// The defaults match `conflux-config`'s own builtin fallbacks for the
/// corresponding fields, so constructing an aggregator without a
/// resolved config gives the same behavior a config with nothing
/// overridden would.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AggregatorParams {
    /// The assumed fraction of a round's batch that might be Byzantine,
    /// used to size how many updates each method excludes or trims:
    /// Krum's *f*, Multi-Krum's *m*, Trimmed Mean's trim count. Read
    /// only by the `robust` family; `fedavg` ignores it entirely.
    pub byzantine_fraction: f32,
    /// Centered Clipping's `τ` — the radius any one client's deviation
    /// from the running reference is clipped to. Read only by
    /// `centered_clipping`. Problem-scale dependent: see
    /// [`CenteredClippingAggregator`]'s own fidelity notes.
    pub clip_radius: f32,
    /// The `optimization` family's server learning rate `η`, and its
    /// adaptivity floor `τ`. `None` for either means "use the paper's
    /// own default for the selected variant" — which is the right
    /// behavior, because Reddi et al. publish a `τ` (`1e-3`) and
    /// deliberately do not publish an `η`: the server learning rate is
    /// the parameter their whole experimental section sweeps, so a
    /// framework that invented one would be making a recommendation the
    /// literature declines to make.
    ///
    /// Read only by `fedadagrad`/`fedadam`/`fedyogi`; every other method
    /// ignores them, exactly as they ignore `byzantine_fraction`.
    pub server_learning_rate: Option<f32>,
    /// The `optimization` family's `τ`. See
    /// [`Self::server_learning_rate`].
    pub server_tau: Option<f32>,
}

/// Builds a variant's parameters, letting config override the paper's
/// defaults per field rather than all-or-nothing.
fn fedopt_params(variant: FedOptVariant, params: &AggregatorParams) -> FedOptParams {
    let mut p = FedOptParams::for_variant(variant);
    if let Some(eta) = params.server_learning_rate {
        p.server_learning_rate = eta;
    }
    if let Some(tau) = params.server_tau {
        p.tau = tau;
    }
    p
}

impl Default for AggregatorParams {
    fn default() -> Self {
        Self {
            byzantine_fraction: 0.2,
            clip_radius: 1.0,
            server_learning_rate: None,
            server_tau: None,
        }
    }
}

/// Constructs the `Aggregator` named by a resolved `config.aggregator.value`.
/// Every match arm mirrors one `inventory::submit!` above — adding a
/// family member means adding both, not restructuring this function.
///
/// ```
/// use conflux_core::{AggregatorParams, build_aggregator};
///
/// // Parameters are a struct rather than positional arguments, so a
/// // third one is a new field instead of a signature break.
/// let params = AggregatorParams {
///     byzantine_fraction: 0.25,
///     ..Default::default()
/// };
/// assert!(build_aggregator("trimmed_mean", params).is_ok());
///
/// // An unknown name is a typed error, not a panic or a silent default
/// // — a config typo must not quietly become FedAvg.
/// assert!(build_aggregator("fedavgg", AggregatorParams::default()).is_err());
/// ```
pub fn build_aggregator(
    name: &str,
    params: AggregatorParams,
) -> Result<Box<dyn Aggregator>, AggregatorBuildError> {
    let AggregatorParams {
        byzantine_fraction,
        clip_radius,
        ..
    } = params;
    match name {
        "fedavg" => Ok(Box::new(FedAvg::default())),
        "krum" => Ok(Box::new(FilteredAggregator::new(
            KrumFilter { byzantine_fraction },
            FedAvg::default(),
        ))),
        "multi_krum" => Ok(Box::new(FilteredAggregator::new(
            MultiKrumFilter { byzantine_fraction },
            FedAvg::default(),
        ))),
        "trimmed_mean" => Ok(Box::new(CoordinateWiseAggregator::new(
            TrimmedMeanStatistic { byzantine_fraction },
        ))),
        "median" => Ok(Box::new(CoordinateWiseAggregator::new(MedianStatistic))),
        "faba" => Ok(Box::new(FilteredAggregator::new(
            FabaFilter { byzantine_fraction },
            FedAvg::default(),
        ))),
        "bulyan" => Ok(Box::new(FilteredAggregator::new(
            BulyanFilter { byzantine_fraction },
            CoordinateWiseAggregator::new(TrimmedMeanStatistic { byzantine_fraction }),
        ))),
        "geometric_median" => Ok(Box::new(VectorRobustAggregator::new(
            GeometricMedianStatistic::default(),
        ))),
        "median_of_means" => Ok(Box::new(CoordinateWiseAggregator::new(
            MedianOfMeansStatistic::default(),
        ))),
        "divide_and_conquer" => Ok(Box::new(FilteredAggregator::new(
            DivideAndConquerFilter {
                byzantine_fraction,
                ..Default::default()
            },
            FedAvg::default(),
        ))),
        "foolsgold" => Ok(Box::new(FoolsGoldAggregator::default())),
        "centered_clipping" => Ok(Box::new(CenteredClippingAggregator::new(clip_radius))),
        // Selectable by config like any other method, but it is the one
        // entry here that will not run on its own: it refuses to
        // aggregate until the round pipeline injects a trusted reference
        // from a sidecar (ADR 0011). Selecting it without running one is
        // a startup-time failure, not a silent fallback.
        "fltrust" => Ok(Box::new(FlTrustAggregator::new())),
        // FLANDERS is a pre-aggregation *filter*, so it needs something
        // to aggregate what survives. The paper names its own choice —
        // "ϕ = Krum or any other existing robust aggregation heuristic"
        // — and the catalog follows it rather than pairing with `fedavg`.
        //
        // That is not a stylistic preference. `docs/research/` §5.14
        // measured `flanders_fedavg` scoring *worse than undefended
        // FedAvg* against every Sybil attack tested (24.5 vs 17.2 at 20%
        // malicious, 84.0 vs 67.9 at 80%), because a colluder that
        // repeats itself is the easiest client in the batch to forecast
        // and therefore the last one the filter drops. Paired with Krum
        // as the paper specifies, it holds (0.33 on the same rows). A
        // catalog entry that shipped the harmful pairing would be
        // misrepresenting the method.
        "flanders" => Ok(Box::new(FlandersAggregator::new(build_aggregator(
            "krum", params,
        )?))),
        // The `optimization` family (Reddi et al., 2021). Three names,
        // one type: the variants differ in exactly one line of the
        // paper's Algorithm 2, so a discriminant is the honest shape.
        // They read `AggregatorParams::server_learning_rate` and friends
        // rather than the robust family's `byzantine_fraction`.
        "fedadagrad" => Ok(Box::new(FedOptAggregator::with_params(
            FedOptVariant::Adagrad,
            fedopt_params(FedOptVariant::Adagrad, &params),
        ))),
        "fedadam" => Ok(Box::new(FedOptAggregator::with_params(
            FedOptVariant::Adam,
            fedopt_params(FedOptVariant::Adam, &params),
        ))),
        "fedyogi" => Ok(Box::new(FedOptAggregator::with_params(
            FedOptVariant::Yogi,
            fedopt_params(FedOptVariant::Yogi, &params),
        ))),
        other => Err(AggregatorBuildError::Unknown(other.to_string())),
    }
}

#[derive(Debug, thiserror::Error)]
/// Why a batch could not be aggregated.
///
/// Every variant is a rejection, never a repair: an aggregator that
/// quietly fixed up bad input would hide the fact that a client sent
/// it.
pub enum AggregatorError {
    #[error("cannot aggregate an empty batch of updates")]
    /// No updates to aggregate. A round that reached its timeout with no
    /// submissions, usually.
    EmptyBatch,
    /// A client submitted `NaN` or an infinity.
    ///
    /// This is a rejection, not a repair, and it is checked before any
    /// aggregator sees the batch — because the alternative is not
    /// "slightly wrong output". Six of the shipped aggregators sort or
    /// compare their inputs (`partial_cmp` returns `None` for `NaN`) and
    /// **panicked**, taking the server down; the other six propagated
    /// `NaN` into the checkpoint, where it persists forever because every
    /// subsequent round starts from it. Either way a single client, with
    /// four bytes, ends the experiment.
    ///
    /// Nothing on the wire prevents this: `decode_weights` accepts any
    /// 4-byte pattern, as it must — the codec has no way to know which
    /// bit patterns are meaningful for a given model.
    #[error("update {client_id} contains a non-finite weight (NaN or infinity) at index {index}")]
    NonFiniteWeights {
        /// Which client sent it.
        client_id: String,
        /// The first offending coordinate. Reported because a real client
        /// hitting this is usually diverging, and *where* in the parameter
        /// vector tells its operator which layer blew up.
        index: usize,
    },
    /// A client reported a sample count that cannot be a real one.
    ///
    /// `num_samples` is self-reported and unauthenticated, and FedAvg
    /// weights by it directly (McMahan et al. 2017 assumes honest
    /// counts). This check only rejects counts that are *structurally*
    /// impossible — see [`MAX_PLAUSIBLE_SAMPLE_COUNT`]. It is not a
    /// defense against a client that merely exaggerates within
    /// plausible bounds; see that constant's docs for what is.
    #[error(
        "update {client_id} reports {got} samples, which exceeds the largest \
         plausible count ({max}) — self-reported sample counts are \
         unauthenticated and this one cannot be real"
    )]
    ImplausibleSampleCount {
        /// Which client reported it.
        client_id: String,
        /// The count it claimed.
        got: u64,
        /// The largest count treated as possible.
        max: u64,
    },
    #[error(
        "update {client_id} has {got} weights, expected {expected} \
         (every update in a batch must have the same weight-vector length)"
    )]
    /// Updates in one batch disagree about the model's dimension. Two
    /// clients training different architectures, or one that failed to
    /// load the dispatched weights.
    MismatchedLength {
        /// The client whose update is the wrong length.
        client_id: String,
        /// The batch's dimension, taken from its first update.
        expected: usize,
        /// This update's dimension.
        got: usize,
    },
    #[error("update {client_id} has malformed weights: {len} bytes is not a multiple of 4")]
    /// A client's weight buffer isn't a whole number of `f32`s.
    MalformedWeights {
        /// Which client sent it.
        client_id: String,
        /// The buffer's length in bytes, which is not a multiple of 4.
        len: usize,
    },
    /// A `trusted`-family aggregator was asked to run without a
    /// reference (ADR 0011).
    ///
    /// A hard error rather than a fallback, and deliberately so. The
    /// obvious fallback — an unweighted mean — *is* FedAvg, the method
    /// FLTrust exists to replace, and it would be substituted at exactly
    /// the moment the defense was supposed to engage, producing a
    /// plausible aggregate with no indication anything was wrong.
    #[error(
        "no trusted reference was supplied for this round — a trusted-family aggregator \
         cannot run without one, and falling back to an unweighted mean would silently \
         replace the defense with the method it exists to replace (ADR 0011)"
    )]
    MissingTrustedReference,
    /// The supplied trusted reference does not match the batch's model
    /// dimension.
    #[error(
        "trusted reference does not fit this batch: batch has {expected} weights, \
         global model has {global}, reference has {reference}"
    )]
    TrustedReferenceDimension {
        /// The batch's dimension.
        expected: usize,
        /// The supplied global model's dimension.
        global: usize,
        /// The supplied reference's dimension.
        reference: usize,
    },
    #[error("batch weights sum to zero — cannot normalize")]
    /// Every client's weight came out zero, so normalizing would divide
    /// by zero. Reachable when every update reports `num_samples = 0`.
    ZeroWeightSum,
}

#[cfg(test)]
mod tests {
    use super::*;

    const NAMES: &[&str] = &[
        "fedavg",
        "krum",
        "multi_krum",
        "trimmed_mean",
        "median",
        "faba",
        "bulyan",
        "geometric_median",
        "median_of_means",
        "divide_and_conquer",
        "foolsgold",
        "centered_clipping",
    ];

    #[test]
    fn build_aggregator_succeeds_for_every_shipped_name() {
        for &name in NAMES {
            assert!(
                build_aggregator(name, AggregatorParams::default()).is_ok(),
                "{name} failed to build"
            );
        }
    }

    #[test]
    fn build_aggregator_fails_for_an_unknown_name() {
        // `Box<dyn Aggregator>` isn't `Debug`, so `.unwrap_err()` (which
        // needs the `Ok` side to be `Debug` for its panic message) isn't
        // usable here — match directly instead.
        match build_aggregator("does_not_exist", AggregatorParams::default()) {
            Err(AggregatorBuildError::Unknown(name)) => assert_eq!(name, "does_not_exist"),
            Ok(_) => panic!("expected an error, got a constructed Aggregator"),
        }
    }

    /// Catches `inventory::submit!` and `build_aggregator`'s match arms
    /// drifting apart as the family grows — every name one accepts must
    /// also be found by the other.
    #[test]
    fn every_buildable_name_is_also_registry_visible() {
        for &name in NAMES {
            assert!(build_aggregator(name, AggregatorParams::default()).is_ok());
            assert!(
                conflux_config::lookup(StrategyKind::Aggregator, name).is_some(),
                "{name} is buildable but not registry-visible"
            );
        }
    }
}
