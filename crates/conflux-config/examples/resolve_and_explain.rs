//! Runnable "try it" for the [crate-deep-dives article on
//! `conflux-config`](https://confluxfl.dev/crate-deep-dives/conflux-config/).
//!
//! Run with:
//!   cargo run --example resolve_and_explain -p conflux-config
//!
//! Resolves the same parameters under two different topology/mode/override
//! combinations and prints `to_log_lines()`'s output for each — this *is*
//! what `conflux-server` prints at startup, in both supported formats.

use conflux_config::{LogFormat, Mode, Overrides, Topology, resolve};

fn main() {
    println!("=== cross_device / research, no overrides (text format) ===");
    let resolved = resolve(
        Topology::CrossDevice,
        Mode::Research,
        None,
        &Overrides::default(),
        &Overrides::default(),
    )
    .expect("resolution cannot fail today — see ConfigError's doc comment");
    for line in resolved.to_log_lines(LogFormat::Text) {
        println!("{line}");
    }

    println!();
    println!("=== cross_silo / production, aggregator overridden via CLI (JSON format) ===");
    let cli_overrides = Overrides {
        aggregator: Some("krum".to_string()),
        robust_byzantine_fraction: Some(0.3),
        ..Default::default()
    };
    let resolved = resolve(
        Topology::CrossSilo,
        Mode::Production,
        None,
        &Overrides::default(),
        &cli_overrides,
    )
    .expect("resolution cannot fail today");
    for line in resolved.to_log_lines(LogFormat::Json) {
        println!("{line}");
    }

    println!();
    println!(
        "notice: aggregator's source is \"cli\" in the second block, and \
         require_node_auth flips true -> production's mode profile default \
         — no code change between the two runs, only the topology/mode/override inputs."
    );
}
