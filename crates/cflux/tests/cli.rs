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
