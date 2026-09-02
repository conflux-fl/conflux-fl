//! Post-resolution validation: range checks on single parameters, and
//! combination checks across them.
//!
//! This runs on the **final** [`crate::ResolvedConfig`], deliberately —
//! not inside the profile loader. A bound enforced only at one tier is
//! not a bound: `round_timeout_secs = 0` is exactly as broken arriving
//! from an env var as from a profile file. Validating after resolution
//! catches every tier with one set of rules, and — because every
//! resolved value carries its [`crate::ConfigSource`] — each finding
//! names the tier that supplied the bad value, in the same phrasing the
//! startup log uses. "Where did this come from?" is answered in the
//! error itself.
//!
//! Two severities, on principle:
//!
//! - **Error** — the value is meaningless or guarantees a broken run:
//!   a round that can never wait (`round_timeout_secs = 0`), a cosine
//!   threshold outside cosine's range, a Byzantine fraction no robust
//!   method's cited guarantee survives. The server refuses to start.
//! - **Warning** — legal, but the configuration contradicts itself in a
//!   way the operator should hear about out loud: DP noise configured
//!   to a mathematical no-op, or a quorum small enough that a robust
//!   method's paper-stated `n` requirement fails at quorum-sized
//!   rounds. The server starts, and says so.
//!
//! The rules here are conservative on purpose (ADR 0008's spirit):
//! every bound is either a mathematical fact about the parameter or a
//! requirement the cited paper states — never a taste judgment about
//! what values are "reasonable" for research.

use crate::{ResolvedConfig, source::ConfigSource};

/// How bad a finding is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    /// Meaningless or guaranteed-broken: refuse to start.
    Error,
    /// Legal but self-contradictory: start, and say so.
    Warning,
}

/// One thing wrong (or suspicious) about a resolved configuration.
#[derive(Debug, Clone)]
pub struct Finding {
    /// [`Severity::Error`] or [`Severity::Warning`].
    pub severity: Severity,
    /// The parameter, by its config name.
    pub parameter: &'static str,
    /// The offending value, rendered.
    pub value: String,
    /// Where the value came from — the same phrase the startup log's
    /// `(source: …)` parenthetical uses, so a profile-supplied mistake
    /// names the profile (and its chain link) that made it.
    pub source: String,
    /// Why it is wrong, and what would be right.
    pub message: String,
}

impl std::fmt::Display for Finding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} = {} (from {}): {}",
            self.parameter, self.value, self.source, self.message
        )
    }
}

/// Everything [`ResolvedConfig::validate`] found, split by severity.
///
/// All findings are collected in one pass rather than failing on the
/// first — an operator fixing a config should see the whole list once,
/// not one item per restart.
#[derive(Debug, Default)]
pub struct Validation {
    /// Refuse-to-start findings.
    pub errors: Vec<Finding>,
    /// Start-but-say-so findings.
    pub warnings: Vec<Finding>,
}

impl Validation {
    /// True when nothing at either severity was found.
    pub fn is_clean(&self) -> bool {
        self.errors.is_empty() && self.warnings.is_empty()
    }

    fn push(
        &mut self,
        severity: Severity,
        parameter: &'static str,
        value: impl std::fmt::Display,
        source: &ConfigSource,
        message: impl Into<String>,
    ) {
        let finding = Finding {
            severity,
            parameter,
            value: value.to_string(),
            source: source.text_phrase(),
            message: message.into(),
        };
        match severity {
            Severity::Error => self.errors.push(finding),
            Severity::Warning => self.warnings.push(finding),
        }
    }
}

impl ResolvedConfig {
    /// Checks ranges and cross-parameter combinations, collecting every
    /// finding. See the module docs for the error/warning split and why
    /// this runs post-resolution rather than per tier.
    ///
    /// `conflux-server` calls this at startup — errors refuse to start,
    /// warnings are logged. Embedders constructing configs through
    /// [`crate::resolve`] can call it (or not) on their own terms:
    /// validation is a separate step precisely so a test that *wants* a
    /// hostile configuration can still build one.
    pub fn validate(&self) -> Validation {
        use Severity::{Error, Warning};
        let mut v = Validation::default();

        // ---- single-parameter ranges: mathematical facts ------------

        if self.round_timeout_secs.value == 0 {
            v.push(
                Error,
                "round_timeout_secs",
                0,
                &self.round_timeout_secs.source,
                "a round with a zero timeout can never wait for a submission — must be ≥ 1",
            );
        } else if self.round_timeout_secs.value > 86_400 {
            v.push(
                Warning,
                "round_timeout_secs",
                self.round_timeout_secs.value,
                &self.round_timeout_secs.source,
                "longer than a day; a stalled client would hold the round open that whole \
                 time — legal, but check the units",
            );
        }
        if self.client_registry_ttl.value == 0 {
            v.push(
                Error,
                "client_registry_ttl",
                0,
                &self.client_registry_ttl.source,
                "a zero TTL evicts every client the moment it registers — must be ≥ 1",
            );
        }
        if self.max_update_bytes.value == 0 {
            v.push(
                Error,
                "max_update_bytes",
                0,
                &self.max_update_bytes.source,
                "a zero byte ceiling rejects every submission — must be ≥ 1",
            );
        }
        if let Some(quorum) = &self.quorum
            && quorum.value == 0
        {
            v.push(
                Error,
                "quorum",
                0,
                &quorum.source,
                "a zero quorum flushes rounds with no submissions; leave it unset to \
                 mean \"all active clients\"",
            );
        }
        // Non-finite values are checked explicitly everywhere below —
        // NaN must land in the error branch, never slide past a
        // comparison that is silently false both ways.
        let score = self.min_reputation_score.value;
        if !(-1.0..=1.0).contains(&score) {
            v.push(
                Error,
                "min_reputation_score",
                score,
                &self.min_reputation_score.source,
                "cosine similarity lives in [-1, 1]; a threshold outside it either \
                 admits everything or nothing, silently",
            );
        }
        let fraction = self.robust_byzantine_fraction.value;
        if !(0.0..=1.0).contains(&fraction) {
            v.push(
                Error,
                "robust_byzantine_fraction",
                fraction,
                &self.robust_byzantine_fraction.source,
                "a fraction of the batch — must be within [0, 1] and finite",
            );
        }
        if !self.clip_norm.value.is_finite() || self.clip_norm.value < 0.0 {
            v.push(
                Error,
                "clip_norm",
                self.clip_norm.value,
                &self.clip_norm.source,
                "an L2 norm bound must be finite and non-negative",
            );
        }
        if !self.noise_multiplier.value.is_finite() || self.noise_multiplier.value < 0.0 {
            v.push(
                Error,
                "noise_multiplier",
                self.noise_multiplier.value,
                &self.noise_multiplier.source,
                "a noise standard-deviation multiple must be finite and non-negative",
            );
        }
        if !(self.target_epsilon.value > 0.0 && self.target_epsilon.value.is_finite()) {
            v.push(
                Error,
                "target_epsilon",
                self.target_epsilon.value,
                &self.target_epsilon.source,
                "a privacy budget must be a positive, finite ε",
            );
        }
        if !(self.delta.value > 0.0 && self.delta.value < 1.0) {
            v.push(
                Error,
                "delta",
                self.delta.value,
                &self.delta.source,
                "(ε, δ)-DP requires 0 < δ < 1 — δ ≥ 1 is no guarantee at all",
            );
        }

        // ---- combinations: what the chosen method requires ----------

        let aggregator = self.aggregator.value.as_str();

        if aggregator == "centered_clipping"
            && (!self.clip_radius.value.is_finite() || self.clip_radius.value <= 0.0)
        {
            v.push(
                Error,
                "clip_radius",
                self.clip_radius.value,
                &self.clip_radius.source,
                "centered_clipping's τ must be positive and finite — a negative radius \
                 inverts the clipping instead of bounding it",
            );
        }
        let uses_server_lr = matches!(
            aggregator,
            "fedadagrad" | "fedadam" | "fedyogi" | "fedavgm" | "scaffold"
        );
        if uses_server_lr
            && (!self.server_learning_rate.value.is_finite()
                || self.server_learning_rate.value <= 0.0)
        {
            v.push(
                Error,
                "server_learning_rate",
                self.server_learning_rate.value,
                &self.server_learning_rate.source,
                format!("{aggregator} steps by η × update; η must be positive and finite"),
            );
        }
        if matches!(aggregator, "fedadagrad" | "fedadam" | "fedyogi")
            && (!self.server_tau.value.is_finite() || self.server_tau.value <= 0.0)
        {
            v.push(
                Error,
                "server_tau",
                self.server_tau.value,
                &self.server_tau.source,
                "the adaptivity floor τ must be positive — it is what keeps the \
                 denominator away from zero",
            );
        }
        if aggregator == "fedavgm" && !(0.0..1.0).contains(&self.server_momentum.value) {
            v.push(
                Error,
                "server_momentum",
                self.server_momentum.value,
                &self.server_momentum.source,
                "momentum β must be in [0, 1) — at 1 the buffer never decays and the \
                 model accelerates forever",
            );
        }
        if aggregator == "qfedavg"
            && (!self.fairness_q.value.is_finite() || self.fairness_q.value < 0.0)
        {
            v.push(
                Error,
                "fairness_q",
                self.fairness_q.value,
                &self.fairness_q.source,
                "q < 0 weights *well-served* clients up — the inverse of the method; \
                 q = 0 is exactly FedAvg",
            );
        }
        if aggregator == "zeno" && (!self.zeno_rho.value.is_finite() || self.zeno_rho.value < 0.0) {
            v.push(
                Error,
                "zeno_rho",
                self.zeno_rho.value,
                &self.zeno_rho.source,
                "a negative ρ *rewards* update magnitude in the suspicion score",
            );
        }
        if aggregator == "scaffold" {
            if self.scaffold_num_clients.value == 0 {
                v.push(
                    Error,
                    "scaffold_num_clients",
                    0,
                    &self.scaffold_num_clients.source,
                    "SCAFFOLD's N is the total client population; zero divides the \
                     control-variate update by nothing",
                );
            } else if let Some(quorum) = &self.quorum
                && u64::from(self.scaffold_num_clients.value) < u64::from(quorum.value)
            {
                {
                    v.push(
                        Error,
                        "scaffold_num_clients",
                        self.scaffold_num_clients.value,
                        &self.scaffold_num_clients.source,
                        format!(
                            "N is the total client population and cannot be smaller than \
                             quorum ({}) — a round's minimum batch cannot exceed the \
                             population it is drawn from",
                            quorum.value
                        ),
                    );
                }
            }
        }

        // A Byzantine *majority* is outside every batch-only robust
        // guarantee; FLTrust and Zeno are the exceptions because their
        // anchor never touches the batch (ADR 0011).
        let robust_batch_method = matches!(
            aggregator,
            "krum"
                | "multi_krum"
                | "trimmed_mean"
                | "median"
                | "faba"
                | "bulyan"
                | "geometric_median"
                | "median_of_means"
                | "divide_and_conquer"
        );
        if robust_batch_method && (0.5..=1.0).contains(&fraction) {
            v.push(
                Error,
                "robust_byzantine_fraction",
                fraction,
                &self.robust_byzantine_fraction.source,
                format!(
                    "{aggregator}'s cited guarantee assumes a Byzantine *minority*; at \
                     ≥ 0.5 no batch-only method can distinguish the colluders from the \
                     honest — that regime is what the trusted family (fltrust, zeno) \
                     exists for"
                ),
            );
        }

        // DP configured into a no-op: the noise standard deviation is
        // `noise_multiplier × clip_norm`, so with clip_norm = 0 the
        // configured noise never happens — while the config *reads* as
        // if it does.
        if self.noise_multiplier.value > 0.0 && self.clip_norm.value == 0.0 {
            v.push(
                Warning,
                "noise_multiplier",
                self.noise_multiplier.value,
                &self.noise_multiplier.source,
                "has no effect: noise std is noise_multiplier × clip_norm, and \
                 clip_norm = 0 — this configuration looks private and adds no noise",
            );
        }

        // Paper-stated batch-size requirements, checked at quorum — the
        // smallest batch a round may close with. `b` is what the
        // implementation will actually trim at that size:
        // floor(fraction × n), capped at n − 1.
        if let Some(quorum) = &self.quorum {
            let n = quorum.value as f32;
            if quorum.value > 0 && (0.0..=1.0).contains(&fraction) {
                let b = ((fraction * n).floor() as u64).min(u64::from(quorum.value) - 1);
                let (required, citation): (u64, &str) = match aggregator {
                    // Blanchard, El Mhamdi, Guerraoui & Stainer (2017).
                    "krum" | "multi_krum" => (2 * b + 3, "Krum requires n ≥ 2f + 3"),
                    // El Mhamdi, Guerraoui & Rouault (2018).
                    "bulyan" => (4 * b + 3, "Bulyan requires n ≥ 4f + 3"),
                    // Yin, Chen, Ramchandran & Bartlett (2018): trimming
                    // b from each side must leave something.
                    "trimmed_mean" => (2 * b + 1, "the trimmed mean must keep ≥ 1 value"),
                    _ => (0, ""),
                };
                if required > 0 && u64::from(quorum.value) < required {
                    v.push(
                        Warning,
                        "quorum",
                        quorum.value,
                        &quorum.source,
                        format!(
                            "at n = {n_q} with robust_byzantine_fraction = {fraction}, \
                             {aggregator} trims/excludes f = {b}, and {citation} — \
                             quorum-sized rounds ({n_q} < {required}) run outside the \
                             cited guarantee",
                            n_q = quorum.value,
                        ),
                    );
                }
            }
        }

        v
    }
}

#[cfg(test)]
mod tests {
    use crate::{Mode, Overrides, Topology, resolve};

    fn config(env: Overrides) -> crate::ResolvedConfig {
        resolve(
            Topology::CrossDevice,
            Mode::Research,
            None,
            &env,
            &Overrides::default(),
        )
        .expect("resolution itself cannot fail")
    }

    /// The shipped defaults must validate clean on every axis pair —
    /// a framework whose own defaults trip its own validator has a
    /// deeper problem than validation.
    #[test]
    fn every_builtin_default_is_clean() {
        for topology in Topology::ALL {
            for mode in Mode::ALL {
                let c = resolve(
                    topology,
                    mode,
                    None,
                    &Overrides::default(),
                    &Overrides::default(),
                )
                .unwrap();
                let v = c.validate();
                assert!(
                    v.is_clean(),
                    "{}/{}: {:?} {:?}",
                    topology.label(),
                    mode.label(),
                    v.errors,
                    v.warnings
                );
            }
        }
    }

    /// A range violation is an error, and the finding names the tier
    /// that supplied the value — the whole point of validating after
    /// resolution instead of per tier.
    #[test]
    fn an_out_of_range_value_is_an_error_that_names_its_source() {
        let c = config(Overrides {
            round_timeout_secs: Some(0),
            ..Default::default()
        });
        let v = c.validate();
        assert_eq!(v.errors.len(), 1, "{:?}", v.errors);
        let text = v.errors[0].to_string();
        assert!(text.contains("round_timeout_secs = 0"), "{text}");
        assert!(
            text.contains("env var CONFLUX_ROUND_TIMEOUT_SECS"),
            "the source tier is named: {text}"
        );
    }

    /// NaN must fail range checks, not slip through them — every bound
    /// is written as a negated in-range predicate for exactly this.
    #[test]
    fn a_nan_value_is_out_of_range_not_in_it() {
        let c = config(Overrides {
            clip_norm: Some(f32::NAN),
            ..Default::default()
        });
        assert_eq!(c.validate().errors.len(), 1);
    }

    /// All findings arrive in one pass — an operator fixes the list,
    /// not one item per restart.
    #[test]
    fn multiple_findings_are_collected_not_first_failed() {
        let c = config(Overrides {
            round_timeout_secs: Some(0),
            max_update_bytes: Some(0),
            min_reputation_score: Some(2.0),
            ..Default::default()
        });
        assert_eq!(c.validate().errors.len(), 3);
    }

    /// The combination class: a Byzantine majority is outside every
    /// batch-only robust guarantee, so krum at fraction 0.6 is an
    /// error — while fedavg with the same fraction is not, because
    /// fedavg never reads it.
    #[test]
    fn a_byzantine_majority_is_an_error_only_for_methods_that_promise_otherwise() {
        let bad = config(Overrides {
            aggregator: Some("krum".to_string()),
            robust_byzantine_fraction: Some(0.6),
            ..Default::default()
        });
        assert_eq!(
            bad.validate().errors.len(),
            1,
            "{:?}",
            bad.validate().errors
        );

        let unused = config(Overrides {
            robust_byzantine_fraction: Some(0.6),
            ..Default::default()
        });
        assert!(unused.validate().errors.is_empty());
    }

    /// DP configured into a no-op: noise std is multiplier × clip_norm.
    #[test]
    fn noise_with_zero_clip_norm_warns_that_it_is_a_no_op() {
        let c = config(Overrides {
            noise_multiplier: Some(0.8),
            clip_norm: Some(0.0),
            ..Default::default()
        });
        let v = c.validate();
        assert!(v.errors.is_empty());
        assert_eq!(v.warnings.len(), 1);
        assert!(
            v.warnings[0].message.contains("no effect"),
            "{}",
            v.warnings[0]
        );
    }

    /// Krum's paper-stated n ≥ 2f + 3, checked at quorum. At fraction
    /// 0.3: n = 4 trims f = floor(1.2) = 1 and needs n ≥ 5 — warning;
    /// n = 5 trims f = 1 and 5 ≥ 5 — clean.
    #[test]
    fn krum_quorum_below_the_cited_requirement_warns_with_the_arithmetic() {
        let short = config(Overrides {
            aggregator: Some("krum".to_string()),
            robust_byzantine_fraction: Some(0.3),
            quorum: Some(4),
            ..Default::default()
        });
        let v = short.validate();
        assert_eq!(v.warnings.len(), 1, "{:?}", v.warnings);
        assert!(
            v.warnings[0].message.contains("n ≥ 2f + 3"),
            "{}",
            v.warnings[0]
        );

        let enough = config(Overrides {
            aggregator: Some("krum".to_string()),
            robust_byzantine_fraction: Some(0.3),
            quorum: Some(5),
            ..Default::default()
        });
        assert!(enough.validate().is_clean());
    }

    /// SCAFFOLD's N is the population; smaller than the minimum batch
    /// is impossible by definition.
    #[test]
    fn scaffold_population_smaller_than_quorum_is_an_error() {
        let c = config(Overrides {
            aggregator: Some("scaffold".to_string()),
            scaffold_num_clients: Some(3),
            quorum: Some(5),
            ..Default::default()
        });
        let v = c.validate();
        assert_eq!(v.errors.len(), 1);
        assert!(
            v.errors[0].message.contains("population"),
            "{}",
            v.errors[0]
        );
    }
}
