//! One module per command; `main.rs` only routes to them.

pub mod catalog;
pub mod config;
pub mod doctor;
pub mod init;

use crate::format::Report;

/// `cflux version`: this binary's version and the framework version it
/// embeds. They are the same number today because every crate in the
/// workspace shares one version, but they are printed separately on
/// purpose — a `cflux` binary pins the framework it links, so the pair
/// is the fact an operator needs when asking "which Conflux is this".
pub fn version() -> Report {
    let cflux = env!("CARGO_PKG_VERSION");
    let framework = conflux_config::FRAMEWORK_VERSION;
    Report::plain(
        format!("cflux {cflux} (framework crates {framework})\n"),
        serde_json::json!({ "cflux": cflux, "framework": framework }),
        0,
    )
}
