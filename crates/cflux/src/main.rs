//! `cflux` — the Conflux FL command line.
//!
//! The Rust toolchain is for a developer *of* the framework; `cflux` is
//! for a user of it. This binary answers, without starting a server,
//! the questions an operator has before a deployment exists: what
//! methods are there and what does each one need (`catalog`), and is
//! this configuration sound — every resolved value, the tier that set
//! it, and every validation finding (`config check`).
//!
//! Every command takes `--format pretty|json`; `json` is what CI and
//! scripts read, and the exit code is stable either way: `0` success,
//! `1` a negative answer (an error finding, an unknown name), `2` the
//! command could not run (bad flag, unreadable file, malformed
//! `CONFLUX_*` value).
//!
//! `src/main.rs` only dispatches; each command owns its arguments and its
//! `run` in `src/commands/<name>.rs`, and `format.rs` renders the result
//! the caller asked for.

mod commands;
mod format;

use clap::{Parser, Subcommand};

use crate::format::{Format, Report};

/// Where the long-form documentation for a command lives. Printed at the
/// end of every `--help`, so the terminal carries the signature and the
/// web carries the explanation.
pub(crate) fn guide(section: &str) -> String {
    format!("📖 Full guide: https://confluxfl.dev/guides/cflux/#{section}")
}

/// Exit code for "the command ran and its answer is no".
pub(crate) const EXIT_NEGATIVE: i32 = 1;
/// Exit code for "the command could not run".
pub(crate) const EXIT_FAILURE: i32 = 2;

/// Anything that stops a command from producing an answer.
#[derive(Debug, thiserror::Error)]
pub(crate) enum CliError {
    /// A profile name matched neither a builtin nor a file.
    #[error("{0}")]
    Profile(#[from] conflux_config::ProfileError),
    /// A `CONFLUX_*` variable is set but malformed.
    #[error("{0}")]
    Env(#[from] conflux_config::EnvError),
    /// The experiment file could not be read or parsed.
    #[error("{0}")]
    ConfigFile(#[from] conflux_config::ConfigFileError),
    /// Resolution itself failed.
    #[error("{0}")]
    Resolve(#[from] conflux_config::ConfigError),
    /// A deployment-material variable names something unusable — a
    /// backend selected without its connection string, an unreadable
    /// certificate, a key that is not a key.
    #[error("{0}")]
    ServerEnv(conflux_server::EnvError),
    /// A file the command was asked to write could not be written.
    #[error("could not write {path}: {source}")]
    Write {
        /// The file.
        path: String,
        /// Why the write failed.
        source: std::io::Error,
    },
}

#[derive(Parser)]
#[command(
    name = "cflux",
    version,
    about = "Conflux FL's command line: inspect the method catalog and pre-flight a configuration without starting anything.",
    after_help = guide("commands")
)]
struct Cli {
    /// Output format: human-readable, or JSON for scripts and CI.
    #[arg(long, global = true, value_enum, default_value_t = Format::Pretty)]
    format: Format,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// The strategy catalog: what methods exist and what each one needs.
    Catalog(commands::catalog::Args),
    /// Resolve and validate a configuration without starting anything.
    Config(commands::config::Args),
    /// Scaffold a deployment: a topology profile, a mode profile, and
    /// optionally a compose file for its durable backends.
    Init(commands::init::Args),
    /// Run every startup check at once — config, backends, TLS, JWT, and
    /// the sidecar — without starting anything.
    Doctor(commands::doctor::Args),
    /// Print this binary's version and the framework version it embeds.
    Version,
}

fn main() {
    // The registry is populated by `inventory::submit!` statics in the
    // family crates; nothing else here references them, so one real
    // reference each keeps the registrations from being dead-stripped.
    let _ = conflux_core::build_aggregator;
    let _ = conflux_selector::build_selector;
    let _ = conflux_privacy::build_privacy_mechanism;

    let cli = Cli::parse();
    let result: Result<Report, CliError> = match cli.command {
        Command::Catalog(args) => commands::catalog::run(args),
        Command::Config(args) => commands::config::run(args),
        Command::Init(args) => commands::init::run(args),
        Command::Doctor(args) => commands::doctor::run(args),
        Command::Version => Ok(commands::version()),
    };
    match result {
        Ok(report) => {
            format::print(&report, cli.format);
            std::process::exit(report.exit_code);
        }
        Err(err) => {
            match cli.format {
                Format::Pretty => eprintln!("error: {err}"),
                Format::Json | Format::GithubActions => println!(
                    "{}",
                    serde_json::json!({ "ok": false, "error": err.to_string() })
                ),
            }
            std::process::exit(EXIT_FAILURE);
        }
    }
}
