//! The `CONFLUX_*` environment, read into the explicit-override tier —
//! shared by `conflux-server` and `cflux` so "what does this variable
//! set" has exactly one answer.
//!
//! Two things live here. [`overrides_from_env`] maps the per-parameter
//! variables onto an [`Overrides`]; a variable that is *set* but does not
//! parse is an error, never a silent fallback, because a typo in a
//! learning rate must not quietly run with a different one.
//! [`topology_profile_named`] and [`mode_profile_named`] turn a name —
//! `CONFLUX_TOPOLOGY`, or a `--topology` flag — into a profile: a builtin
//! by its label, otherwise a `<name>.toml` in the profile directory, and
//! a name that matches nothing is an error listing what exists rather
//! than a fallback to some other topology.

use std::path::Path;

use crate::Overrides;
use crate::profile::{
    ModeProfile, ProfileError, TopologyProfile, load_mode_profile, load_topology_profile,
};
use crate::types::{Mode, Topology};

/// A `CONFLUX_*` variable that is set but cannot be parsed.
#[derive(Debug, thiserror::Error)]
pub enum EnvError {
    /// The variable's value is not valid for its parameter's type.
    #[error("{name}={value:?} is not a valid value")]
    Invalid {
        /// The variable, e.g. `CONFLUX_QUORUM`.
        name: String,
        /// The value found.
        value: String,
    },
}

/// The per-parameter `CONFLUX_*` variables of the current process, as
/// the explicit-override tier. Unset variables leave their field `None`
/// so a profile or builtin decides.
pub fn overrides_from_env() -> Result<Overrides, EnvError> {
    overrides_from_vars(|name| std::env::var(name).ok())
}

/// [`overrides_from_env`] over any lookup function, so the mapping can be
/// tested without touching the process environment.
pub fn overrides_from_vars(get: impl Fn(&str) -> Option<String>) -> Result<Overrides, EnvError> {
    fn parse<T: std::str::FromStr>(name: &str, raw: Option<String>) -> Result<Option<T>, EnvError> {
        match raw {
            None => Ok(None),
            Some(value) => value.parse().map(Some).map_err(|_| EnvError::Invalid {
                name: name.to_string(),
                value,
            }),
        }
    }
    macro_rules! var {
        ($name:literal) => {
            parse($name, get($name))?
        };
    }
    Ok(Overrides {
        aggregator: get("CONFLUX_AGGREGATOR"),
        selector: get("CONFLUX_SELECTOR"),
        privacy_mechanism: get("CONFLUX_PRIVACY_MECHANISM"),
        robust_byzantine_fraction: var!("CONFLUX_ROBUST_BYZANTINE_FRACTION"),
        clip_radius: var!("CONFLUX_CLIP_RADIUS"),
        server_learning_rate: var!("CONFLUX_SERVER_LEARNING_RATE"),
        server_tau: var!("CONFLUX_SERVER_TAU"),
        server_momentum: var!("CONFLUX_SERVER_MOMENTUM"),
        fairness_q: var!("CONFLUX_FAIRNESS_Q"),
        scaffold_num_clients: var!("CONFLUX_SCAFFOLD_NUM_CLIENTS"),
        zeno_rho: var!("CONFLUX_ZENO_RHO"),
        server_lipschitz: var!("CONFLUX_SERVER_LIPSCHITZ"),
        min_reputation_score: var!("CONFLUX_MIN_REPUTATION_SCORE"),
        reputation_filter_enabled: var!("CONFLUX_REPUTATION_FILTER_ENABLED"),
        quorum: var!("CONFLUX_QUORUM"),
        max_update_bytes: var!("CONFLUX_MAX_UPDATE_BYTES"),
        round_timeout_secs: var!("CONFLUX_ROUND_TIMEOUT_SECS"),
        clip_norm: var!("CONFLUX_CLIP_NORM"),
        noise_multiplier: var!("CONFLUX_NOISE_MULTIPLIER"),
        ..Default::default()
    })
}

/// The topology profile a name selects: `None` is the builtin default
/// (`cross_device`), a builtin label is that builtin, and anything else
/// is `<name>.toml` under `dir`, followed down its `inherits` chain.
pub fn topology_profile_named(
    dir: &Path,
    name: Option<&str>,
) -> Result<TopologyProfile, ProfileError> {
    match name {
        None => Ok(TopologyProfile::builtin(Topology::CrossDevice)),
        Some(name) => match Topology::ALL.iter().find(|t| t.label() == name) {
            Some(t) => Ok(TopologyProfile::builtin(*t)),
            None => load_topology_profile(dir, name),
        },
    }
}

/// The mode profile a name selects — see [`topology_profile_named`]; the
/// default is `research`.
pub fn mode_profile_named(dir: &Path, name: Option<&str>) -> Result<ModeProfile, ProfileError> {
    match name {
        None => Ok(ModeProfile::builtin(Mode::Research)),
        Some(name) => match Mode::ALL.iter().find(|m| m.label() == name) {
            Some(m) => Ok(ModeProfile::builtin(*m)),
            None => load_mode_profile(dir, name),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn vars(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn unset_variables_leave_every_field_none() {
        let o = overrides_from_vars(|_| None).unwrap();
        assert_eq!(o.aggregator, None);
        assert_eq!(o.quorum, None);
        assert_eq!(o.noise_multiplier, None);
    }

    #[test]
    fn set_variables_are_parsed_into_their_fields() {
        let m = vars(&[
            ("CONFLUX_AGGREGATOR", "krum"),
            ("CONFLUX_QUORUM", "7"),
            ("CONFLUX_NOISE_MULTIPLIER", "1.5"),
            ("CONFLUX_REPUTATION_FILTER_ENABLED", "true"),
        ]);
        let o = overrides_from_vars(|k| m.get(k).cloned()).unwrap();
        assert_eq!(o.aggregator.as_deref(), Some("krum"));
        assert_eq!(o.quorum, Some(7));
        assert_eq!(o.noise_multiplier, Some(1.5));
        assert_eq!(o.reputation_filter_enabled, Some(true));
    }

    #[test]
    fn a_malformed_value_is_an_error_naming_the_variable() {
        let m = vars(&[("CONFLUX_QUORUM", "seven")]);
        let err = overrides_from_vars(|k| m.get(k).cloned()).unwrap_err();
        assert_eq!(
            err.to_string(),
            "CONFLUX_QUORUM=\"seven\" is not a valid value"
        );
    }

    #[test]
    fn a_builtin_label_selects_the_builtin_and_none_the_default() {
        let dir = Path::new("/nonexistent-profile-dir");
        assert_eq!(
            topology_profile_named(dir, None).unwrap().base,
            Topology::CrossDevice
        );
        assert_eq!(
            topology_profile_named(dir, Some("cross_silo"))
                .unwrap()
                .base,
            Topology::CrossSilo
        );
        assert_eq!(mode_profile_named(dir, None).unwrap().base, Mode::Research);
        assert_eq!(
            mode_profile_named(dir, Some("production")).unwrap().base,
            Mode::Production
        );
    }

    #[test]
    fn an_unknown_name_is_an_error_listing_the_builtins() {
        let dir = Path::new("/nonexistent-profile-dir");
        let err = topology_profile_named(dir, Some("cros_silo"))
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("cros_silo") && err.contains("cross_silo"),
            "{err}"
        );
    }
}
