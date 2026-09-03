//! Enums for every closed-set config parameter (an `AuthMode` can only be
//! `Mtls` or `Jwt` — never an arbitrary string), plus the topology and
//! mode profile definitions: what each of the four topologies and two
//! modes defaults every parameter it owns to.

/// `rename_all = "snake_case"` is not an arbitrary style choice: it
/// produces exactly the strings each enum's own `as_str()` returns, so
/// the spelling accepted in an experiment TOML file and the spelling
/// printed in the resolved-config log are the same by
/// construction rather than by two lists someone has to keep in sync.
/// `every_enums_toml_spelling_matches_its_as_str` asserts that.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionMode {
    /// The server streams tasks to a subscribed client. `cross_silo`'s
    /// default: few, trusted, always-reachable participants.
    Push,
    /// The client asks for its next task when ready. The default
    /// everywhere else — many, intermittently-connected participants.
    Pull,
}

impl ConnectionMode {
    /// This value's canonical string form — the spelling accepted in a
    /// config file and printed in the startup log, which are the same by
    /// construction.
    pub fn as_str(&self) -> &'static str {
        match self {
            ConnectionMode::Push => "push",
            ConnectionMode::Pull => "pull",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
/// How a node proves who it is.
pub enum AuthMode {
    /// Mutual TLS: identity is a client certificate, verified at the TLS
    /// layer before any RPC runs.
    Mtls,
    /// A signed JWT in `RegisterRequest.auth_token`, verified against a
    /// configured public key.
    Jwt,
}

impl AuthMode {
    /// This value's canonical string form — the spelling accepted in a
    /// config file and printed in the startup log, which are the same by
    /// construction.
    pub fn as_str(&self) -> &'static str {
        match self {
            AuthMode::Mtls => "mtls",
            AuthMode::Jwt => "jwt",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
/// Where client-sampling randomness comes from.
pub enum SeedMode {
    /// A fixed seed, so a run is reproducible. Research default.
    Fixed,
    /// OS entropy. Production default — a predictable selection is
    /// exploitable by a client that wants to be chosen.
    OsRandom,
}

impl SeedMode {
    /// This value's canonical string form — the spelling accepted in a
    /// config file and printed in the startup log, which are the same by
    /// construction.
    pub fn as_str(&self) -> &'static str {
        match self {
            SeedMode::Fixed => "fixed",
            SeedMode::OsRandom => "os_random",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
/// What to do when the privacy budget runs out.
pub enum BudgetExhaustedAction {
    /// Stop the round. The default, and the only choice that keeps the
    /// epsilon guarantee true.
    Halt,
    /// Keep training past the budget. The name is deliberately blunt:
    /// choosing this means the deployment no longer has the differential
    /// privacy guarantee it configured.
    ContinueWithoutGuarantee,
}

impl BudgetExhaustedAction {
    /// This value's canonical string form — the spelling accepted in a
    /// config file and printed in the startup log, which are the same by
    /// construction.
    pub fn as_str(&self) -> &'static str {
        match self {
            BudgetExhaustedAction::Halt => "halt",
            BudgetExhaustedAction::ContinueWithoutGuarantee => "continue_without_guarantee",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
/// Whether epsilon is tracked for the experiment as a whole or per
/// client.
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
    /// This value's canonical string form — the spelling accepted in a
    /// config file and printed in the startup log, which are the same by
    /// construction.
    pub fn as_str(&self) -> &'static str {
        match self {
            AccountingScope::Global => "global",
            AccountingScope::PerClient => "per_client",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
/// How the resolved configuration is printed at startup.
pub enum LogFormat {
    /// One JSON object per parameter. Production default — machine-
    /// readable for log aggregation.
    Json,
    /// Aligned, human-readable lines. Research default.
    Text,
}

impl LogFormat {
    /// This value's canonical string form — the spelling accepted in a
    /// config file and printed in the startup log, which are the same by
    /// construction.
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
    /// Few, trusted institutional participants on reliable connections.
    CrossSilo,
    /// Many intermittently-connected devices, e.g. phones.
    CrossDevice,
    /// Like `CrossDevice`, but participants are anonymous or public
    /// rather than known devices — hence stricter reputation gating.
    Crowdsource,
    /// Operator-provisioned fleets of compute-constrained devices —
    /// gateways, SBCs, IoT. Pulls like `CrossDevice`, but authenticates
    /// with mTLS and gets silo-grade patience; `defaults` says why.
    Edge,
}

/// The topology-owned parameters: `connection_mode`, `auth`,
/// `round_timeout_secs`, `min_reputation_score`, `client_registry_ttl`.
#[derive(Debug, Clone, Copy)]
pub struct TopologyDefaults {
    /// Push or pull, per this topology's connectivity assumptions.
    pub connection_mode: ConnectionMode,
    /// The authentication mode this topology's trust model implies.
    pub auth: AuthMode,
    /// How long to wait for a round's submissions.
    pub round_timeout_secs: u64,
    /// Reputation gating threshold. `0.0` means no gating.
    pub min_reputation_score: f32,
    /// Seconds.
    pub client_registry_ttl: u64,
}

impl Topology {
    /// Every builtin topology, for iteration — profile loading checks
    /// names against this so the list can never drift from the enum.
    pub const ALL: [Topology; 4] = [
        Topology::CrossSilo,
        Topology::CrossDevice,
        Topology::Crowdsource,
        Topology::Edge,
    ];

    /// This topology's config-file and log spelling.
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
            // Edge is deliberately not a copy of cross_device. The
            // distinction that drives every field below: an edge
            // fleet is **operator-provisioned** — someone installs each
            // device and can install identity material while doing it —
            // where cross_device/crowdsource participants are machines
            // nobody provisions. (The topology taxonomy follows Kairouz
            // et al. 2021, *Advances and Open Problems in Federated
            // Learning*, §1: population size, membership stability, and
            // provisioning are what actually separate deployment
            // shapes.)
            Topology::Edge => TopologyDefaults {
                // Gateways and NAT'd installations can't accept inbound
                // connections any more than phones can — polling, same
                // as cross_device, for the same reason.
                connection_mode: ConnectionMode::Pull,
                // The provisioning argument, applied: a fleet someone
                // installs can carry a client certificate from day one,
                // which is exactly the trust model mTLS encodes. JWT
                // exists for the topologies where pre-provisioning is
                // impossible — that constraint does not hold here, and
                // edge devices are the ones most physically exposed to
                // tampering, so the stronger identity is also the one
                // this topology most needs.
                auth: AuthMode::Mtls,
                // Edge hardware is compute-constrained relative to the
                // phones cross_device assumes — MCU/SBC-class devices,
                // not NPUs — so the same local step count takes several
                // times longer. A 5-minute budget that fits a phone
                // starves a Pi-class trainer into missing every round.
                round_timeout_secs: 900,
                // Reputation gating defaults track how *open* the
                // population is: cross_silo 0.0 (closed, trusted),
                // cross_device 0.3 (open), crowdsource 0.6 (anonymous).
                // An operator-owned fleet is a closed population — the
                // realistic threat is a compromised device, not a Sybil
                // influx — so gating is opt-in here as it is for silos.
                min_reputation_score: 0.0,
                // Membership is stable (installed devices don't churn
                // like phones) but *links* are not (sleep cycles, lossy
                // radio). A short TTL evicts a device mid-sleep and
                // buys nothing from a fleet whose membership barely
                // changes; silo-grade patience fits.
                client_registry_ttl: 3600,
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
    /// Iterating on an experiment. Permissive defaults, reproducible
    /// seeding, human-readable logs.
    Research,
    /// A live deployment. Refuses configurations research tolerates —
    /// in-memory backends, missing auth material, the stub client.
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
    /// Fixed seeding for reproducibility, or OS entropy.
    pub seed_mode: SeedMode,
    /// The fixed seed, when `seed_mode` is `Fixed`. `None` under
    /// `OsRandom`.
    pub seed_value: Option<u64>,
    /// What happens when the privacy budget runs out.
    pub budget_exhausted_action: BudgetExhaustedAction,
    /// Whether epsilon is tracked globally or per client.
    pub accounting_scope: AccountingScope,
    /// Whether the stub Python `ClientApp` (fixed dummy weights, no
    /// PyTorch) may connect.
    pub allow_stub_client: bool,
    /// Same on/off-toggle shape as `allow_stub_client`: a real production
    /// deployment must check a connecting node's identity against an
    /// allow-list before letting it register, but a research experiment
    /// iterating on an algorithm shouldn't have to stand one up first.
    pub require_node_auth: bool,
    /// JSON or text for the startup configuration log.
    pub config_log_format: LogFormat,
}

impl Mode {
    /// Every builtin mode, for iteration.
    pub const ALL: [Mode; 2] = [Mode::Research, Mode::Production];

    /// This mode's config-file and log spelling.
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
    /// different `rename_all` style, the file schema and the
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
