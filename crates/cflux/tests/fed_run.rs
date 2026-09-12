//! `cflux fed run` end to end: a real federation from a real manifest.
//!
//! Drives the built binary rather than the library, because what is being
//! checked here is the *command* — that a manifest reaches a
//! `FederationConfig`, that flags beat the file, that the JSON contract
//! holds, and that the run actually learns. A library-level test would
//! exercise none of the wiring that a user touches.

use std::process::{Command, Output};

fn cflux(args: &[&str]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_cflux"));
    cmd.args(args);
    // A clean CONFLUX_* environment, so a developer's shell cannot leak
    // into the assertions.
    for (key, _) in std::env::vars() {
        if key.starts_with("CONFLUX_") {
            cmd.env_remove(key);
        }
    }
    cmd.output().expect("run cflux")
}

/// Writes `contents` to a uniquely-named temp file and hands back the
/// path. The name mixes the process id with a per-call counter: a counter
/// alone still collides across two `cargo test` runs at once.
fn temp_manifest(contents: &str) -> std::path::PathBuf {
    use std::sync::atomic::{AtomicU32, Ordering};
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    let path = std::env::temp_dir().join(format!("cflux-fed-{}-{n}.toml", std::process::id()));
    std::fs::write(&path, contents).expect("write manifest");
    path
}

fn json(out: &Output) -> serde_json::Value {
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "stdout was not JSON ({e}): {}",
            String::from_utf8_lossy(&out.stdout)
        )
    })
}

#[test]
fn a_manifest_drives_a_federation_that_learns() {
    let manifest = temp_manifest(
        r#"
[federation]
clients   = 3
rounds    = 15
isolation = "task"

[method]
aggregator = "fedavg"

[client]
builtin = "linreg"
"#,
    );

    let out = cflux(&["--format", "json", "fed", "run", manifest.to_str().unwrap()]);
    let _ = std::fs::remove_file(&manifest);

    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v = json(&out);
    assert_eq!(v["model"], "linreg");
    assert_eq!(v["rounds"], 15);
    assert_eq!(v["clients"].as_array().unwrap().len(), 3);
    for client in v["clients"].as_array().unwrap() {
        assert_eq!(client["rounds_completed"], 15);
    }

    // Not a simulation, and the machine-readable report says so — a
    // number from here is a number about the real pipeline.
    assert_eq!(v["simulated"], false);
    assert_eq!(v["isolation"], "task");

    // And it learned. Every client is blind to a different coefficient,
    // so this number is only reachable by averaging them.
    let mse = v["score"]["value"].as_f64().expect("a score");
    assert_eq!(v["score"]["label"], "held-out MSE");
    assert!(
        mse < 0.05,
        "a federation of three clients should recover the model; held-out MSE was {mse}"
    );
}

#[test]
fn a_flag_beats_the_file() {
    let manifest = temp_manifest(
        r#"
[federation]
clients = 9
rounds  = 99

[client]
builtin = "stub"
"#,
    );

    let out = cflux(&[
        "--format",
        "json",
        "fed",
        "run",
        manifest.to_str().unwrap(),
        "--clients",
        "2",
        "--rounds",
        "2",
    ]);
    let _ = std::fs::remove_file(&manifest);

    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v = json(&out);
    assert_eq!(v["rounds"], 2, "the flag should win over the manifest");
    assert_eq!(v["clients"].as_array().unwrap().len(), 2);
    // `stub` has nothing to score, and reports that rather than a number
    // somebody could mistake for a result.
    assert!(v["score"].is_null(), "stub must not produce a score");
}

#[test]
fn the_demo_is_loud_about_what_it_is() {
    let manifest = temp_manifest("[client]\nbuiltin = \"stub\"\n");
    let out = cflux(&[
        "fed",
        "run",
        manifest.to_str().unwrap(),
        "--clients",
        "1",
        "--rounds",
        "1",
    ]);
    let _ = std::fs::remove_file(&manifest);

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("Local federation"),
        "the banner should name the tier: {stderr}"
    );
    assert!(
        stderr.contains("Not real:"),
        "the banner should name what is missing: {stderr}"
    );
    assert!(
        stderr.contains("does not learn"),
        "`stub` must say it does not learn, every time: {stderr}"
    );
    // The banner is on stderr so `--format json` stays parseable; the
    // previous tests would have failed if it were not.
}

#[test]
fn an_unbuilt_isolation_says_so_rather_than_failing_obscurely() {
    let out = cflux(&["fed", "run", "--isolation", "process"]);
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("not built yet") && stderr.contains("--isolation task"),
        "should name the state and the alternative: {stderr}"
    );
}

#[test]
fn an_unknown_model_lists_what_exists() {
    let out = cflux(&["fed", "run", "--model", "resnet50"]);
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("resnet50") && stderr.contains("linreg"),
        "should name the miss and what is available: {stderr}"
    );
}

#[test]
fn every_demo_model_is_listed_with_its_caveat() {
    let out = cflux(&["--format", "json", "fed", "models"]);
    assert!(out.status.success());
    let v = json(&out);
    let models = v["models"].as_array().expect("models");
    assert_eq!(models.len(), 3);
    for m in models {
        assert!(
            !m["caveat"].as_str().unwrap().is_empty(),
            "{} must carry a caveat",
            m["name"]
        );
    }
}
