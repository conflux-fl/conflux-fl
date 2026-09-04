//! Profile files: topology and mode profiles defined in TOML, extending
//! a base via `inherits`.
//!
//! The four topologies and two modes ship as compiled-in defaults, and
//! that is deliberately still true — a builtin profile involves no I/O
//! and cannot be misspelled into silence. What this module adds is the
//! framework's promise that **a new special case is config, never a code
//! change**: a deployment that is "cross_silo, but slower" writes
//!
//! ```toml
//! # profiles/hospital_silo.toml
//! inherits = "cross_silo"
//! round_timeout_secs = 1800
//! ```
//!
//! and sets `CONFLUX_TOPOLOGY=hospital_silo`. Everything not overridden
//! falls through to the base, and every resolved parameter's startup log
//! line names the file in the chain that actually set it (the chain is
//! said out loud, not just the winner).
//!
//! # Rules, and why each exists
//!
//! - **`inherits` is required**, and the chain must end at a builtin.
//!   A profile from nothing would need its own answer for every
//!   parameter, which is how two profiles drift apart; extending is the
//!   only mode offered because it is the only one that stays coherent.
//! - **A profile may only set its own axis's parameters** (the two axes
//!   own disjoint sets). A topology profile setting
//!   `allow_stub_client` is told it is a mode parameter — not "unknown
//!   key", which would send someone hunting for a typo that isn't there.
//! - **A profile may not shadow a builtin name.** `cross_device.toml`
//!   silently replacing the builtin would make one deployment's
//!   `cross_device` mean something different from everyone else's.
//! - **Unknown keys fail with a suggestion** when one is close enough
//!   to be a plausible typo, and the full valid set either way.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::types::{Mode, ModeDefaults, Topology, TopologyDefaults};

/// The topology-axis parameters a profile file may set. One entry per
/// `TopologyDefaults` field — the compiler cannot check that
/// correspondence, so `tests::every_axis_key_round_trips` does.
const TOPOLOGY_KEYS: &[&str] = &[
    "connection_mode",
    "auth",
    "round_timeout_secs",
    "min_reputation_score",
    "client_registry_ttl",
];

/// The mode-axis parameters a profile file may set.
const MODE_KEYS: &[&str] = &[
    "seed_mode",
    "seed_value",
    "budget_exhausted_action",
    "accounting_scope",
    "allow_stub_client",
    "require_node_auth",
    "config_log_format",
];

/// Why a profile file could not become a profile.
///
/// Every variant is a startup error a person will read once and act on,
/// so each carries what to *do*, not only what went wrong.
#[derive(Debug, thiserror::Error)]
pub enum ProfileError {
    #[error(
        "no {axis} profile named \"{name}\": not a builtin ({builtins}) and no \
         {name}.toml in {dir} (found: {available})"
    )]
    /// The name is neither a builtin nor a file in the profile directory.
    NotFound {
        /// Which axis was being resolved.
        axis: &'static str,
        /// The name that resolved to nothing.
        name: String,
        /// The directory searched.
        dir: String,
        /// The builtin names for this axis.
        builtins: String,
        /// What `.toml` files the directory actually contains, or a note
        /// that it does not exist.
        available: String,
    },

    #[error("could not read {path}: {source}")]
    /// The file exists but could not be read.
    Io {
        /// The file.
        path: String,
        /// The underlying error.
        source: std::io::Error,
    },

    #[error("{path} is not valid TOML: {message}")]
    /// The file is not parseable TOML.
    Parse {
        /// The file.
        path: String,
        /// The TOML parser's message.
        message: String,
    },

    #[error(
        "profile \"{name}\" shadows the builtin {axis} \"{name}\" — rename the file. \
         A file silently replacing a builtin would make one deployment's \"{name}\" \
         mean something different from every other deployment's"
    )]
    /// A profile file uses a builtin's name.
    ShadowsBuiltin {
        /// Which axis.
        axis: &'static str,
        /// The shadowed name.
        name: String,
    },

    #[error(
        "profile \"{name}\" has no `inherits`. Every profile extends a base — \
         ultimately one of the builtins ({builtins}) — and overrides only what \
         differs; a profile built from nothing would need its own answer for \
         every parameter, which is how configurations drift apart"
    )]
    /// The file has no `inherits` key.
    MissingInherits {
        /// The profile missing it.
        name: String,
        /// The builtin names it could extend.
        builtins: String,
    },

    #[error("profile inheritance cycle: {chain}")]
    /// `inherits` loops back on itself.
    Cycle {
        /// The chain, rendered `a → b → a`.
        chain: String,
    },

    #[error("{key} is not a {axis} parameter{suggestion} — the {axis} profile keys are: {valid}")]
    /// A key that belongs to no axis.
    UnknownKey {
        /// Which axis's profile contained it.
        axis: &'static str,
        /// The offending key.
        key: String,
        /// `", did you mean \"…\"?"` when something is close.
        suggestion: String,
        /// The full valid set.
        valid: String,
    },

    #[error(
        "{key} is a {owner}-axis parameter, but \"{name}\" is a {axis} profile — the \
         two axes own disjoint parameter sets, so put it in your {owner} profile \
         instead"
    )]
    /// A real parameter, wrong axis — deliberately distinct from
    /// [`Self::UnknownKey`], because "you misspelled something" and "you
    /// put it in the wrong file" send a person to different places.
    WrongAxis {
        /// The profile that contained the key.
        name: String,
        /// The axis of that profile.
        axis: &'static str,
        /// The key.
        key: String,
        /// The axis that owns the key.
        owner: &'static str,
    },

    #[error("{key} in profile \"{name}\": {message}")]
    /// The key is right, the value is not.
    BadValue {
        /// The profile.
        name: String,
        /// The key.
        key: String,
        /// What the deserializer reported.
        message: String,
    },
}

/// A loaded topology profile: the merged defaults, the base builtin it
/// terminates at, and — for the startup log's provenance — which link in
/// the chain set each parameter.
#[derive(Debug, Clone)]
pub struct TopologyProfile {
    /// The name resolution selects it by.
    pub name: String,
    /// The builtin the `inherits` chain terminates at. This is what
    /// `ResolvedConfig.topology` reports: a custom profile *is* its
    /// base, behaviorally, everywhere the enum is matched.
    pub base: Topology,
    /// The merged parameter values.
    pub defaults: TopologyDefaults,
    /// The full chain, custom-most first, base label last.
    pub chain: Vec<String>,
    /// Per-key: the chain entry that set it.
    origins: HashMap<&'static str, String>,
}

/// A loaded mode profile — see [`TopologyProfile`], same shape.
#[derive(Debug, Clone)]
pub struct ModeProfile {
    /// The name resolution selects it by.
    pub name: String,
    /// The builtin the chain terminates at.
    pub base: Mode,
    /// The merged parameter values.
    pub defaults: ModeDefaults,
    /// The full chain, custom-most first, base label last.
    pub chain: Vec<String>,
    origins: HashMap<&'static str, String>,
}

impl TopologyProfile {
    /// A builtin, wrapped — what [`crate::resolve`] uses, so the two
    /// entry points share one code path.
    pub fn builtin(topology: Topology) -> Self {
        Self {
            name: topology.label().to_string(),
            base: topology,
            defaults: topology.defaults(),
            chain: vec![topology.label().to_string()],
            origins: HashMap::new(),
        }
    }

    /// The provenance label for `key`: the profile's own name when the
    /// chain is trivial, otherwise `name → link-that-set-it`.
    pub(crate) fn source_label(&self, key: &'static str) -> String {
        source_label(&self.name, &self.origins, key)
    }
}

impl ModeProfile {
    /// A builtin, wrapped.
    pub fn builtin(mode: Mode) -> Self {
        Self {
            name: mode.label().to_string(),
            base: mode,
            defaults: mode.defaults(),
            chain: vec![mode.label().to_string()],
            origins: HashMap::new(),
        }
    }

    pub(crate) fn source_label(&self, key: &'static str) -> String {
        source_label(&self.name, &self.origins, key)
    }
}

fn source_label(name: &str, origins: &HashMap<&'static str, String>, key: &'static str) -> String {
    match origins.get(key) {
        // A builtin profile, or a value the custom-most file set itself.
        None => name.to_string(),
        Some(origin) if origin == name => name.to_string(),
        // Inherited: say which link actually set it. `hospital_silo →
        // cross_silo` reads as "selected as hospital_silo, value from
        // cross_silo".
        Some(origin) => format!("{name} → {origin}"),
    }
}

/// Loads the topology profile `name` from `dir`, following `inherits`
/// to a builtin.
pub fn load_topology_profile(dir: &Path, name: &str) -> Result<TopologyProfile, ProfileError> {
    let chain = load_chain("topology", dir, name, &topology_builtin_labels())?;
    let (files, base_label) = chain;
    let base = Topology::ALL
        .iter()
        .copied()
        .find(|t| t.label() == base_label)
        .expect("load_chain only terminates at a builtin label");

    let mut defaults = base.defaults();
    let mut origins: HashMap<&'static str, String> = HashMap::new();
    for key in TOPOLOGY_KEYS {
        origins.insert(key, base_label.clone());
    }

    // Base-most file first, so nearer files override farther ones.
    for (file_name, table) in files.iter().rev() {
        for (key, value) in table {
            let key_static = apply_topology_key(&mut defaults, name, key, value)?;
            origins.insert(key_static, file_name.clone());
        }
    }

    let mut chain_names: Vec<String> = files.iter().map(|(n, _)| n.clone()).collect();
    chain_names.push(base_label);
    Ok(TopologyProfile {
        name: name.to_string(),
        base,
        defaults,
        chain: chain_names,
        origins,
    })
}

/// Loads the mode profile `name` from `dir` — see
/// [`load_topology_profile`].
pub fn load_mode_profile(dir: &Path, name: &str) -> Result<ModeProfile, ProfileError> {
    let (files, base_label) = load_chain("mode", dir, name, &mode_builtin_labels())?;
    let base = Mode::ALL
        .iter()
        .copied()
        .find(|m| m.label() == base_label)
        .expect("load_chain only terminates at a builtin label");

    let mut defaults = base.defaults();
    let mut origins: HashMap<&'static str, String> = HashMap::new();
    for key in MODE_KEYS {
        origins.insert(key, base_label.clone());
    }
    for (file_name, table) in files.iter().rev() {
        for (key, value) in table {
            let key_static = apply_mode_key(&mut defaults, name, key, value)?;
            origins.insert(key_static, file_name.clone());
        }
    }

    let mut chain_names: Vec<String> = files.iter().map(|(n, _)| n.clone()).collect();
    chain_names.push(base_label);
    Ok(ModeProfile {
        name: name.to_string(),
        base,
        defaults,
        chain: chain_names,
        origins,
    })
}

type LoadedFile = (String, Vec<(String, toml::Value)>);

/// Walks `name`'s `inherits` chain until it reaches a builtin label,
/// returning each file's non-`inherits` keys (custom-most first) plus
/// the terminal builtin's label. Owns every chain-shaped error: not
/// found, shadowing, cycles, missing `inherits`.
fn load_chain(
    axis: &'static str,
    dir: &Path,
    name: &str,
    builtins: &[&'static str],
) -> Result<(Vec<LoadedFile>, String), ProfileError> {
    let builtins_list = builtins.join(", ");
    let mut files: Vec<LoadedFile> = Vec::new();
    let mut visited: Vec<String> = Vec::new();
    let mut current = name.to_string();

    loop {
        if builtins.contains(&current.as_str()) {
            // A builtin terminates the chain — but a builtin *file* must
            // not exist beside it, silently diverging from the compiled
            // one.
            if profile_path(dir, &current).exists() {
                return Err(ProfileError::ShadowsBuiltin {
                    axis,
                    name: current,
                });
            }
            return Ok((files, current));
        }
        if visited.contains(&current) {
            visited.push(current);
            return Err(ProfileError::Cycle {
                chain: visited.join(" → "),
            });
        }
        visited.push(current.clone());

        let path = profile_path(dir, &current);
        if !path.exists() {
            return Err(ProfileError::NotFound {
                axis,
                name: current,
                dir: dir.display().to_string(),
                builtins: builtins_list,
                available: available_profiles(dir),
            });
        }
        let text = std::fs::read_to_string(&path).map_err(|source| ProfileError::Io {
            path: path.display().to_string(),
            source,
        })?;
        let table: toml::Table =
            text.parse()
                .map_err(|e: toml::de::Error| ProfileError::Parse {
                    path: path.display().to_string(),
                    message: e.message().to_string(),
                })?;

        let mut inherits: Option<String> = None;
        let mut keys: Vec<(String, toml::Value)> = Vec::new();
        for (key, value) in table {
            if key == "inherits" {
                inherits = value.as_str().map(str::to_string);
                if inherits.is_none() {
                    return Err(ProfileError::BadValue {
                        name: current.clone(),
                        key,
                        message: "expected a profile or builtin name as a string".to_string(),
                    });
                }
            } else {
                keys.push((key, value));
            }
        }
        let Some(parent) = inherits else {
            return Err(ProfileError::MissingInherits {
                name: current,
                builtins: builtins_list,
            });
        };
        files.push((current, keys));
        current = parent;
    }
}

fn profile_path(dir: &Path, name: &str) -> PathBuf {
    dir.join(format!("{name}.toml"))
}

/// What `.toml` files `dir` holds — for the not-found error, so the
/// person sees what they *can* select instead of an empty rebuke.
fn available_profiles(dir: &Path) -> String {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return format!("directory {} does not exist", dir.display());
    };
    let mut names: Vec<String> = entries
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let p = e.path();
            (p.extension()? == "toml").then(|| p.file_stem()?.to_str().map(str::to_string))?
        })
        .collect();
    names.sort();
    if names.is_empty() {
        "no .toml files".to_string()
    } else {
        names.join(", ")
    }
}

fn topology_builtin_labels() -> Vec<&'static str> {
    Topology::ALL.iter().map(|t| t.label()).collect()
}

fn mode_builtin_labels() -> Vec<&'static str> {
    Mode::ALL.iter().map(|m| m.label()).collect()
}

/// Applies one topology-axis key, or explains precisely why it cannot.
fn apply_topology_key(
    defaults: &mut TopologyDefaults,
    profile: &str,
    key: &str,
    value: &toml::Value,
) -> Result<&'static str, ProfileError> {
    let parse = |message: String| ProfileError::BadValue {
        name: profile.to_string(),
        key: key.to_string(),
        message,
    };
    match key {
        "connection_mode" => {
            defaults.connection_mode = value
                .clone()
                .try_into()
                .map_err(|e: toml::de::Error| parse(e.message().to_string()))?;
            Ok("connection_mode")
        }
        "auth" => {
            defaults.auth = value
                .clone()
                .try_into()
                .map_err(|e: toml::de::Error| parse(e.message().to_string()))?;
            Ok("auth")
        }
        "round_timeout_secs" => {
            defaults.round_timeout_secs = value
                .clone()
                .try_into()
                .map_err(|e: toml::de::Error| parse(e.message().to_string()))?;
            Ok("round_timeout_secs")
        }
        "min_reputation_score" => {
            defaults.min_reputation_score = value
                .clone()
                .try_into()
                .map_err(|e: toml::de::Error| parse(e.message().to_string()))?;
            Ok("min_reputation_score")
        }
        "client_registry_ttl" => {
            defaults.client_registry_ttl = value
                .clone()
                .try_into()
                .map_err(|e: toml::de::Error| parse(e.message().to_string()))?;
            Ok("client_registry_ttl")
        }
        other => Err(wrong_key(
            "topology",
            profile,
            other,
            TOPOLOGY_KEYS,
            MODE_KEYS,
        )),
    }
}

/// Applies one mode-axis key.
fn apply_mode_key(
    defaults: &mut ModeDefaults,
    profile: &str,
    key: &str,
    value: &toml::Value,
) -> Result<&'static str, ProfileError> {
    let parse = |message: String| ProfileError::BadValue {
        name: profile.to_string(),
        key: key.to_string(),
        message,
    };
    match key {
        "seed_mode" => {
            defaults.seed_mode = value
                .clone()
                .try_into()
                .map_err(|e: toml::de::Error| parse(e.message().to_string()))?;
            Ok("seed_mode")
        }
        "seed_value" => {
            defaults.seed_value = Some(
                value
                    .clone()
                    .try_into()
                    .map_err(|e: toml::de::Error| parse(e.message().to_string()))?,
            );
            Ok("seed_value")
        }
        "budget_exhausted_action" => {
            defaults.budget_exhausted_action = value
                .clone()
                .try_into()
                .map_err(|e: toml::de::Error| parse(e.message().to_string()))?;
            Ok("budget_exhausted_action")
        }
        "accounting_scope" => {
            defaults.accounting_scope = value
                .clone()
                .try_into()
                .map_err(|e: toml::de::Error| parse(e.message().to_string()))?;
            Ok("accounting_scope")
        }
        "allow_stub_client" => {
            defaults.allow_stub_client = value
                .clone()
                .try_into()
                .map_err(|e: toml::de::Error| parse(e.message().to_string()))?;
            Ok("allow_stub_client")
        }
        "require_node_auth" => {
            defaults.require_node_auth = value
                .clone()
                .try_into()
                .map_err(|e: toml::de::Error| parse(e.message().to_string()))?;
            Ok("require_node_auth")
        }
        "config_log_format" => {
            defaults.config_log_format = value
                .clone()
                .try_into()
                .map_err(|e: toml::de::Error| parse(e.message().to_string()))?;
            Ok("config_log_format")
        }
        other => Err(wrong_key("mode", profile, other, MODE_KEYS, TOPOLOGY_KEYS)),
    }
}

/// The unknown-key decision: is it the *other* axis's parameter (a
/// placement mistake), or nobody's (a typo, with a suggestion when one
/// is plausible)?
fn wrong_key(
    axis: &'static str,
    profile: &str,
    key: &str,
    own_keys: &[&'static str],
    other_keys: &[&'static str],
) -> ProfileError {
    if other_keys.contains(&key) {
        let owner = if axis == "topology" {
            "mode"
        } else {
            "topology"
        };
        return ProfileError::WrongAxis {
            name: profile.to_string(),
            axis,
            key: key.to_string(),
            owner,
        };
    }
    let suggestion = own_keys
        .iter()
        .map(|k| (k, levenshtein(key, k)))
        .filter(|(_, d)| *d <= 2)
        .min_by_key(|(_, d)| *d)
        .map(|(k, _)| format!(", did you mean \"{k}\"?"))
        .unwrap_or_default();
    ProfileError::UnknownKey {
        axis,
        key: key.to_string(),
        suggestion,
        valid: own_keys.join(", "),
    }
}

/// Plain Levenshtein distance, small inputs only. Not worth a
/// dependency: the longest key is 23 bytes and this runs once, at
/// startup, on the error path.
fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0usize; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        cur[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            cur[j + 1] = (prev[j + 1] + 1).min(cur[j] + 1).min(prev[j] + cost);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static DIR_SEQ: AtomicUsize = AtomicUsize::new(0);

    /// A fresh directory per test, under the OS temp dir — real files,
    /// because the loader's whole job is reading real files.
    fn dir() -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "conflux-profile-tests-{}-{}",
            std::process::id(),
            DIR_SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn write(dir: &Path, name: &str, body: &str) {
        std::fs::write(dir.join(format!("{name}.toml")), body).unwrap();
    }

    #[test]
    fn overrides_apply_and_everything_else_inherits_from_the_base() {
        let d = dir();
        write(
            &d,
            "hospital_silo",
            "inherits = \"cross_silo\"\nround_timeout_secs = 1800\n",
        );

        let p = load_topology_profile(&d, "hospital_silo").unwrap();

        assert_eq!(p.base, Topology::CrossSilo);
        assert_eq!(p.defaults.round_timeout_secs, 1800);
        // Untouched keys are the base's, verbatim.
        let base = Topology::CrossSilo.defaults();
        assert_eq!(p.defaults.auth, base.auth);
        assert_eq!(p.defaults.connection_mode, base.connection_mode);
        assert_eq!(
            p.chain,
            vec!["hospital_silo".to_string(), "cross_silo".to_string()]
        );
    }

    #[test]
    fn provenance_names_the_chain_link_that_actually_set_each_value() {
        let d = dir();
        write(
            &d,
            "hospital_silo",
            "inherits = \"cross_silo\"\nround_timeout_secs = 1800\n",
        );
        let p = load_topology_profile(&d, "hospital_silo").unwrap();

        // Set by the file itself: credited to the file.
        assert_eq!(p.source_label("round_timeout_secs"), "hospital_silo");
        // Inherited untouched: the chain is said out loud.
        assert_eq!(p.source_label("auth"), "hospital_silo → cross_silo");
    }

    #[test]
    fn a_two_file_chain_credits_the_middle_link() {
        let d = dir();
        write(
            &d,
            "region_base",
            "inherits = \"cross_device\"\nround_timeout_secs = 900\n",
        );
        write(
            &d,
            "clinic",
            "inherits = \"region_base\"\nmin_reputation_score = 0.5\n",
        );

        let p = load_topology_profile(&d, "clinic").unwrap();

        assert_eq!(p.base, Topology::CrossDevice);
        assert_eq!(p.defaults.round_timeout_secs, 900);
        assert_eq!(p.defaults.min_reputation_score, 0.5);
        assert_eq!(p.source_label("min_reputation_score"), "clinic");
        assert_eq!(p.source_label("round_timeout_secs"), "clinic → region_base");
        assert_eq!(p.source_label("auth"), "clinic → cross_device");
    }

    #[test]
    fn the_nearer_file_wins_when_both_links_set_a_key() {
        let d = dir();
        write(
            &d,
            "base",
            "inherits = \"edge\"\nround_timeout_secs = 100\n",
        );
        write(&d, "leaf", "inherits = \"base\"\nround_timeout_secs = 50\n");
        let p = load_topology_profile(&d, "leaf").unwrap();
        assert_eq!(p.defaults.round_timeout_secs, 50);
        assert_eq!(p.source_label("round_timeout_secs"), "leaf");
    }

    #[test]
    fn a_cycle_is_named_rather_than_looped() {
        let d = dir();
        write(&d, "a", "inherits = \"b\"\n");
        write(&d, "b", "inherits = \"a\"\n");
        let err = load_topology_profile(&d, "a").unwrap_err();
        match err {
            ProfileError::Cycle { chain } => assert_eq!(chain, "a → b → a"),
            other => panic!("expected Cycle, got {other}"),
        }
    }

    #[test]
    fn missing_inherits_is_refused_with_the_builtins_offered() {
        let d = dir();
        write(&d, "floating", "round_timeout_secs = 5\n");
        let err = load_topology_profile(&d, "floating").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("no `inherits`"), "{msg}");
        assert!(msg.contains("cross_silo"), "{msg}");
    }

    #[test]
    fn a_file_shadowing_a_builtin_name_is_refused() {
        let d = dir();
        write(&d, "cross_device", "inherits = \"cross_silo\"\n");
        // Both directly…
        let err = load_topology_profile(&d, "cross_device").unwrap_err();
        assert!(matches!(err, ProfileError::ShadowsBuiltin { .. }), "{err}");
        // …and when reached through a chain.
        write(&d, "child", "inherits = \"cross_device\"\n");
        let err = load_topology_profile(&d, "child").unwrap_err();
        assert!(matches!(err, ProfileError::ShadowsBuiltin { .. }), "{err}");
    }

    #[test]
    fn a_typo_gets_a_suggestion_and_the_valid_set() {
        let d = dir();
        write(&d, "t", "inherits = \"edge\"\nround_timout_secs = 60\n");
        let msg = load_topology_profile(&d, "t").unwrap_err().to_string();
        assert!(
            msg.contains("did you mean \"round_timeout_secs\"?"),
            "{msg}"
        );
        assert!(
            msg.contains("client_registry_ttl"),
            "the valid set is listed: {msg}"
        );
    }

    #[test]
    fn a_mode_parameter_in_a_topology_profile_is_a_placement_error_not_a_typo() {
        let d = dir();
        write(&d, "t", "inherits = \"edge\"\nallow_stub_client = false\n");
        let err = load_topology_profile(&d, "t").unwrap_err();
        let msg = err.to_string();
        assert!(matches!(err, ProfileError::WrongAxis { .. }), "{msg}");
        assert!(msg.contains("mode-axis parameter"), "{msg}");
        assert!(msg.contains("disjoint parameter sets"), "{msg}");
    }

    #[test]
    fn not_found_lists_what_is_actually_available() {
        let d = dir();
        write(&d, "other_profile", "inherits = \"edge\"\n");
        let msg = load_topology_profile(&d, "nope").unwrap_err().to_string();
        assert!(msg.contains("other_profile"), "{msg}");
        assert!(
            msg.contains("cross_silo"),
            "builtins are offered too: {msg}"
        );
    }

    #[test]
    fn a_bad_value_names_the_key_and_the_profile() {
        let d = dir();
        write(
            &d,
            "t",
            "inherits = \"edge\"\nround_timeout_secs = \"fast\"\n",
        );
        let msg = load_topology_profile(&d, "t").unwrap_err().to_string();
        assert!(msg.contains("round_timeout_secs"), "{msg}");
        assert!(msg.contains("\"t\""), "{msg}");
    }

    #[test]
    fn mode_profiles_work_the_same_way() {
        let d = dir();
        write(
            &d,
            "locked_down_research",
            "inherits = \"research\"\nallow_stub_client = false\nrequire_node_auth = true\n",
        );
        let p = load_mode_profile(&d, "locked_down_research").unwrap();
        assert_eq!(p.base, Mode::Research);
        assert!(!p.defaults.allow_stub_client);
        assert!(p.defaults.require_node_auth);
        assert_eq!(p.source_label("allow_stub_client"), "locked_down_research");
        assert_eq!(
            p.source_label("accounting_scope"),
            "locked_down_research → research"
        );
    }

    /// Guards the KEYS lists against drifting from the defaults structs:
    /// a file that sets every axis key must apply cleanly, and every
    /// origin must credit the file. If a field is added to
    /// `TopologyDefaults`/`ModeDefaults` without extending this module,
    /// this test's fixture stops covering it — update both.
    #[test]
    fn every_axis_key_round_trips() {
        let d = dir();
        write(
            &d,
            "all_topo",
            "inherits = \"cross_silo\"\n\
             connection_mode = \"pull\"\n\
             auth = \"jwt\"\n\
             round_timeout_secs = 7\n\
             min_reputation_score = 0.25\n\
             client_registry_ttl = 99\n",
        );
        let p = load_topology_profile(&d, "all_topo").unwrap();
        for key in TOPOLOGY_KEYS {
            assert_eq!(p.source_label(key), "all_topo", "{key} did not apply");
        }

        write(
            &d,
            "all_mode",
            "inherits = \"production\"\n\
             seed_mode = \"fixed\"\n\
             seed_value = 42\n\
             budget_exhausted_action = \"halt\"\n\
             accounting_scope = \"global\"\n\
             allow_stub_client = false\n\
             require_node_auth = true\n\
             config_log_format = \"json\"\n",
        );
        let m = load_mode_profile(&d, "all_mode").unwrap();
        for key in MODE_KEYS {
            assert_eq!(m.source_label(key), "all_mode", "{key} did not apply");
        }
    }
}
