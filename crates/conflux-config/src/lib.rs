//! Layered config, topology/mode profiles, strategy registry.
//!
//! See `docs/spec/conflux-spec-v1.md` §4.

mod registry;
mod source;
mod types;

pub use registry::{StrategyEntry, StrategyKind, lookup};
pub use source::ConfigSource;
pub use types::{
    AccountingScope, AuthMode, BudgetExhaustedAction, ConnectionMode, LogFormat, Mode,
    ModeDefaults, SeedMode, Topology, TopologyDefaults,
};

use source::{LoggedValue, log_line};

/// A resolved config value paired with where it came from.
///
/// Generic over the value's type `T` so the same struct represents a
/// resolved `bool`, `f64`, or `ConnectionMode` alike — `ResolvedConfig`
/// below is nineteen fields of `Resolved<SomeType>`, not nineteen
/// hand-rolled "value + source" pairs.
#[derive(Debug, Clone, PartialEq)]
pub struct Resolved<T> {
    pub value: T,
    pub source: ConfigSource,
}

/// One override tier's worth of parameters (spec §9's full parameter list).
/// `file`, `env`, and `cli` in [`resolve`] are each an `Overrides` — same
/// shape, different precedence.
#[derive(Debug, Default, Clone)]
pub struct Overrides {
    pub connection_mode: Option<ConnectionMode>,
    pub auth: Option<AuthMode>,
    pub round_timeout_secs: Option<u64>,
    pub min_reputation_score: Option<f32>,
    pub client_registry_ttl: Option<u64>,
    pub quorum: Option<u32>,
    pub selector: Option<String>,
    pub seed_mode: Option<SeedMode>,
    pub seed_value: Option<u64>,
    pub aggregator: Option<String>,
    /// Phase 11a: the assumed fraction of Byzantine clients in a round's
    /// batch — feeds `robust` family members (Krum's *f*, Multi-Krum's
    /// *m*, Trimmed Mean's trim count). An algorithm-tuning value, same
    /// category as `clip_norm`/`noise_multiplier`, not a
    /// research-vs-production posture.
    pub robust_byzantine_fraction: Option<f32>,
    pub privacy_mechanism: Option<String>,
    pub clip_norm: Option<f32>,
    pub noise_multiplier: Option<f32>,
    pub target_epsilon: Option<f64>,
    pub delta: Option<f64>,
    pub budget_exhausted_action: Option<BudgetExhaustedAction>,
    pub accounting_scope: Option<AccountingScope>,
    pub allow_stub_client: Option<bool>,
    pub require_node_auth: Option<bool>,
    pub config_log_format: Option<LogFormat>,
}

/// Every parameter from spec §9, resolved against a topology + mode, with
/// its `ConfigSource` attached (ADR 0007 — this is what makes resolution
/// explainable rather than just a merged bag of values).
#[derive(Debug, Clone)]
pub struct ResolvedConfig {
    pub connection_mode: Resolved<ConnectionMode>,
    pub auth: Resolved<AuthMode>,
    pub round_timeout_secs: Resolved<u64>,
    pub min_reputation_score: Resolved<f32>,
    pub client_registry_ttl: Resolved<u64>,
    /// No universal default exists (spec §9) — `None` means genuinely
    /// unset, not "fell back to a built-in value".
    pub quorum: Option<Resolved<u32>>,
    pub selector: Resolved<String>,
    pub seed_mode: Resolved<SeedMode>,
    /// `None` when the resolved mode profile doesn't use a fixed seed
    /// (production's "n/a", spec §4.1) — the `source` still names which
    /// tier produced that `None`.
    pub seed_value: Resolved<Option<u64>>,
    pub aggregator: Resolved<String>,
    pub robust_byzantine_fraction: Resolved<f32>,
    pub privacy_mechanism: Resolved<String>,
    pub clip_norm: Resolved<f32>,
    pub noise_multiplier: Resolved<f32>,
    pub target_epsilon: Resolved<f64>,
    pub delta: Resolved<f64>,
    pub budget_exhausted_action: Resolved<BudgetExhaustedAction>,
    pub accounting_scope: Resolved<AccountingScope>,
    pub allow_stub_client: Resolved<bool>,
    pub require_node_auth: Resolved<bool>,
    pub config_log_format: Resolved<LogFormat>,
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// ADR 0006: `PerClient` accounting is deferred to Phase 8. Selecting
    /// it before it exists must fail fast, not silently behave like
    /// `Global`.
    #[error(
        "accounting_scope = \"per_client\" is not implemented yet (deferred to Phase 8, \
         see docs/adr/0006-global-epsilon-accounting.md); select \"global\" instead"
    )]
    PerClientAccountingNotImplemented,
}

/// Resolves one parameter against the precedence chain from spec §4.1:
/// builtin fallback < topology profile < mode profile < file < env < cli.
/// The first tier (checked highest-precedence first) that set a value
/// wins.
///
/// This one generic function backs every field in [`resolve`] below,
/// rather than each of the 19 parameters re-implementing the same
/// six-way precedence check.
///
/// Ten arguments because there are genuinely six precedence tiers (each
/// needing a value) plus four of their sources — bundling them into a
/// struct would just move the same information around, not reduce it.
#[allow(clippy::too_many_arguments)]
fn layer<T: Clone>(
    builtin: T,
    topology: Option<T>,
    mode: Option<T>,
    file: Option<T>,
    env: Option<T>,
    cli: Option<T>,
    topology_source: &ConfigSource,
    mode_source: &ConfigSource,
    file_source: &ConfigSource,
    env_source: &ConfigSource,
) -> Resolved<T> {
    if let Some(value) = cli {
        return Resolved {
            value,
            source: ConfigSource::Cli,
        };
    }
    if let Some(value) = env {
        return Resolved {
            value,
            source: env_source.clone(),
        };
    }
    if let Some(value) = file {
        return Resolved {
            value,
            source: file_source.clone(),
        };
    }
    if let Some(value) = mode {
        return Resolved {
            value,
            source: mode_source.clone(),
        };
    }
    if let Some(value) = topology {
        return Resolved {
            value,
            source: topology_source.clone(),
        };
    }
    Resolved {
        value: builtin,
        source: ConfigSource::BuiltinFallback,
    }
}

/// Resolves every parameter in spec §9 against `topology` and `mode`, then
/// layers `file` (an experiment file's overrides, if any), `env`, and
/// `cli` on top — highest precedence last, per spec §4.1.
///
/// The ordering *within* the "explicit override" tier (cli beats env beats
/// file) is a Phase 1 decision, not something the spec pins down — see
/// spec §11 Open Item 2 and `docs/phases/phase-1-config-registry.md`.
pub fn resolve(
    topology: Topology,
    mode: Mode,
    file: Option<(&str, &Overrides)>,
    env: &Overrides,
    cli: &Overrides,
) -> Result<ResolvedConfig, ConfigError> {
    let topology_defaults = topology.defaults();
    let mode_defaults = mode.defaults();

    let topology_source = ConfigSource::TopologyProfile(topology.label().to_string());
    let mode_source = ConfigSource::ModeProfile(mode.label().to_string());
    let file_overrides = file.map(|(_, overrides)| overrides);
    let file_source = match file {
        Some((path, _)) => ConfigSource::ExperimentFile(path.to_string()),
        // Never read below: every field-level `file` extraction is `None`
        // whenever `file` itself is `None`, so this placeholder never
        // reaches a `Resolved`.
        None => ConfigSource::ExperimentFile(String::new()),
    };

    macro_rules! env_var {
        ($name:literal) => {
            ConfigSource::EnvVar(concat!("CONFLUX_", $name).to_string())
        };
    }

    let connection_mode = layer(
        topology_defaults.connection_mode,
        Some(topology_defaults.connection_mode),
        None,
        file_overrides.and_then(|o| o.connection_mode),
        env.connection_mode,
        cli.connection_mode,
        &topology_source,
        &mode_source,
        &file_source,
        &env_var!("CONNECTION_MODE"),
    );
    let auth = layer(
        topology_defaults.auth,
        Some(topology_defaults.auth),
        None,
        file_overrides.and_then(|o| o.auth),
        env.auth,
        cli.auth,
        &topology_source,
        &mode_source,
        &file_source,
        &env_var!("AUTH"),
    );
    let round_timeout_secs = layer(
        topology_defaults.round_timeout_secs,
        Some(topology_defaults.round_timeout_secs),
        None,
        file_overrides.and_then(|o| o.round_timeout_secs),
        env.round_timeout_secs,
        cli.round_timeout_secs,
        &topology_source,
        &mode_source,
        &file_source,
        &env_var!("ROUND_TIMEOUT_SECS"),
    );
    let min_reputation_score = layer(
        topology_defaults.min_reputation_score,
        Some(topology_defaults.min_reputation_score),
        None,
        file_overrides.and_then(|o| o.min_reputation_score),
        env.min_reputation_score,
        cli.min_reputation_score,
        &topology_source,
        &mode_source,
        &file_source,
        &env_var!("MIN_REPUTATION_SCORE"),
    );
    let client_registry_ttl = layer(
        topology_defaults.client_registry_ttl,
        Some(topology_defaults.client_registry_ttl),
        None,
        file_overrides.and_then(|o| o.client_registry_ttl),
        env.client_registry_ttl,
        cli.client_registry_ttl,
        &topology_source,
        &mode_source,
        &file_source,
        &env_var!("CLIENT_REGISTRY_TTL"),
    );

    // `quorum` has no builtin fallback (spec §9) — `None` here means no
    // tier set it at all, not "fell back to a default".
    let quorum = match (
        cli.quorum,
        env.quorum,
        file_overrides.and_then(|o| o.quorum),
    ) {
        (Some(value), _, _) => Some(Resolved {
            value,
            source: ConfigSource::Cli,
        }),
        (None, Some(value), _) => Some(Resolved {
            value,
            source: env_var!("QUORUM"),
        }),
        (None, None, Some(value)) => Some(Resolved {
            value,
            source: file_source.clone(),
        }),
        (None, None, None) => None,
    };

    let selector = layer(
        "uniform_random".to_string(),
        None,
        None,
        file_overrides.and_then(|o| o.selector.clone()),
        env.selector.clone(),
        cli.selector.clone(),
        &topology_source,
        &mode_source,
        &file_source,
        &env_var!("SELECTOR"),
    );
    let seed_mode = layer(
        mode_defaults.seed_mode,
        None,
        Some(mode_defaults.seed_mode),
        file_overrides.and_then(|o| o.seed_mode),
        env.seed_mode,
        cli.seed_mode,
        &topology_source,
        &mode_source,
        &file_source,
        &env_var!("SEED_MODE"),
    );
    let seed_value = layer(
        mode_defaults.seed_value,
        None,
        Some(mode_defaults.seed_value),
        file_overrides.and_then(|o| o.seed_value).map(Some),
        env.seed_value.map(Some),
        cli.seed_value.map(Some),
        &topology_source,
        &mode_source,
        &file_source,
        &env_var!("SEED_VALUE"),
    );
    let aggregator = layer(
        "fedavg".to_string(),
        None,
        None,
        file_overrides.and_then(|o| o.aggregator.clone()),
        env.aggregator.clone(),
        cli.aggregator.clone(),
        &topology_source,
        &mode_source,
        &file_source,
        &env_var!("AGGREGATOR"),
    );
    let robust_byzantine_fraction = layer(
        0.2_f32,
        None,
        None,
        file_overrides.and_then(|o| o.robust_byzantine_fraction),
        env.robust_byzantine_fraction,
        cli.robust_byzantine_fraction,
        &topology_source,
        &mode_source,
        &file_source,
        &env_var!("ROBUST_BYZANTINE_FRACTION"),
    );
    let privacy_mechanism = layer(
        "gaussian_clipping".to_string(),
        None,
        None,
        file_overrides.and_then(|o| o.privacy_mechanism.clone()),
        env.privacy_mechanism.clone(),
        cli.privacy_mechanism.clone(),
        &topology_source,
        &mode_source,
        &file_source,
        &env_var!("PRIVACY_MECHANISM"),
    );
    let clip_norm = layer(
        1.0_f32,
        None,
        None,
        file_overrides.and_then(|o| o.clip_norm),
        env.clip_norm,
        cli.clip_norm,
        &topology_source,
        &mode_source,
        &file_source,
        &env_var!("CLIP_NORM"),
    );
    let noise_multiplier = layer(
        1.0_f32,
        None,
        None,
        file_overrides.and_then(|o| o.noise_multiplier),
        env.noise_multiplier,
        cli.noise_multiplier,
        &topology_source,
        &mode_source,
        &file_source,
        &env_var!("NOISE_MULTIPLIER"),
    );
    let target_epsilon = layer(
        8.0_f64,
        None,
        None,
        file_overrides.and_then(|o| o.target_epsilon),
        env.target_epsilon,
        cli.target_epsilon,
        &topology_source,
        &mode_source,
        &file_source,
        &env_var!("TARGET_EPSILON"),
    );
    let delta = layer(
        1e-5_f64,
        None,
        None,
        file_overrides.and_then(|o| o.delta),
        env.delta,
        cli.delta,
        &topology_source,
        &mode_source,
        &file_source,
        &env_var!("DELTA"),
    );
    let budget_exhausted_action = layer(
        mode_defaults.budget_exhausted_action,
        None,
        Some(mode_defaults.budget_exhausted_action),
        file_overrides.and_then(|o| o.budget_exhausted_action),
        env.budget_exhausted_action,
        cli.budget_exhausted_action,
        &topology_source,
        &mode_source,
        &file_source,
        &env_var!("BUDGET_EXHAUSTED_ACTION"),
    );
    let accounting_scope = layer(
        mode_defaults.accounting_scope,
        None,
        Some(mode_defaults.accounting_scope),
        file_overrides.and_then(|o| o.accounting_scope),
        env.accounting_scope,
        cli.accounting_scope,
        &topology_source,
        &mode_source,
        &file_source,
        &env_var!("ACCOUNTING_SCOPE"),
    );
    let allow_stub_client = layer(
        mode_defaults.allow_stub_client,
        None,
        Some(mode_defaults.allow_stub_client),
        file_overrides.and_then(|o| o.allow_stub_client),
        env.allow_stub_client,
        cli.allow_stub_client,
        &topology_source,
        &mode_source,
        &file_source,
        &env_var!("ALLOW_STUB_CLIENT"),
    );
    let require_node_auth = layer(
        mode_defaults.require_node_auth,
        None,
        Some(mode_defaults.require_node_auth),
        file_overrides.and_then(|o| o.require_node_auth),
        env.require_node_auth,
        cli.require_node_auth,
        &topology_source,
        &mode_source,
        &file_source,
        &env_var!("REQUIRE_NODE_AUTH"),
    );
    let config_log_format = layer(
        mode_defaults.config_log_format,
        None,
        Some(mode_defaults.config_log_format),
        file_overrides.and_then(|o| o.config_log_format),
        env.config_log_format,
        cli.config_log_format,
        &topology_source,
        &mode_source,
        &file_source,
        &env_var!("CONFIG_LOG_FORMAT"),
    );

    if accounting_scope.value == AccountingScope::PerClient {
        return Err(ConfigError::PerClientAccountingNotImplemented);
    }

    Ok(ResolvedConfig {
        connection_mode,
        auth,
        round_timeout_secs,
        min_reputation_score,
        client_registry_ttl,
        quorum,
        selector,
        seed_mode,
        seed_value,
        aggregator,
        robust_byzantine_fraction,
        privacy_mechanism,
        clip_norm,
        noise_multiplier,
        target_epsilon,
        delta,
        budget_exhausted_action,
        accounting_scope,
        allow_stub_client,
        require_node_auth,
        config_log_format,
    })
}

impl ResolvedConfig {
    /// Every resolved parameter, one log line each, in `format` — this is
    /// what makes resolution explainable "out loud" rather than just
    /// internally consistent (ADR 0007). `conflux-server` must emit these
    /// at startup before reaching "ready".
    // Sequential pushes read more clearly here than one giant `vec![]`
    // literal spanning 19 multi-line, heterogeneously-typed `log_line`
    // calls.
    #[allow(clippy::vec_init_then_push)]
    pub fn to_log_lines(&self, format: LogFormat) -> Vec<String> {
        let mut lines = Vec::new();

        lines.push(log_line(
            format,
            "connection_mode",
            LoggedValue::Text(self.connection_mode.value.as_str()),
            &self.connection_mode.source,
        ));
        lines.push(log_line(
            format,
            "auth",
            LoggedValue::Text(self.auth.value.as_str()),
            &self.auth.source,
        ));
        lines.push(log_line(
            format,
            "round_timeout_secs",
            LoggedValue::Number(self.round_timeout_secs.value.to_string()),
            &self.round_timeout_secs.source,
        ));
        lines.push(log_line(
            format,
            "min_reputation_score",
            LoggedValue::Number(self.min_reputation_score.value.to_string()),
            &self.min_reputation_score.source,
        ));
        lines.push(log_line(
            format,
            "client_registry_ttl",
            LoggedValue::Number(self.client_registry_ttl.value.to_string()),
            &self.client_registry_ttl.source,
        ));
        if let Some(quorum) = &self.quorum {
            lines.push(log_line(
                format,
                "quorum",
                LoggedValue::Number(quorum.value.to_string()),
                &quorum.source,
            ));
        }
        lines.push(log_line(
            format,
            "selector",
            LoggedValue::Text(&self.selector.value),
            &self.selector.source,
        ));
        lines.push(log_line(
            format,
            "seed_mode",
            LoggedValue::Text(self.seed_mode.value.as_str()),
            &self.seed_mode.source,
        ));
        lines.push(log_line(
            format,
            "seed_value",
            match self.seed_value.value {
                Some(value) => LoggedValue::Number(value.to_string()),
                None => LoggedValue::Text("n/a"),
            },
            &self.seed_value.source,
        ));
        lines.push(log_line(
            format,
            "aggregator",
            LoggedValue::Text(&self.aggregator.value),
            &self.aggregator.source,
        ));
        lines.push(log_line(
            format,
            "robust_byzantine_fraction",
            LoggedValue::Number(self.robust_byzantine_fraction.value.to_string()),
            &self.robust_byzantine_fraction.source,
        ));
        lines.push(log_line(
            format,
            "privacy_mechanism",
            LoggedValue::Text(&self.privacy_mechanism.value),
            &self.privacy_mechanism.source,
        ));
        lines.push(log_line(
            format,
            "clip_norm",
            LoggedValue::Number(self.clip_norm.value.to_string()),
            &self.clip_norm.source,
        ));
        lines.push(log_line(
            format,
            "noise_multiplier",
            LoggedValue::Number(self.noise_multiplier.value.to_string()),
            &self.noise_multiplier.source,
        ));
        lines.push(log_line(
            format,
            "target_epsilon",
            LoggedValue::Number(self.target_epsilon.value.to_string()),
            &self.target_epsilon.source,
        ));
        lines.push(log_line(
            format,
            "delta",
            LoggedValue::Number(self.delta.value.to_string()),
            &self.delta.source,
        ));
        lines.push(log_line(
            format,
            "budget_exhausted_action",
            LoggedValue::Text(self.budget_exhausted_action.value.as_str()),
            &self.budget_exhausted_action.source,
        ));
        lines.push(log_line(
            format,
            "accounting_scope",
            LoggedValue::Text(self.accounting_scope.value.as_str()),
            &self.accounting_scope.source,
        ));
        lines.push(log_line(
            format,
            "allow_stub_client",
            LoggedValue::Number(self.allow_stub_client.value.to_string()),
            &self.allow_stub_client.source,
        ));
        lines.push(log_line(
            format,
            "require_node_auth",
            LoggedValue::Number(self.require_node_auth.value.to_string()),
            &self.require_node_auth.source,
        ));
        lines.push(log_line(
            format,
            "config_log_format",
            LoggedValue::Text(self.config_log_format.value.as_str()),
            &self.config_log_format.source,
        ));

        lines
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_fallback_wins_with_no_overrides_or_axis_ownership() {
        let resolved = resolve(
            Topology::CrossSilo,
            Mode::Research,
            None,
            &Overrides::default(),
            &Overrides::default(),
        )
        .unwrap();

        assert_eq!(resolved.clip_norm.value, 1.0);
        assert_eq!(resolved.clip_norm.source, ConfigSource::BuiltinFallback);
    }

    #[test]
    fn topology_profile_wins_over_builtin() {
        let resolved = resolve(
            Topology::CrossDevice,
            Mode::Research,
            None,
            &Overrides::default(),
            &Overrides::default(),
        )
        .unwrap();

        assert_eq!(resolved.round_timeout_secs.value, 300);
        assert_eq!(
            resolved.round_timeout_secs.source,
            ConfigSource::TopologyProfile("cross_device".to_string())
        );
    }

    #[test]
    fn mode_profile_wins_over_topology_for_mode_owned_params() {
        let resolved = resolve(
            Topology::CrossSilo,
            Mode::Production,
            None,
            &Overrides::default(),
            &Overrides::default(),
        )
        .unwrap();

        assert!(!resolved.allow_stub_client.value);
        assert_eq!(
            resolved.allow_stub_client.source,
            ConfigSource::ModeProfile("production".to_string())
        );
    }

    #[test]
    fn require_node_auth_defaults_off_in_research_and_on_in_production() {
        let research = resolve(
            Topology::CrossSilo,
            Mode::Research,
            None,
            &Overrides::default(),
            &Overrides::default(),
        )
        .unwrap();
        assert!(!research.require_node_auth.value);
        assert_eq!(
            research.require_node_auth.source,
            ConfigSource::ModeProfile("research".to_string())
        );

        let production = resolve(
            Topology::CrossSilo,
            Mode::Production,
            None,
            &Overrides::default(),
            &Overrides::default(),
        )
        .unwrap();
        assert!(production.require_node_auth.value);
        assert_eq!(
            production.require_node_auth.source,
            ConfigSource::ModeProfile("production".to_string())
        );
    }

    #[test]
    fn require_node_auth_explicit_override_wins_over_mode_default() {
        let cli_overrides = Overrides {
            require_node_auth: Some(true),
            ..Default::default()
        };

        let resolved = resolve(
            Topology::CrossSilo,
            Mode::Research,
            None,
            &Overrides::default(),
            &cli_overrides,
        )
        .unwrap();

        assert!(resolved.require_node_auth.value);
        assert_eq!(resolved.require_node_auth.source, ConfigSource::Cli);
    }

    #[test]
    fn robust_byzantine_fraction_defaults_to_builtin_fallback() {
        let resolved = resolve(
            Topology::CrossSilo,
            Mode::Research,
            None,
            &Overrides::default(),
            &Overrides::default(),
        )
        .unwrap();

        assert_eq!(resolved.robust_byzantine_fraction.value, 0.2);
        assert_eq!(
            resolved.robust_byzantine_fraction.source,
            ConfigSource::BuiltinFallback
        );
    }

    #[test]
    fn robust_byzantine_fraction_explicit_override_wins() {
        let cli_overrides = Overrides {
            robust_byzantine_fraction: Some(0.33),
            ..Default::default()
        };

        let resolved = resolve(
            Topology::CrossSilo,
            Mode::Research,
            None,
            &Overrides::default(),
            &cli_overrides,
        )
        .unwrap();

        assert_eq!(resolved.robust_byzantine_fraction.value, 0.33);
        assert_eq!(resolved.robust_byzantine_fraction.source, ConfigSource::Cli);
    }

    #[test]
    fn file_wins_over_topology_and_mode() {
        let file_overrides = Overrides {
            round_timeout_secs: Some(120),
            ..Default::default()
        };

        let resolved = resolve(
            Topology::CrossDevice,
            Mode::Research,
            Some(("experiment.toml", &file_overrides)),
            &Overrides::default(),
            &Overrides::default(),
        )
        .unwrap();

        assert_eq!(resolved.round_timeout_secs.value, 120);
        assert_eq!(
            resolved.round_timeout_secs.source,
            ConfigSource::ExperimentFile("experiment.toml".to_string())
        );
    }

    #[test]
    fn env_wins_over_file() {
        let file_overrides = Overrides {
            clip_norm: Some(2.0),
            ..Default::default()
        };
        let env_overrides = Overrides {
            clip_norm: Some(3.0),
            ..Default::default()
        };

        let resolved = resolve(
            Topology::CrossSilo,
            Mode::Research,
            Some(("experiment.toml", &file_overrides)),
            &env_overrides,
            &Overrides::default(),
        )
        .unwrap();

        assert_eq!(resolved.clip_norm.value, 3.0);
        assert_eq!(
            resolved.clip_norm.source,
            ConfigSource::EnvVar("CONFLUX_CLIP_NORM".to_string())
        );
    }

    #[test]
    fn cli_wins_over_everything() {
        let env_overrides = Overrides {
            clip_norm: Some(3.0),
            ..Default::default()
        };
        let cli_overrides = Overrides {
            clip_norm: Some(4.0),
            ..Default::default()
        };

        let resolved = resolve(
            Topology::CrossSilo,
            Mode::Research,
            None,
            &env_overrides,
            &cli_overrides,
        )
        .unwrap();

        assert_eq!(resolved.clip_norm.value, 4.0);
        assert_eq!(resolved.clip_norm.source, ConfigSource::Cli);
    }

    #[test]
    fn quorum_has_no_builtin_fallback() {
        let resolved = resolve(
            Topology::CrossSilo,
            Mode::Research,
            None,
            &Overrides::default(),
            &Overrides::default(),
        )
        .unwrap();

        assert!(resolved.quorum.is_none());
    }

    #[test]
    fn quorum_resolves_when_overridden() {
        let cli_overrides = Overrides {
            quorum: Some(5),
            ..Default::default()
        };

        let resolved = resolve(
            Topology::CrossSilo,
            Mode::Research,
            None,
            &Overrides::default(),
            &cli_overrides,
        )
        .unwrap();

        assert_eq!(resolved.quorum.unwrap().value, 5);
    }

    #[test]
    fn seed_value_is_none_in_production_by_default() {
        let resolved = resolve(
            Topology::CrossSilo,
            Mode::Production,
            None,
            &Overrides::default(),
            &Overrides::default(),
        )
        .unwrap();

        assert_eq!(resolved.seed_value.value, None);
        assert_eq!(
            resolved.seed_value.source,
            ConfigSource::ModeProfile("production".to_string())
        );
    }

    #[test]
    fn per_client_accounting_fails_fast() {
        let cli_overrides = Overrides {
            accounting_scope: Some(AccountingScope::PerClient),
            ..Default::default()
        };

        let err = resolve(
            Topology::CrossSilo,
            Mode::Research,
            None,
            &Overrides::default(),
            &cli_overrides,
        )
        .unwrap_err();

        assert!(matches!(
            err,
            ConfigError::PerClientAccountingNotImplemented
        ));
    }

    #[test]
    fn json_log_line_matches_spec_example_shape() {
        let resolved = resolve(
            Topology::CrossDevice,
            Mode::Research,
            None,
            &Overrides::default(),
            &Overrides::default(),
        )
        .unwrap();

        let lines = resolved.to_log_lines(LogFormat::Json);
        let round_timeout_line = lines
            .iter()
            .find(|line| line.contains("\"param\":\"round_timeout_secs\""))
            .expect("round_timeout_secs line present");

        assert_eq!(
            round_timeout_line,
            "{\"param\":\"round_timeout_secs\",\"value\":300,\"source\":\"topology_profile\",\"profile\":\"cross_device\"}"
        );
    }

    #[test]
    fn text_log_line_matches_spec_example_shape() {
        let resolved = resolve(
            Topology::CrossSilo,
            Mode::Research,
            None,
            &Overrides::default(),
            &Overrides::default(),
        )
        .unwrap();

        let lines = resolved.to_log_lines(LogFormat::Text);
        let clip_norm_line = lines
            .iter()
            .find(|line| line.starts_with("[config] clip_norm"))
            .expect("clip_norm line present");

        assert!(clip_norm_line.contains("= 1"));
        assert!(clip_norm_line.contains("(source: built-in fallback)"));
    }
}
