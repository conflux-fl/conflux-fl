//! Enums for every closed-set config parameter, plus the topology/mode
//! profile definitions from spec §3 and §4.1.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionMode {
    Push,
    Pull,
}

impl ConnectionMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            ConnectionMode::Push => "push",
            ConnectionMode::Pull => "pull",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthMode {
    Mtls,
    Jwt,
}

impl AuthMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            AuthMode::Mtls => "mtls",
            AuthMode::Jwt => "jwt",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeedMode {
    Fixed,
    OsRandom,
}

impl SeedMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            SeedMode::Fixed => "fixed",
            SeedMode::OsRandom => "os_random",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BudgetExhaustedAction {
    Halt,
    ContinueWithoutGuarantee,
}

impl BudgetExhaustedAction {
    pub fn as_str(&self) -> &'static str {
        match self {
            BudgetExhaustedAction::Halt => "halt",
            BudgetExhaustedAction::ContinueWithoutGuarantee => "continue_without_guarantee",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccountingScope {
    Global,
    /// Deferred to Phase 8 (ADR 0006) — selecting this fails resolution
    /// fast rather than silently behaving like `Global`.
    PerClient,
}

impl AccountingScope {
    pub fn as_str(&self) -> &'static str {
        match self {
            AccountingScope::Global => "global",
            AccountingScope::PerClient => "per_client",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogFormat {
    Json,
    Text,
}

impl LogFormat {
    pub fn as_str(&self) -> &'static str {
        match self {
            LogFormat::Json => "json",
            LogFormat::Text => "text",
        }
    }
}

/// The four deployment topologies from spec §3, one framework codebase.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Topology {
    CrossSilo,
    CrossDevice,
    Crowdsource,
    Edge,
}

/// The topology-owned parameters (spec §3): `connection_mode`, `auth`,
/// `round_timeout_secs`, `min_reputation_score`, `client_registry_ttl`.
#[derive(Debug, Clone, Copy)]
pub struct TopologyDefaults {
    pub connection_mode: ConnectionMode,
    pub auth: AuthMode,
    pub round_timeout_secs: u64,
    pub min_reputation_score: f32,
    /// Seconds.
    pub client_registry_ttl: u64,
}

impl Topology {
    pub fn label(&self) -> &'static str {
        match self {
            Topology::CrossSilo => "cross_silo",
            Topology::CrossDevice => "cross_device",
            Topology::Crowdsource => "crowdsource",
            Topology::Edge => "edge",
        }
    }

    /// `connection_mode`/`auth` are exactly spec §3's table.
    /// `round_timeout_secs = 300` for `cross_device` is spec §4.2's own
    /// worked example, treated as canonical. Every other numeric value
    /// here is a Phase 1 placeholder — the spec promises "full defaults in
    /// §8's reference table" but neither §8 nor §9 actually lists
    /// per-topology numbers beyond that one example. See
    /// `docs/STATUS.md`'s "Known deviations" for the real gap and
    /// `docs/phases/phase-1-config-registry.md`.
    pub fn defaults(&self) -> TopologyDefaults {
        match self {
            Topology::CrossSilo => TopologyDefaults {
                connection_mode: ConnectionMode::Push,
                auth: AuthMode::Mtls,
                round_timeout_secs: 600,   // placeholder
                min_reputation_score: 0.0, // placeholder: trusted, no gating
                client_registry_ttl: 3600, // placeholder: few, always-reachable
            },
            Topology::CrossDevice => TopologyDefaults {
                connection_mode: ConnectionMode::Pull,
                auth: AuthMode::Jwt,
                round_timeout_secs: 300,   // spec §4.2 example
                min_reputation_score: 0.3, // placeholder
                client_registry_ttl: 900,  // placeholder: intermittent
            },
            Topology::Crowdsource => TopologyDefaults {
                connection_mode: ConnectionMode::Pull,
                auth: AuthMode::Jwt,
                round_timeout_secs: 300, // placeholder, mirrors cross_device
                min_reputation_score: 0.6, // placeholder: "stricter" per spec §3's fit column
                client_registry_ttl: 900, // placeholder
            },
            Topology::Edge => TopologyDefaults {
                connection_mode: ConnectionMode::Pull,
                auth: AuthMode::Jwt,
                round_timeout_secs: 300, // placeholder
                // placeholder: resource-aware selection is future/Phase 8
                // (spec §3), so v1 mirrors cross_device.
                min_reputation_score: 0.3,
                client_registry_ttl: 900, // placeholder
            },
        }
    }
}

/// The two operating postures from spec §4.1: research (iterating) vs.
/// production (live deployment).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Research,
    Production,
}

/// The mode-owned parameters (spec §4.1/§9): `seed_mode`, `seed_value`,
/// `budget_exhausted_action`, `accounting_scope`, `allow_stub_client`,
/// `require_node_auth`, `config_log_format`.
#[derive(Debug, Clone, Copy)]
pub struct ModeDefaults {
    pub seed_mode: SeedMode,
    pub seed_value: Option<u64>,
    pub budget_exhausted_action: BudgetExhaustedAction,
    pub accounting_scope: AccountingScope,
    pub allow_stub_client: bool,
    /// Phase 8b: same on/off-toggle shape as `allow_stub_client` — a real
    /// production deployment must check node identity against an
    /// allow-list before letting it register, but a research experiment
    /// iterating on an algorithm shouldn't have to stand one up first.
    pub require_node_auth: bool,
    pub config_log_format: LogFormat,
}

impl Mode {
    pub fn label(&self) -> &'static str {
        match self {
            Mode::Research => "research",
            Mode::Production => "production",
        }
    }

    /// Exactly the `[profiles.research]`/`[profiles.production]` TOML
    /// block from spec §4.1.
    pub fn defaults(&self) -> ModeDefaults {
        match self {
            Mode::Research => ModeDefaults {
                seed_mode: SeedMode::Fixed,
                seed_value: Some(42),
                budget_exhausted_action: BudgetExhaustedAction::ContinueWithoutGuarantee,
                accounting_scope: AccountingScope::Global,
                allow_stub_client: true,
                require_node_auth: false,
                config_log_format: LogFormat::Text,
            },
            Mode::Production => ModeDefaults {
                seed_mode: SeedMode::OsRandom,
                seed_value: None, // n/a per spec §4.1
                budget_exhausted_action: BudgetExhaustedAction::Halt,
                accounting_scope: AccountingScope::Global,
                allow_stub_client: false,
                require_node_auth: true,
                config_log_format: LogFormat::Json,
            },
        }
    }
}
