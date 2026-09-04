//! The committed "Reproduced papers" table must match the manifests.
//!
//! `baselines/README.md` carries a fenced region that `conflux-baselines
//! table --write` generates from every `baselines/*/baseline.toml`. This
//! test asks the built runner to check that region, so a manifest edited
//! without regenerating, or a hand-edited row, fails CI — the same
//! guarantee the aggregation catalog's golden file gives.

use std::process::Command;

#[test]
fn the_readme_table_matches_a_fresh_generation() {
    let output = Command::new(env!("CARGO_BIN_EXE_conflux-baselines"))
        .args(["table", "--check"])
        .output()
        .expect("run the baselines runner");
    assert!(
        output.status.success(),
        "baselines/README.md is stale — regenerate with \
         `cargo run -p conflux-baselines -- table --write`\n{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
