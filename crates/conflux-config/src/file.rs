//! Reading an experiment's config file into an [`Overrides`].
//!
//! This is the *narrow* half of spec §11 Open Item 2: one flat TOML file
//! of experiment-level overrides, feeding the `file` tier
//! [`crate::resolve`] has accepted but which nothing ever
//! supplied a real value for. Making topology/mode **profiles**
//! themselves TOML-defined, with `inherits`-based extension between
//! them, is the other half — materially larger, and deliberately not
//! here (see its phase brief).
//!
//! Nothing about resolution changes. The layering chain, the precedence
//! order, and the [`crate::ConfigSource::ExperimentFile`] provenance
//! label all already existed and are already tested; this module only
//! produces a value to put into them.

use std::path::Path;

use crate::Overrides;

/// Why an experiment config file couldn't be turned into [`Overrides`].
///
/// Four variants rather than one, because "your config is broken" is not
/// an actionable message and these four failures need four different
/// responses from whoever hit them: fix the path, fix the syntax, fix a
/// value's type, or fix a key's spelling. ADR 0007 requires a resolved
/// value to explain where it came from; the same standard applied to
/// failure means saying which of these happened, and where.
#[derive(Debug, thiserror::Error)]
pub enum ConfigFileError {
    #[error("experiment config file not found: {path}")]
    /// No file at that path. An operator named a config file that isn't
    /// there — a typo, or a relative path resolved from an unexpected
    /// working directory.
    NotFound {
        /// The path that was looked for.
        path: String,
    },
    #[error("could not read experiment config file {path}: {source}")]
    /// The file exists but could not be read: permissions, usually.
    Unreadable {
        /// The path that could not be read.
        path: String,
        /// The underlying OS error.
        #[source]
        source: std::io::Error,
    },
    /// The file isn't valid TOML at all — a missing quote, an unclosed
    /// bracket. Caught before any field is looked at, so the message can
    /// say "this isn't TOML" rather than blaming whichever key the
    /// parser happened to choke near.
    #[error("experiment config file {path} is not valid TOML: {message}")]
    Syntax {
        /// The file that failed to parse.
        path: String,
        /// What the TOML parser reported.
        message: String,
    },
    /// The file is valid TOML, but doesn't describe an `Overrides`: a
    /// value of the wrong type (`quorum = "three"`), an unknown key
    /// (usually a typo), or an enum spelled with a value that isn't one
    /// of its variants.
    #[error("experiment config file {path} has an invalid setting: {message}")]
    Schema {
        /// The file whose contents don't describe an `Overrides`.
        path: String,
        /// What the deserializer reported — a wrong type, an unknown key,
        /// or an enum value that isn't one of its variants.
        message: String,
    },
}

/// Reads `path` as a flat TOML file of experiment-level overrides.
///
/// The schema is deliberately the simplest thing that closes the gap:
/// top-level keys named exactly like [`Overrides`]' fields, no nested
/// tables, no `inherits`. Any subset may be present; anything absent
/// means "this tier has no opinion," and resolution falls through to the
/// mode profile, topology profile, or builtin fallback as it always has.
///
/// ```toml
/// aggregator = "centered_clipping"
/// clip_radius = 4.0
/// quorum = 8
/// auth = "mtls"
/// ```
///
/// Parsing happens in two stages — text to a generic [`toml::Table`],
/// then that table to `Overrides` — purely so the two failure modes stay
/// distinguishable. A single-stage `toml::from_str::<Overrides>` would
/// report a malformed file and a mistyped field through the same error
/// type, and the caller could not tell a syntax problem from a schema
/// problem well enough to say anything useful about it.
pub fn load_experiment_file(path: &Path) -> Result<Overrides, ConfigFileError> {
    let display = path.display().to_string();

    let text = std::fs::read_to_string(path).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => ConfigFileError::NotFound {
            path: display.clone(),
        },
        _ => ConfigFileError::Unreadable {
            path: display.clone(),
            source: e,
        },
    })?;

    // Stage 1: text -> a generic TOML document. `toml::Table` rather
    // than `str::parse::<toml::Value>()`, which in toml 0.9 parses a
    // bare *value* (`42`, `"x"`) and rejects a whole document.
    let table: toml::Table =
        toml::from_str(&text).map_err(|e: toml::de::Error| ConfigFileError::Syntax {
            path: display.clone(),
            message: e.message().to_string(),
        })?;

    // Stage 2: that document -> this crate's own schema.
    toml::Value::Table(table)
        .try_into()
        .map_err(|e: toml::de::Error| ConfigFileError::Schema {
            path: display,
            message: e.message().to_string(),
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AuthMode, ConnectionMode, SeedMode};

    /// Writes `contents` to a uniquely-named file under the OS temp dir
    /// and hands back the path.
    ///
    /// The name mixes the process id with a per-call counter for the
    /// same reason `redis_registry.rs`'s test keys do: a
    /// counter alone still collides across two separate `cargo test`
    /// invocations running at once, which is exactly the flake that was
    /// found and fixed there rather than re-discovered here.
    fn temp_toml(contents: &str) -> std::path::PathBuf {
        use std::sync::atomic::{AtomicU32, Ordering};
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!(
            "conflux-experiment-{}-{}.toml",
            std::process::id(),
            n
        ));
        std::fs::write(&path, contents).expect("failed to write temp config");
        path
    }

    #[test]
    fn a_partial_file_sets_only_the_keys_it_names() {
        let path = temp_toml(
            r#"
            aggregator = "centered_clipping"
            clip_radius = 4.0
            quorum = 8
            "#,
        );
        let overrides = load_experiment_file(&path).unwrap();

        assert_eq!(overrides.aggregator.as_deref(), Some("centered_clipping"));
        assert_eq!(overrides.clip_radius, Some(4.0));
        assert_eq!(overrides.quorum, Some(8));
        // Everything the file didn't mention stays "no opinion" — the
        // property that makes a partial file usable at all.
        assert_eq!(overrides.selector, None);
        assert_eq!(overrides.robust_byzantine_fraction, None);
        assert_eq!(overrides.auth, None);
        assert_eq!(overrides.target_epsilon, None);

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn an_empty_file_is_valid_and_means_no_opinion_about_anything() {
        let path = temp_toml("");
        let overrides = load_experiment_file(&path).unwrap();

        assert!(overrides.aggregator.is_none());
        assert!(overrides.quorum.is_none());

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn enum_valued_keys_use_each_enums_own_spelling() {
        let path = temp_toml(
            r#"
            connection_mode = "push"
            auth = "mtls"
            seed_mode = "os_random"
            "#,
        );
        let overrides = load_experiment_file(&path).unwrap();

        assert_eq!(overrides.connection_mode, Some(ConnectionMode::Push));
        assert_eq!(overrides.auth, Some(AuthMode::Mtls));
        assert_eq!(overrides.seed_mode, Some(SeedMode::OsRandom));

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn a_missing_file_is_distinguishable_from_a_broken_one() {
        let err = load_experiment_file(Path::new("/nonexistent/conflux/experiment.toml"))
            .expect_err("a missing file should not resolve");

        assert!(matches!(err, ConfigFileError::NotFound { .. }), "{err:?}");
        // The path has to be in the message — "not found" alone doesn't
        // tell an operator which path was actually looked at.
        assert!(
            err.to_string()
                .contains("/nonexistent/conflux/experiment.toml")
        );
    }

    #[test]
    fn malformed_toml_reports_a_syntax_error_not_a_schema_error() {
        let path = temp_toml("aggregator = \"unterminated\n[oops");
        let err = load_experiment_file(&path).expect_err("malformed TOML should not parse");

        assert!(matches!(err, ConfigFileError::Syntax { .. }), "{err:?}");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn a_wrong_typed_value_reports_a_schema_error_not_a_syntax_error() {
        // Valid TOML, invalid Overrides — the distinction the two-stage
        // parse exists to preserve.
        let path = temp_toml(r#"robust_byzantine_fraction = "not a number""#);
        let err = load_experiment_file(&path).expect_err("a string is not an f32");

        assert!(matches!(err, ConfigFileError::Schema { .. }), "{err:?}");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn an_unknown_key_is_refused_rather_than_silently_ignored() {
        // The failure this test exists for: without `deny_unknown_fields`
        // this file parses fine, the typo is dropped, and the resolved
        // config reports `aggregator` as a builtin fallback — a config
        // that lies about its own provenance.
        let path = temp_toml(r#"agregator = "krum""#);
        let err = load_experiment_file(&path).expect_err("a typo'd key should be refused");

        assert!(matches!(err, ConfigFileError::Schema { .. }), "{err:?}");
        assert!(
            err.to_string().contains("agregator"),
            "the message must name the offending key: {err}"
        );
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn an_invalid_enum_value_is_refused() {
        let path = temp_toml(r#"auth = "kerberos""#);
        let err = load_experiment_file(&path).expect_err("kerberos is not an AuthMode");

        assert!(matches!(err, ConfigFileError::Schema { .. }), "{err:?}");
        std::fs::remove_file(&path).ok();
    }

    // --- integration with resolve(): the file tier is not a new code
    // path, it is a real value for one that already existed -----------

    #[test]
    fn a_parsed_file_resolves_identically_to_a_hand_built_overrides() {
        use crate::{ConfigSource, Mode, Topology, resolve};

        // Exactly the value `file_wins_over_topology_and_mode` builds by
        // hand in lib.rs's own tests. If parsing and hand-construction
        // ever diverge, this catches it — the point of the phase was to
        // feed the existing layering, not to add a second one.
        let path = temp_toml("round_timeout_secs = 120");
        let parsed = load_experiment_file(&path).unwrap();
        let hand_built = Overrides {
            round_timeout_secs: Some(120),
            ..Default::default()
        };

        let label = path.display().to_string();
        let from_file = resolve(
            Topology::CrossDevice,
            Mode::Research,
            Some((&label, &parsed)),
            &Overrides::default(),
            &Overrides::default(),
        )
        .unwrap();
        let from_hand = resolve(
            Topology::CrossDevice,
            Mode::Research,
            Some((&label, &hand_built)),
            &Overrides::default(),
            &Overrides::default(),
        )
        .unwrap();

        assert_eq!(from_file.round_timeout_secs.value, 120);
        assert_eq!(
            from_file.round_timeout_secs.value,
            from_hand.round_timeout_secs.value
        );
        // And the provenance names the real path, not a placeholder —
        // "which file said this?" is the question ADR 0007 exists to
        // answer, and it is only useful if the label is the actual path.
        assert_eq!(
            from_file.round_timeout_secs.source,
            ConfigSource::ExperimentFile(label)
        );

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn an_env_var_still_beats_the_same_key_in_a_file() {
        use crate::{ConfigSource, Mode, Topology, resolve};

        // Precedence is pre-existing and already tested with
        // hand-built overrides; this asserts a *parsed* file lands in
        // the same tier rather than accidentally outranking env.
        let path = temp_toml("clip_norm = 2.0");
        let parsed = load_experiment_file(&path).unwrap();
        let env = Overrides {
            clip_norm: Some(3.0),
            ..Default::default()
        };

        let label = path.display().to_string();
        let resolved = resolve(
            Topology::CrossSilo,
            Mode::Research,
            Some((&label, &parsed)),
            &env,
            &Overrides::default(),
        )
        .unwrap();

        assert_eq!(resolved.clip_norm.value, 3.0);
        assert_eq!(
            resolved.clip_norm.source,
            ConfigSource::EnvVar("CONFLUX_CLIP_NORM".to_string())
        );

        std::fs::remove_file(&path).ok();
    }
}
