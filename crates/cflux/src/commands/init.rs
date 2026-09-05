//! `cflux init` — scaffold a deployment.
//!
//! A deployment's identity is its *profile*, not a pile of environment
//! variables: a topology profile and a mode profile, each extending a
//! builtin and overriding only what differs. This command writes both,
//! with every key that axis owns present but commented at its inherited
//! value — so nothing a deployment can tune is invisible, and nothing is
//! silently changed by scaffolding it.

use std::path::PathBuf;

use clap::Args as ClapArgs;
use conflux_config::{Mode, ModeProfile, Topology, TopologyProfile};
use serde_json::json;

use crate::format::Report;
use crate::{CliError, EXIT_NEGATIVE, guide};

#[derive(ClapArgs)]
#[command(after_help = guide("init"))]
pub struct Args {
    /// The builtin topology to extend.
    #[arg(long, default_value = "cross_silo")]
    topology: String,
    /// The builtin mode to extend.
    #[arg(long, default_value = "production")]
    mode: String,
    /// Names the generated profiles: `<name>.toml` and `<name>_mode.toml`.
    #[arg(long, default_value = "my_deployment")]
    name: String,
    /// Where to write. `profiles/` is created under it.
    #[arg(long, default_value = ".")]
    dir: PathBuf,
    /// Also write a compose file for this topology's durable backends.
    #[arg(long)]
    docker: bool,
    /// Overwrite files that already exist.
    #[arg(long)]
    force: bool,
}

/// One file the command would write.
struct Planned {
    path: PathBuf,
    contents: String,
}

pub fn run(args: Args) -> Result<Report, CliError> {
    // A profile may not shadow a builtin: `cross_device.toml` quietly
    // replacing the builtin would make one deployment's `cross_device`
    // mean something nobody else's does. The loader refuses it too; this
    // refuses it earlier, when the name is still easy to change.
    let builtins: Vec<&str> = Topology::ALL
        .iter()
        .map(|t| t.label())
        .chain(Mode::ALL.iter().map(|m| m.label()))
        .collect();
    if builtins.contains(&args.name.as_str()) {
        return Ok(Report::plain(
            format!(
                "{:?} is a builtin profile name; choose another (a profile may not shadow a builtin)\n",
                args.name
            ),
            json!({ "ok": false, "error": "name shadows a builtin", "builtins": builtins }),
            EXIT_NEGATIVE,
        ));
    }

    let Some(topology) = Topology::ALL.iter().find(|t| t.label() == args.topology) else {
        return Ok(unknown(
            "topology",
            &args.topology,
            &Topology::ALL.iter().map(|t| t.label()).collect::<Vec<_>>(),
        ));
    };
    let Some(mode) = Mode::ALL.iter().find(|m| m.label() == args.mode) else {
        return Ok(unknown(
            "mode",
            &args.mode,
            &Mode::ALL.iter().map(|m| m.label()).collect::<Vec<_>>(),
        ));
    };

    let profiles = args.dir.join("profiles");
    let mut planned = vec![
        Planned {
            path: profiles.join(format!("{}.toml", args.name)),
            contents: topology_profile(*topology, &args.name),
        },
        Planned {
            path: profiles.join(format!("{}_mode.toml", args.name)),
            contents: mode_profile(*mode, &args.name),
        },
    ];
    if args.docker {
        planned.push(Planned {
            path: args.dir.join("docker-compose.yml"),
            contents: compose_file(),
        });
    }

    // Refuse the whole batch rather than half-writing it: a scaffold that
    // stopped partway would leave a deployment described by one new file
    // and one old one.
    let existing: Vec<String> = planned
        .iter()
        .filter(|p| p.path.exists())
        .map(|p| p.path.display().to_string())
        .collect();
    if !existing.is_empty() && !args.force {
        return Ok(Report::plain(
            format!(
                "refusing to overwrite:\n{}\nre-run with --force to replace them\n",
                existing
                    .iter()
                    .map(|p| format!("  {p}"))
                    .collect::<Vec<_>>()
                    .join("\n")
            ),
            json!({ "ok": false, "error": "files exist", "files": existing }),
            EXIT_NEGATIVE,
        ));
    }

    std::fs::create_dir_all(&profiles).map_err(|source| CliError::Write {
        path: profiles.display().to_string(),
        source,
    })?;
    for p in &planned {
        std::fs::write(&p.path, &p.contents).map_err(|source| CliError::Write {
            path: p.path.display().to_string(),
            source,
        })?;
    }

    let written: Vec<String> = planned
        .iter()
        .map(|p| p.path.display().to_string())
        .collect();
    let mut text = String::from("wrote:\n");
    for w in &written {
        text.push_str(&format!("  {w}\n"));
    }
    text.push_str(&format!(
        "\nEdit the profiles, then check them before starting anything:\n\
         \x20 CONFLUX_TOPOLOGY={name} CONFLUX_MODE={name}_mode cflux config check\n",
        name = args.name
    ));
    if args.docker {
        text.push_str("\nThe compose file brings up Redis, Postgres and MinIO on non-standard\nports so they cannot collide with services already on this machine:\n  docker compose up -d\n");
    }
    Ok(Report::plain(
        text,
        json!({ "ok": true, "files": written, "topology": args.topology, "mode": args.mode }),
        0,
    ))
}

fn unknown(axis: &str, name: &str, known: &[&str]) -> Report {
    Report::plain(
        format!(
            "no builtin {axis} named {name:?} (known: {})\n",
            known.join(", ")
        ),
        json!({ "ok": false, "axis": axis, "name": name, "known": known }),
        EXIT_NEGATIVE,
    )
}

/// A commented line: the key at its inherited value, ready to uncomment.
fn key(name: &str, value: impl std::fmt::Display) -> String {
    format!("# {name} = {value}\n")
}

fn topology_profile(topology: Topology, name: &str) -> String {
    let d = TopologyProfile::builtin(topology).defaults;
    format!(
        "# Topology profile {name:?} — what kind of participants and network.\n\
         # Select it with CONFLUX_TOPOLOGY={name} (or --topology {name}).\n\
         #\n\
         # Every key below is commented at the value it inherits from\n\
         # {base:?}. Uncomment only what this deployment changes; anything\n\
         # left commented keeps following the base, and its startup log line\n\
         # says so.\n\
         inherits = \"{base}\"\n\
         \n{connection_mode}{auth}{round_timeout_secs}{min_reputation_score}{client_registry_ttl}",
        base = topology.label(),
        connection_mode = key(
            "connection_mode",
            format!("{:?}", d.connection_mode.as_str())
        ),
        auth = key("auth", format!("{:?}", d.auth.as_str())),
        round_timeout_secs = key("round_timeout_secs", d.round_timeout_secs),
        // `{:?}` rather than `{}` so a whole number keeps its decimal
        // point. The loader accepts `0` either way — serde widens an
        // integer to a float — but a scaffold is also documentation, and
        // `0.0` shows the reader which kind of number this key holds.
        min_reputation_score = key(
            "min_reputation_score",
            format!("{:?}", d.min_reputation_score)
        ),
        client_registry_ttl = key("client_registry_ttl", d.client_registry_ttl),
    )
}

fn mode_profile(mode: Mode, name: &str) -> String {
    let d = ModeProfile::builtin(mode).defaults;
    let seed_value = match d.seed_value {
        Some(v) => key("seed_value", v),
        // No inherited value to show, so show the shape instead of
        // nothing: a key that appears only when set is a key nobody
        // discovers.
        None => "# seed_value = 42  # only meaningful with seed_mode = \"fixed\"\n".to_string(),
    };
    format!(
        "# Mode profile {name:?} — iterating, or running a live deployment.\n\
         # Select it with CONFLUX_MODE={name}_mode (or --mode {name}_mode).\n\
         #\n\
         # Every key below is commented at the value it inherits from\n\
         # {base:?}. The two axes own disjoint parameter sets, so a topology\n\
         # key here is refused by name rather than ignored.\n\
         inherits = \"{base}\"\n\
         \n{seed_mode}{seed_value}{budget}{scope}{stub}{node_auth}{log_format}",
        base = mode.label(),
        seed_mode = key("seed_mode", format!("{:?}", d.seed_mode.as_str())),
        budget = key(
            "budget_exhausted_action",
            format!("{:?}", d.budget_exhausted_action.as_str())
        ),
        scope = key(
            "accounting_scope",
            format!("{:?}", d.accounting_scope.as_str())
        ),
        stub = key("allow_stub_client", d.allow_stub_client),
        node_auth = key("require_node_auth", d.require_node_auth),
        log_format = key(
            "config_log_format",
            format!("{:?}", d.config_log_format.as_str())
        ),
    )
}

/// The durable backends a real deployment needs, on the same
/// non-standard ports the project's own compose file uses so a developer
/// with either one running is unaffected.
fn compose_file() -> String {
    String::from(
        r#"# Durable backends for a Conflux FL deployment, generated by `cflux init --docker`.
#
# Ports are deliberately non-standard so they cannot collide with a Redis
# or Postgres already running on this machine for something else.
#
#   docker compose up -d
#   CONFLUX_REGISTRY_BACKEND=redis CONFLUX_REDIS_URL=redis://localhost:16379 \
#   CONFLUX_STORE_BACKEND=postgres CONFLUX_POSTGRES_URL=postgres://conflux:conflux@localhost:15432/conflux \
#   CONFLUX_ACCOUNTING_PERSISTENCE=true cflux config check
services:
  redis:
    image: redis:7-alpine
    ports: ["16379:6379"]
    healthcheck:
      test: ["CMD", "redis-cli", "ping"]
      interval: 5s
  postgres:
    image: postgres:16-alpine
    environment:
      POSTGRES_USER: conflux
      # A local development password. A real deployment supplies its own
      # out of band; nothing here belongs in version control.
      POSTGRES_PASSWORD: conflux
      POSTGRES_DB: conflux
    ports: ["15432:5432"]
    healthcheck:
      test: ["CMD-SHELL", "pg_isready -U conflux"]
      interval: 5s
  minio:
    image: minio/minio
    command: server /data --console-address ":9001"
    environment:
      MINIO_ROOT_USER: minioadmin
      MINIO_ROOT_PASSWORD: minioadmin
    ports: ["19000:9000", "19001:9001"]
"#,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_topology_profile_inherits_and_comments_every_key_it_owns() {
        let text = topology_profile(Topology::CrossSilo, "hospital");
        assert!(text.contains("inherits = \"cross_silo\""));
        for k in [
            "connection_mode",
            "auth",
            "round_timeout_secs",
            "min_reputation_score",
            "client_registry_ttl",
        ] {
            assert!(
                text.contains(&format!("# {k} = ")),
                "missing {k} in:\n{text}"
            );
        }
        // Nothing is active except `inherits`, so scaffolding changes no
        // behavior until someone uncomments a line.
        let active: Vec<&str> = text
            .lines()
            .filter(|l| !l.trim_start().starts_with('#') && !l.trim().is_empty())
            .collect();
        assert_eq!(active, vec!["inherits = \"cross_silo\""], "{text}");
    }

    #[test]
    fn a_float_key_is_written_as_a_float() {
        // Not a correctness requirement — the loader widens an integer
        // to a float — but the scaffold doubles as documentation of each
        // key's type, and `0` would misreport this one.
        let text = topology_profile(Topology::CrossSilo, "hospital");
        assert!(text.contains("# min_reputation_score = 0.0"), "{text}");
    }

    #[test]
    fn a_mode_profile_shows_seed_value_even_though_it_has_none() {
        let text = mode_profile(Mode::Production, "strict");
        assert!(text.contains("inherits = \"production\""));
        assert!(text.contains("# seed_value ="), "{text}");
        assert!(text.contains("# require_node_auth = true"), "{text}");
    }
}
