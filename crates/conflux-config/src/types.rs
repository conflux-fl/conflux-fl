//! Enums for every closed-set config parameter (an `AuthMode` can only be
//! `Mtls` or `Jwt` — never an arbitrary string), plus the topology and
//! mode profile definitions: what each of the four topologies and two
//! modes defaults every parameter it owns to.

/// `rename_all = "snake_case"` is not an arbitrary style choice: it
/// produces exactly the strings each enum's own `as_str()` returns, so
/// the spelling accepted in an experiment TOML file and the spelling
/// printed in the resolved-config log (ADR 0007) are the same by
/// construction rather than by two lists someone has to keep in sync.
/// `every_enums_toml_spelling_matches_its_as_str` asserts that.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountingScope {
    /// One shared differential-privacy budget for the whole experiment —
    /// every client's rounds count against the same epsilon.
    Global,
    /// A separate differential-privacy budget tracked per client. Each
    /// client's own round history is replayed from `PrivacyRoundLog` on
    /// server restart, alongside the global history, regardless of which
    /// scope is currently configured — so switching scopes doesn't lose
    /// either history.
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
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

/// The four deployment topologies Conflux FL supports from one codebase,
/// selected by configuration rather than by forking code: `cross_silo`
/// (few, trusted, institutional participants — e.g. hospitals training a
/// shared model), `cross_device` (many phones/laptops, intermittently
/// connected), `crowdsource` (public/anonymous participants), and `edge`
/// (IoT/edge compute).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Topology {
    CrossSilo,
    CrossDevice,
    Crowdsource,
    Edge,
}

/// The topology-owned parameters: `connection_mode`, `auth`,
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

    /// `connection_mode`/`auth` reflect each topology's real constraints:
    /// `cross_silo`'s few, trusted, always-reachable institutions can hold
    /// an open connection and use mutual TLS; `cross_device`,
    /// `crowdsource`, and `edge` all involve many intermittently-connected
    /// participants, so they pull tasks on their own schedule and
    /// authenticate with a JWT rather than a long-lived TLS session.
    ///
    /// The numeric values (`round_timeout_secs`, `min_reputation_score`,
    /// `client_registry_ttl`) are starting defaults tuned for each
    /// topology's general shape, not derived from a formula — a real
    /// deployment should tune them for its own network conditions and
    /// trust level via an experiment file, env var, or CLI override
    /// rather than treating them as fixed. `cross_silo` uses a long
    /// timeout and TTL (few, patient, always-on participants) and no
    /// reputation gating (already trusted, e.g. by contract);
    /// `cross_device` and `edge` use a shorter timeout and TTL
    /// (intermittent connectivity) with light gating; `crowdsource`
    /// mirrors `cross_device` but with stricter reputation gating, since
    /// its participants are anonymous/public rather than known devices.
    pub fn defaults(&self) -> TopologyDefaults {
        match self {
            Topology::CrossSilo => TopologyDefaults {
                connection_mode: ConnectionMode::Push,
                auth: AuthMode::Mtls,
                round_timeout_secs: 600,
                min_reputation_score: 0.0,
                client_registry_ttl: 3600,
            },
            Topology::CrossDevice => TopologyDefaults {
                connection_mode: ConnectionMode::Pull,
                auth: AuthMode::Jwt,
                round_timeout_secs: 300,
                min_reputation_score: 0.3,
                client_registry_ttl: 900,
            },
            Topology::Crowdsource => TopologyDefaults {
                connection_mode: ConnectionMode::Pull,
                auth: AuthMode::Jwt,
                round_timeout_secs: 300,
                min_reputation_score: 0.6,
                client_registry_ttl: 900,
            },
            Topology::Edge => TopologyDefaults {
                connection_mode: ConnectionMode::Pull,
                auth: AuthMode::Jwt,
                round_timeout_secs: 300,
                // Mirrors cross_device: resource-aware selection tuned
                // specifically for edge/IoT constraints isn't implemented
                // yet, so edge uses the same starting posture as the
                // closest existing topology rather than an untested guess.
                min_reputation_score: 0.3,
                client_registry_ttl: 900,
            },
        }
    }
}

/// The two operating postures every deployment picks one of: `research`
/// (iterating on an algorithm — relax safety checks for speed) or
/// `production` (a live deployment — require real backends and identity
/// checks, refuse to start otherwise).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Research,
    Production,
}

/// The mode-owned parameters: `seed_mode`/`seed_value` (reproducibility),
/// `budget_exhausted_action`/`accounting_scope` (differential-privacy
/// posture), `allow_stub_client` (whether the fixed-dummy-weights,
/// no-PyTorch stub client is allowed to connect), `require_node_auth`
/// (whether a connecting node's identity is checked against an
/// allow-list before it can register), and `config_log_format`.
#[derive(Debug, Clone, Copy)]
pub struct ModeDefaults {
    pub seed_mode: SeedMode,
    pub seed_value: Option<u64>,
    pub budget_exhausted_action: BudgetExhaustedAction,
    pub accounting_scope: AccountingScope,
    pub allow_stub_client: bool,
    /// Same on/off-toggle shape as `allow_stub_client`: a real production
    /// deployment must check a connecting node's identity against an
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

    /// `research` favors fast iteration: a fixed seed for reproducible
    /// runs, no node-identity checks, text logs (easier to read at a
    /// terminal while iterating), and a privacy budget that keeps running
    /// past exhaustion rather than halting an experiment. `production`
    /// favors safety: OS randomness (no fixed seed to accidentally rely
    /// on), mandatory node auth, JSON logs (machine-parseable for a real
    /// deployment's log pipeline), and a privacy budget that halts the
    /// round rather than silently continuing past its guarantee.
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
                seed_value: None, // production uses OS randomness, not a fixed seed — not applicable
                budget_exhausted_action: BudgetExhaustedAction::Halt,
                accounting_scope: AccountingScope::Global,
                allow_stub_client: false,
                require_node_auth: true,
                config_log_format: LogFormat::Json,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The coupling `rename_all = "snake_case"` relies on: every enum's
    /// TOML spelling must be exactly the string its own `as_str()`
    /// returns. If someone renames a variant, adds one, or reaches for a
    /// different `rename_all` style, the file schema and the ADR 0007
    /// resolved-config log would silently disagree — this fails first.
    #[test]
    fn every_enums_toml_spelling_matches_its_as_str() {
        fn round_trip<T>(variants: &[T])
        where
            T: serde::de::DeserializeOwned + PartialEq + std::fmt::Debug + Copy,
            T: AsStr,
        {
            for variant in variants {
                let spelling = variant.as_str_for_test();
                // TOML has no bare top-level value, so wrap it in a
                // one-key document to deserialize a single enum.
                let doc = format!("value = \"{spelling}\"");
                #[derive(serde::Deserialize)]
                struct Wrapper<U> {
                    value: U,
                }
                let parsed: Wrapper<T> = toml::from_str(&doc)
                    .unwrap_or_else(|e| panic!("{spelling:?} is not accepted by serde: {e}"));
                assert_eq!(
                    parsed.value, *variant,
                    "{spelling:?} deserialized to the wrong variant"
                );
            }
        }

        /// Lets the generic helper above reach each enum's inherent
        /// `as_str` — they're inherent methods, not a shared trait, so
        /// this test-only trait bridges them without changing the
        /// public API.
        trait AsStr {
            fn as_str_for_test(&self) -> &'static str;
        }
        macro_rules! impl_as_str {
            ($($t:ty),*) => {$(
                impl AsStr for $t {
                    fn as_str_for_test(&self) -> &'static str {
                        self.as_str()
                    }
                }
            )*};
        }
        impl_as_str!(
            ConnectionMode,
            AuthMode,
            SeedMode,
            BudgetExhaustedAction,
            AccountingScope,
            LogFormat
        );

        round_trip(&[ConnectionMode::Push, ConnectionMode::Pull]);
        round_trip(&[AuthMode::Mtls, AuthMode::Jwt]);
        round_trip(&[SeedMode::Fixed, SeedMode::OsRandom]);
        round_trip(&[
            BudgetExhaustedAction::Halt,
            BudgetExhaustedAction::ContinueWithoutGuarantee,
        ]);
        round_trip(&[AccountingScope::Global, AccountingScope::PerClient]);
        round_trip(&[LogFormat::Json, LogFormat::Text]);
    }
}
