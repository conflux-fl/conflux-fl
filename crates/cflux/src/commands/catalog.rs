//! `cflux catalog list` and `cflux catalog describe <name>`.
//!
//! Registry-backed: the same `StrategyEntry` data the generated
//! aggregation catalog is built from, so the CLI and the docs cannot
//! disagree about what ships.

use clap::{Args as ClapArgs, Subcommand, ValueEnum};
use conflux_config::{StrategyEntry, StrategyKind, entries, lookup};
use serde_json::json;

use crate::format::Report;
use crate::{CliError, EXIT_NEGATIVE, guide};

#[derive(ClapArgs)]
#[command(after_help = guide("catalog"))]
pub struct Args {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Every registered method, grouped by kind.
    List {
        /// Which kind of strategy to list.
        #[arg(long, value_enum, default_value_t = Kind::All)]
        kind: Kind,
    },
    /// One method: family, paper, parameters read, and what it needs.
    Describe {
        /// The method's registry name, e.g. `krum`.
        name: String,
    },
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
enum Kind {
    Aggregator,
    Selector,
    Privacy,
    All,
}

impl Kind {
    fn strategy_kinds(self) -> Vec<StrategyKind> {
        match self {
            Kind::Aggregator => vec![StrategyKind::Aggregator],
            Kind::Selector => vec![StrategyKind::Selector],
            Kind::Privacy => vec![StrategyKind::PrivacyMechanism],
            Kind::All => vec![
                StrategyKind::Aggregator,
                StrategyKind::Selector,
                StrategyKind::PrivacyMechanism,
            ],
        }
    }
}

/// The word the CLI uses for a [`StrategyKind`].
fn kind_label(kind: StrategyKind) -> &'static str {
    match kind {
        StrategyKind::Aggregator => "aggregator",
        StrategyKind::Selector => "selector",
        StrategyKind::PrivacyMechanism => "privacy_mechanism",
    }
}

/// What a method needs beyond the server itself. The `trusted` family
/// is the only one whose algorithm requires server-side training or
/// scoring on data no client holds, which Conflux runs as the separate
/// `conflux-trusted-reference` sidecar process.
fn sidecar(entry: &StrategyEntry) -> Option<&'static str> {
    (entry.kind == StrategyKind::Aggregator && entry.family == "trusted")
        .then_some("trusted-reference")
}

fn entry_json(entry: &StrategyEntry) -> serde_json::Value {
    json!({
        "name": entry.name,
        "kind": kind_label(entry.kind),
        "family": entry.family,
        "citation": entry.citation,
        "params": entry.params,
        "sidecar": sidecar(entry),
    })
}

pub fn run(args: Args) -> Result<Report, CliError> {
    Ok(match args.command {
        Command::List { kind } => list(kind),
        Command::Describe { name } => describe(&name),
    })
}

fn list(kind: Kind) -> Report {
    let mut text = String::new();
    let mut groups = serde_json::Map::new();
    for k in kind.strategy_kinds() {
        let mut all = entries(k);
        all.sort_by_key(|e| (e.family, e.name));
        text.push_str(&format!("{} ({}):\n", kind_label(k), all.len()));
        let width = all.iter().map(|e| e.name.len()).max().unwrap_or(0);
        for e in &all {
            text.push_str(&format!(
                "  {:<width$}  {:<13} {}{}\n",
                e.name,
                e.family,
                e.citation,
                sidecar(e)
                    .map(|s| format!("  [needs {s} sidecar]"))
                    .unwrap_or_default(),
            ));
        }
        text.push('\n');
        groups.insert(
            format!("{}s", kind_label(k)),
            serde_json::Value::Array(all.iter().map(|e| entry_json(e)).collect()),
        );
    }
    Report {
        text,
        json: serde_json::Value::Object(groups),
        exit_code: 0,
    }
}

fn describe(name: &str) -> Report {
    let kinds = [
        StrategyKind::Aggregator,
        StrategyKind::Selector,
        StrategyKind::PrivacyMechanism,
    ];
    let Some(entry) = kinds.iter().find_map(|k| lookup(*k, name)) else {
        let known: Vec<&str> = kinds
            .iter()
            .flat_map(|k| entries(*k).into_iter().map(|e| e.name))
            .collect();
        return Report {
            text: format!(
                "no strategy named {name:?} in the catalog (known: {})\n",
                known.join(", ")
            ),
            json: json!({ "ok": false, "name": name, "known": known }),
            exit_code: EXIT_NEGATIVE,
        };
    };
    let params = if entry.params.is_empty() {
        "— (none beyond the shared ones)".to_string()
    } else {
        entry.params.join(", ")
    };
    let needs = match sidecar(entry) {
        Some(s) => format!("the {s} sidecar (a separate process the server calls over gRPC)"),
        None => "nothing beyond conflux-server".to_string(),
    };
    Report {
        text: format!(
            "{}\n  kind:      {}\n  family:    {}\n  paper:     {}\n  reads:     {}\n  needs:     {}\n",
            entry.name,
            kind_label(entry.kind),
            entry.family,
            entry.citation,
            params,
            needs
        ),
        json: entry_json(entry),
        exit_code: 0,
    }
}
