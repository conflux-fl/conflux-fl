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

/// A temp directory for one test's child logs, named so two `cargo test`
/// runs at once cannot share it.
fn temp_log_dir(tag: &str) -> std::path::PathBuf {
    use std::sync::atomic::{AtomicU32, Ordering};
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("cflux-fed-{tag}-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// The claim the two isolation tiers make about each other: same
/// federation, same pipeline, different amount of process isolation. If
/// they disagreed on what was learned, one of them would be lying about
/// being the real pipeline.
#[test]
fn the_two_tiers_agree_on_what_was_learned() {
    let dir = temp_log_dir("agree");
    let args = ["--clients", "3", "--rounds", "15", "--model", "logreg"];

    let task = cflux(&[&["--format", "json", "fed", "run"][..], &args[..]].concat());
    assert!(
        task.status.success(),
        "task tier: {}",
        String::from_utf8_lossy(&task.stderr)
    );
    let task = json(&task);

    let process = cflux(
        &[
            &["--format", "json", "fed", "run"][..],
            &args[..],
            &["--isolation", "process", "--log-dir", dir.to_str().unwrap()][..],
        ]
        .concat(),
    );
    assert!(
        process.status.success(),
        "process tier: {}",
        String::from_utf8_lossy(&process.stderr)
    );
    let process = json(&process);
    let _ = std::fs::remove_dir_all(&dir);

    assert_eq!(task["isolation"], "task");
    assert_eq!(process["isolation"], "process");
    // Neither is a simulation, and both say so in the machine-readable
    // report rather than only in prose.
    assert_eq!(task["simulated"], false);
    assert_eq!(process["simulated"], false);

    let a = task["score"]["value"].as_f64().expect("task score");
    let b = process["score"]["value"].as_f64().expect("process score");
    assert_eq!(
        task["score"]["label"], process["score"]["label"],
        "both tiers should report the same metric"
    );
    assert!(
        (a - b).abs() < 1e-6,
        "the tiers disagree about what was learned: task {a}, process {b}"
    );
    assert!(a > 0.9, "three clients should have learned something: {a}");
}

/// Process mode spawns the same commands an operator runs on real
/// machines, and prints them. That printout is the deployment recipe, so
/// it is part of the contract rather than decoration.
#[test]
fn process_mode_prints_what_it_ran() {
    let dir = temp_log_dir("recipe");
    let out = cflux(&[
        "--format",
        "json",
        "fed",
        "run",
        "--clients",
        "2",
        "--rounds",
        "2",
        "--model",
        "stub",
        "--isolation",
        "process",
        "--log-dir",
        dir.to_str().unwrap(),
    ]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v = json(&out);
    let recipe: Vec<String> = v["recipe"]
        .as_array()
        .expect("a recipe")
        .iter()
        .map(|l| l.as_str().unwrap().to_string())
        .collect();

    // One server, two nodes, two clients.
    assert_eq!(recipe.len(), 5, "{recipe:#?}");
    assert!(recipe[0].contains("server start"), "{}", recipe[0]);
    assert!(recipe[1].contains("node start"), "{}", recipe[1]);
    assert!(recipe[3].contains("fed client"), "{}", recipe[3]);

    // Every listener is on port 0, which is the whole point: nothing in
    // this federation picked a port.
    for line in recipe.iter().take(3) {
        assert!(
            line.contains("127.0.0.1:0"),
            "a listener should bind port 0 and report back: {line}"
        );
    }
    // And the clients were told a real address that the node reported.
    assert!(
        recipe[3].contains("--address http://127.0.0.1:") && !recipe[3].contains(":0 "),
        "the client should get the node's real address: {}",
        recipe[3]
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// A client that is a program cannot be a task, and saying so is better
/// than failing later with something about trait bounds.
#[test]
fn a_client_command_needs_its_own_process() {
    let manifest = temp_manifest(
        r#"
[client]
command = ["python3", "-m", "trainer"]
"#,
    );
    let out = cflux(&[
        "fed",
        "run",
        manifest.to_str().unwrap(),
        "--isolation",
        "task",
    ]);
    let _ = std::fs::remove_file(&manifest);
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("--isolation process"),
        "should name the tier that can run it: {stderr}"
    );
}

/// An arbitrary program is driven through the same contract the SDKs
/// implement — `--address`, `--client-id`, `--rounds` appended to
/// whatever the manifest names. Proven here with a command that is not
/// the built-in path, even though it happens to be the same binary.
#[test]
fn an_external_command_gets_the_address_and_identity_contract() {
    let dir = temp_log_dir("command");
    let exe = env!("CARGO_BIN_EXE_cflux");
    let manifest = temp_manifest(&format!(
        r#"
[federation]
clients = 2
rounds  = 3

[client]
command = ["{exe}", "fed", "client", "--model", "linreg"]
"#
    ));
    let out = cflux(&[
        "--format",
        "json",
        "fed",
        "run",
        manifest.to_str().unwrap(),
        "--isolation",
        "process",
        "--log-dir",
        dir.to_str().unwrap(),
    ]);
    let _ = std::fs::remove_file(&manifest);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v = json(&out);
    assert_eq!(v["clients"].as_array().unwrap().len(), 2);
    let recipe = v["recipe"].as_array().unwrap();
    let client_line = recipe.last().unwrap().as_str().unwrap();
    assert!(
        client_line.contains("--address")
            && client_line.contains("--client-id client-1")
            && client_line.contains("--rounds 3"),
        "the contract should be appended to the command: {client_line}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
