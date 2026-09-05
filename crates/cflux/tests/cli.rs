//! `cflux` end to end: the built binary, its exit codes, and both output
//! formats — the contract scripts and CI depend on.

use std::process::{Command, Output};

fn cflux(args: &[&str], env: &[(&str, &str)]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_cflux"));
    cmd.args(args);
    // A clean CONFLUX_* environment, so a developer's shell cannot leak
    // into the assertions; only what the test sets is visible.
    for (key, _) in std::env::vars() {
        if key.starts_with("CONFLUX_") {
            cmd.env_remove(key);
        }
    }
    for (k, v) in env {
        cmd.env(k, v);
    }
    cmd.output().expect("run cflux")
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

/// A fresh directory under the target dir, so a test that writes files
/// never touches the checkout and two tests never collide.
fn temp_dir(tag: &str) -> std::path::PathBuf {
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(tag);
    std::fs::remove_dir_all(&dir).ok();
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

#[test]
fn catalog_list_names_every_registered_method() {
    let o = cflux(&["catalog", "list"], &[]);
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    let out = stdout(&o);
    for name in ["fedavg", "krum", "uniform_random", "gaussian_clipping"] {
        assert!(out.contains(name), "missing {name} in:\n{out}");
    }
    assert!(out.contains("[needs trusted-reference sidecar]"), "{out}");

    let o = cflux(&["--format", "json", "catalog", "list"], &[]);
    let json: serde_json::Value = serde_json::from_slice(&o.stdout).expect("json");
    let aggregators = json["aggregators"].as_array().expect("array");
    assert!(aggregators.len() >= 22, "{} aggregators", aggregators.len());
    assert!(
        aggregators
            .iter()
            .any(|a| a["name"] == "fltrust" && a["sidecar"] == "trusted-reference")
    );
}

#[test]
fn catalog_describe_reports_family_paper_and_needs() {
    let o = cflux(&["catalog", "describe", "krum"], &[]);
    assert_eq!(o.status.code(), Some(0));
    let out = stdout(&o);
    assert!(
        out.contains("family:    robust") && out.contains("Blanchard"),
        "{out}"
    );
    assert!(out.contains("robust_byzantine_fraction"), "{out}");

    let o = cflux(&["catalog", "describe", "no_such_method"], &[]);
    assert_eq!(o.status.code(), Some(1));
    assert!(stdout(&o).contains("known:"), "{}", stdout(&o));
}

#[test]
fn config_check_of_the_builtin_default_is_valid() {
    let o = cflux(&["config", "check"], &[]);
    assert_eq!(o.status.code(), Some(0), "{}{}", stdout(&o), stderr(&o));
    let out = stdout(&o);
    assert!(
        out.contains("topology: cross_device") && out.contains("✓ configuration is valid"),
        "{out}"
    );
    assert!(out.contains("nothing was started"), "{out}");
}

#[test]
fn config_check_reports_an_error_finding_and_exits_1() {
    let o = cflux(&["config", "check"], &[("CONFLUX_ROUND_TIMEOUT_SECS", "0")]);
    assert_eq!(o.status.code(), Some(1), "{}", stdout(&o));
    let out = stdout(&o);
    assert!(
        out.contains("error:") && out.contains("round_timeout_secs"),
        "{out}"
    );

    let o = cflux(
        &["--format", "json", "config", "check"],
        &[("CONFLUX_ROUND_TIMEOUT_SECS", "0")],
    );
    assert_eq!(o.status.code(), Some(1));
    let json: serde_json::Value = serde_json::from_slice(&o.stdout).expect("json");
    assert_eq!(json["ok"], false);
    assert!(
        json["findings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| { f["severity"] == "error" && f["parameter"] == "round_timeout_secs" }),
        "{json}"
    );
    assert!(!json["parameters"].as_array().unwrap().is_empty());
}

#[test]
fn flags_win_over_the_environment() {
    let o = cflux(
        &[
            "config",
            "resolve",
            "--topology",
            "cross_silo",
            "--mode",
            "production",
        ],
        &[("CONFLUX_TOPOLOGY", "edge")],
    );
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    let out = stdout(&o);
    assert!(
        out.contains("topology: cross_silo") && out.contains("mode:     production"),
        "{out}"
    );
}

#[test]
fn a_malformed_variable_or_unknown_profile_cannot_run_and_exits_2() {
    let o = cflux(&["config", "check"], &[("CONFLUX_QUORUM", "seven")]);
    assert_eq!(o.status.code(), Some(2));
    assert!(stderr(&o).contains("CONFLUX_QUORUM"), "{}", stderr(&o));

    let o = cflux(&["config", "check", "--topology", "cros_silo"], &[]);
    assert_eq!(o.status.code(), Some(2));
    assert!(stderr(&o).contains("cross_silo"), "{}", stderr(&o));

    let o = cflux(
        &["--format", "json", "config", "check"],
        &[("CONFLUX_QUORUM", "seven")],
    );
    assert_eq!(o.status.code(), Some(2));
    let json: serde_json::Value = serde_json::from_slice(&o.stdout).expect("json");
    assert_eq!(json["ok"], false);
}

#[test]
fn init_scaffolds_two_profiles_that_change_nothing_until_edited() {
    let dir = temp_dir("init-basic");
    let o = cflux(
        &["init", "--name", "hospital", "--dir", dir.to_str().unwrap()],
        &[],
    );
    assert_eq!(o.status.code(), Some(0), "{}{}", stdout(&o), stderr(&o));

    let topology =
        std::fs::read_to_string(dir.join("profiles/hospital.toml")).expect("topology profile");
    let mode =
        std::fs::read_to_string(dir.join("profiles/hospital_mode.toml")).expect("mode profile");
    assert!(topology.contains("inherits = \"cross_silo\""), "{topology}");
    assert!(mode.contains("inherits = \"production\""), "{mode}");
    // Scaffolding must not change behavior: `inherits` is the only line
    // that is not a comment in either file.
    for text in [&topology, &mode] {
        let active: Vec<&str> = text
            .lines()
            .filter(|l| !l.trim_start().starts_with('#') && !l.trim().is_empty())
            .collect();
        assert_eq!(active.len(), 1, "only `inherits` should be active:\n{text}");
    }
    // And the profiles it wrote are the ones the rest of the tool reads.
    let o = cflux(
        &[
            "config",
            "resolve",
            "--topology",
            "hospital",
            "--mode",
            "hospital_mode",
            "--profile-dir",
            dir.join("profiles").to_str().unwrap(),
        ],
        &[],
    );
    assert_eq!(o.status.code(), Some(0), "{}{}", stdout(&o), stderr(&o));
    assert!(
        stdout(&o).contains("hospital → cross_silo"),
        "{}",
        stdout(&o)
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_fully_uncommented_scaffold_is_still_valid_input_to_the_loader() {
    // The scaffold's whole promise is that every commented key is ready
    // to uncomment. Only the loader can say whether that holds: a key
    // spelled wrong, or a value of a type it refuses, looks fine as text
    // and fails the first time someone acts on it.
    let dir = temp_dir("init-uncommented");
    let o = cflux(
        &["init", "--name", "wide", "--dir", dir.to_str().unwrap()],
        &[],
    );
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));

    let profiles = dir.join("profiles");
    for file in ["wide.toml", "wide_mode.toml"] {
        let path = profiles.join(file);
        let text = std::fs::read_to_string(&path).expect("profile");
        let uncommented: String = text
            .lines()
            .map(|line| match line.strip_prefix("# ") {
                // Only the `key = value` lines; the prose stays comment.
                Some(rest)
                    if rest.split_once(" = ").is_some_and(|(k, _)| {
                        !k.is_empty() && k.chars().all(|c| c.is_ascii_lowercase() || c == '_')
                    }) =>
                {
                    rest.to_string()
                }
                _ => line.to_string(),
            })
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(&path, uncommented).expect("rewrite profile");
    }

    let o = cflux(
        &[
            "config",
            "resolve",
            "--topology",
            "wide",
            "--mode",
            "wide_mode",
            "--profile-dir",
            profiles.to_str().unwrap(),
        ],
        &[],
    );
    assert_eq!(o.status.code(), Some(0), "{}{}", stdout(&o), stderr(&o));
    // Every value now comes from the profile itself rather than the base.
    assert!(
        stdout(&o).contains("(source: topology profile \"wide\")"),
        "{}",
        stdout(&o)
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn init_refuses_to_shadow_a_builtin_or_overwrite_without_force() {
    let dir = temp_dir("init-refusals");
    let o = cflux(
        &[
            "init",
            "--name",
            "cross_device",
            "--dir",
            dir.to_str().unwrap(),
        ],
        &[],
    );
    assert_eq!(o.status.code(), Some(1), "{}", stdout(&o));
    assert!(stdout(&o).contains("builtin"), "{}", stdout(&o));

    let args = ["init", "--name", "twice", "--dir", dir.to_str().unwrap()];
    assert_eq!(cflux(&args, &[]).status.code(), Some(0));
    let again = cflux(&args, &[]);
    assert_eq!(again.status.code(), Some(1), "{}", stdout(&again));
    assert!(stdout(&again).contains("--force"), "{}", stdout(&again));
    let forced = cflux(
        &[
            "init",
            "--name",
            "twice",
            "--dir",
            dir.to_str().unwrap(),
            "--force",
        ],
        &[],
    );
    assert_eq!(forced.status.code(), Some(0), "{}", stderr(&forced));

    // --docker adds the compose file beside the profiles.
    let o = cflux(
        &[
            "init",
            "--name",
            "withdocker",
            "--dir",
            dir.to_str().unwrap(),
            "--docker",
        ],
        &[],
    );
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    let compose = std::fs::read_to_string(dir.join("docker-compose.yml")).expect("compose file");
    assert!(
        compose.contains("16379:6379") && compose.contains("15432:5432"),
        "{compose}"
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn doctor_reports_every_check_at_once_and_passes_a_research_default() {
    let o = cflux(&["doctor"], &[]);
    assert_eq!(o.status.code(), Some(0), "{}{}", stdout(&o), stderr(&o));
    let out = stdout(&o);
    for check in [
        "configuration",
        "backends",
        "redis",
        "store",
        "tls",
        "jwt key",
        "sidecar",
    ] {
        assert!(out.contains(check), "missing {check} in:\n{out}");
    }
    assert!(out.contains("nothing was started"), "{out}");
}

#[test]
fn doctor_fails_production_on_in_memory_backends_and_names_each_one() {
    let o = cflux(&["doctor", "--mode", "production"], &[]);
    assert_eq!(o.status.code(), Some(1), "{}", stdout(&o));
    assert!(stdout(&o).contains("durable registry"), "{}", stdout(&o));

    let o = cflux(&["--format", "json", "doctor", "--mode", "production"], &[]);
    assert_eq!(o.status.code(), Some(1));
    let json: serde_json::Value = serde_json::from_slice(&o.stdout).expect("json");
    assert_eq!(json["ok"], false);
    let checks = json["checks"].as_array().expect("checks");
    assert!(
        checks
            .iter()
            .any(|c| c["name"] == "backends" && c["status"] == "fail"),
        "{json}"
    );

    // The same run as CI annotations rather than only a log.
    let o = cflux(
        &[
            "--format",
            "github-actions",
            "doctor",
            "--mode",
            "production",
        ],
        &[],
    );
    assert!(stdout(&o).contains("::error::backends:"), "{}", stdout(&o));
}

#[test]
fn doctor_fails_when_a_configured_backend_is_unreachable_and_when_a_sidecar_is_missing() {
    // Port 1 is reserved and nothing listens there, so the probe fails
    // for the right reason rather than by DNS accident.
    let o = cflux(
        &["doctor"],
        &[
            ("CONFLUX_REGISTRY_BACKEND", "redis"),
            ("CONFLUX_REDIS_URL", "redis://127.0.0.1:1"),
        ],
    );
    assert_eq!(o.status.code(), Some(1), "{}", stdout(&o));
    assert!(
        stdout(&o).contains("nothing is listening on 127.0.0.1:1"),
        "{}",
        stdout(&o)
    );

    // An aggregator defined in terms of a sidecar, with no sidecar.
    let o = cflux(&["doctor"], &[("CONFLUX_AGGREGATOR", "fltrust")]);
    assert_eq!(o.status.code(), Some(1), "{}", stdout(&o));
    assert!(
        stdout(&o).contains("CONFLUX_TRUSTED_REFERENCE_ADDR"),
        "{}",
        stdout(&o)
    );

    // A sidecar address set for a method that never calls one is legal
    // and pointless — a warning, not a failure.
    let o = cflux(
        &["doctor"],
        &[("CONFLUX_TRUSTED_REFERENCE_ADDR", "http://127.0.0.1:50100")],
    );
    assert_eq!(o.status.code(), Some(0), "{}", stdout(&o));
    assert!(
        stdout(&o).contains("never calls a sidecar"),
        "{}",
        stdout(&o)
    );
}

#[test]
fn a_backend_named_without_its_connection_string_cannot_run() {
    let o = cflux(&["doctor"], &[("CONFLUX_STORE_BACKEND", "postgres")]);
    assert_eq!(o.status.code(), Some(2), "{}", stdout(&o));
    assert!(
        stderr(&o).contains("CONFLUX_POSTGRES_URL"),
        "{}",
        stderr(&o)
    );
}

#[test]
fn every_help_ends_with_a_guide_link_and_version_names_both_versions() {
    for args in [
        vec!["--help"],
        vec!["catalog", "--help"],
        vec!["config", "--help"],
    ] {
        let o = cflux(&args, &[]);
        assert!(
            stdout(&o).contains("https://confluxfl.dev/guides/cflux/"),
            "{:?}",
            args
        );
    }
    let o = cflux(&["version"], &[]);
    assert!(
        stdout(&o).starts_with("cflux ") && stdout(&o).contains("framework crates"),
        "{}",
        stdout(&o)
    );
}
