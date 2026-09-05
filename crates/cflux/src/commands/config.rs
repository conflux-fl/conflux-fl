//! `cflux config resolve` and `cflux config check`.
//!
//! Both do exactly what `conflux-server` does at startup, and then stop:
//! select the topology and mode profiles, read the experiment file and
//! the `CONFLUX_*` variables, resolve every parameter with its
//! provenance, and — for `check` — validate ranges and combinations.
//! Nothing listens, nothing connects. The flag values win over their
//! environment counterparts so a single command line can ask "what if".

use std::path::PathBuf;

use clap::{Args as ClapArgs, Subcommand};
use conflux_config::{
    LogFormat, Overrides, ResolvedConfig, Severity, Validation, mode_profile_named,
    overrides_from_env, resolve_with_profiles, topology_profile_named,
};
use serde_json::json;

use crate::format::Report;
use crate::{CliError, EXIT_NEGATIVE, guide};

#[derive(ClapArgs)]
#[command(after_help = guide("config"))]
pub struct Args {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Print every resolved parameter with the tier that set it.
    Resolve(Selection),
    /// `resolve`, then validate; exits 1 on any error-severity finding.
    Check(Selection),
}

/// What selects a configuration — the same inputs the server reads from
/// its environment, exposed as flags.
#[derive(ClapArgs)]
pub struct Selection {
    /// Topology: a builtin (cross_silo, cross_device, crowdsource, edge)
    /// or a profile file name under the profile directory. Defaults to
    /// $CONFLUX_TOPOLOGY, then cross_device.
    #[arg(long)]
    topology: Option<String>,
    /// Mode: research or production, or a profile file name. Defaults
    /// to $CONFLUX_MODE, then research.
    #[arg(long)]
    mode: Option<String>,
    /// Experiment-level overrides file (flat TOML). Defaults to
    /// $CONFLUX_EXPERIMENT_CONFIG_PATH.
    #[arg(long = "config", value_name = "FILE")]
    config_file: Option<PathBuf>,
    /// Directory holding custom profiles. Defaults to
    /// $CONFLUX_PROFILE_DIR, then `profiles`.
    #[arg(long, value_name = "DIR")]
    profile_dir: Option<PathBuf>,
}

/// A configuration plus the chains that produced it.
pub struct Resolution {
    pub config: ResolvedConfig,
    pub topology_chain: Vec<String>,
    pub mode_chain: Vec<String>,
}

pub fn resolve(sel: &Selection) -> Result<Resolution, CliError> {
    let profile_dir = sel
        .profile_dir
        .clone()
        .or_else(|| std::env::var_os("CONFLUX_PROFILE_DIR").map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("profiles"));
    let topology_name = sel
        .topology
        .clone()
        .or_else(|| std::env::var("CONFLUX_TOPOLOGY").ok());
    let mode_name = sel
        .mode
        .clone()
        .or_else(|| std::env::var("CONFLUX_MODE").ok());
    let topology = topology_profile_named(&profile_dir, topology_name.as_deref())?;
    let mode = mode_profile_named(&profile_dir, mode_name.as_deref())?;

    let config_path = sel
        .config_file
        .clone()
        .or_else(|| std::env::var_os("CONFLUX_EXPERIMENT_CONFIG_PATH").map(PathBuf::from));
    let file_overrides = match &config_path {
        Some(path) => Some(conflux_config::load_experiment_file(path)?),
        None => None,
    };
    let path_text = config_path
        .as_ref()
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    let file_tier = file_overrides.as_ref().map(|o| (path_text.as_str(), o));

    let env = overrides_from_env()?;
    let config = resolve_with_profiles(&topology, &mode, file_tier, &env, &Overrides::default())?;
    Ok(Resolution {
        config,
        topology_chain: topology.chain,
        mode_chain: mode.chain,
    })
}

/// The provenance lines as JSON objects — the server's own JSON log
/// format, parsed rather than re-invented, so a value's `source` reads
/// the same in a `cflux` report and in a server log.
fn parameters_json(config: &ResolvedConfig) -> Vec<serde_json::Value> {
    config
        .to_log_lines(LogFormat::Json)
        .iter()
        .map(|line| serde_json::from_str(line).unwrap_or_else(|_| json!({ "line": line })))
        .collect()
}

pub fn header(r: &Resolution) -> String {
    let mut s = format!(
        "topology: {} ({})\nmode:     {} ({})\n\n",
        r.config.topology.label(),
        r.topology_chain.join(" → "),
        r.config.mode.label(),
        r.mode_chain.join(" → ")
    );
    for line in r.config.to_log_lines(LogFormat::Text) {
        s.push_str(&line);
        s.push('\n');
    }
    s
}

pub fn base_json(r: &Resolution) -> serde_json::Map<String, serde_json::Value> {
    let mut m = serde_json::Map::new();
    m.insert("topology".into(), json!(r.config.topology.label()));
    m.insert("mode".into(), json!(r.config.mode.label()));
    m.insert(
        "chains".into(),
        json!({ "topology": r.topology_chain, "mode": r.mode_chain }),
    );
    m.insert("parameters".into(), json!(parameters_json(&r.config)));
    m
}

pub fn findings_json(v: &Validation) -> Vec<serde_json::Value> {
    v.errors
        .iter()
        .chain(v.warnings.iter())
        .map(|f| {
            json!({
                "severity": match f.severity { Severity::Error => "error", Severity::Warning => "warning" },
                "parameter": f.parameter,
                "value": f.value,
                "source": f.source,
                "message": f.message,
            })
        })
        .collect()
}

pub fn run(args: Args) -> Result<Report, CliError> {
    match args.command {
        Command::Resolve(sel) => {
            let r = resolve(&sel)?;
            Ok(Report {
                text: header(&r),
                json: serde_json::Value::Object(base_json(&r)),
                annotations: Vec::new(),
                exit_code: 0,
            })
        }
        Command::Check(sel) => {
            let r = resolve(&sel)?;
            let validation = r.config.validate();
            let mut text = header(&r);
            text.push('\n');
            for f in &validation.errors {
                text.push_str(&format!("error:   {f}\n"));
            }
            for f in &validation.warnings {
                text.push_str(&format!("warning: {f}\n"));
            }
            let ok = validation.errors.is_empty();
            text.push_str(&if ok {
                format!(
                    "✓ configuration is valid ({} warning(s)); nothing was started\n",
                    validation.warnings.len()
                )
            } else {
                format!(
                    "✗ configuration invalid: {} error(s), {} warning(s); nothing was started\n",
                    validation.errors.len(),
                    validation.warnings.len()
                )
            });
            let mut json = base_json(&r);
            json.insert("findings".into(), json!(findings_json(&validation)));
            json.insert("ok".into(), json!(ok));
            Ok(Report {
                text,
                json: serde_json::Value::Object(json),
                annotations: Vec::new(),
                exit_code: if ok { 0 } else { EXIT_NEGATIVE },
            })
        }
    }
}
