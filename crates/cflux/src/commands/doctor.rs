//! `cflux doctor` — everything the server fail-fasts on, checked up
//! front and reported as one list.
//!
//! The value is not that these checks exist; the server already makes
//! every one of them at startup. The value is that they are made *early*
//! and *together*: today a deployer learns about a missing certificate,
//! then a wrong backend, then an incapable sidecar, one restart at a
//! time. This runs the server's own functions — `validate_production_backends`,
//! `resolve_server_tls`, `validate_jwt_startup`, and the same `Describe`
//! handshake — against the same environment, and prints every answer at
//! once.
//!
//! Suggestions are printed, never applied. A `--fix` here would operate
//! on a deployment rather than a local file, and the findings that look
//! most fixable — a missing key, an unreachable database — are exactly
//! the ones where doing something silently is worst.

use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

use clap::Args as ClapArgs;
use conflux_config::{AuthMode, Severity};
use conflux_server::{
    AccountingBackend, BackendSelection, RegistryBackend, StoreBackend, backend_selection_from_env,
    jwt_key_from_env, resolve_server_tls, tls_material_from_env, tls_paths_present,
    trusted_reference_addr, validate_jwt_startup, validate_production_backends,
};
use serde_json::json;

use crate::commands::config::{Selection, base_json, resolve};
use crate::format::{Annotation, Report};
use crate::{CliError, EXIT_NEGATIVE, guide};

/// How long to wait for a backend to accept a connection. Long enough
/// for a container still starting, short enough that a wrong host does
/// not stall the whole report.
const PROBE_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(ClapArgs)]
#[command(after_help = guide("doctor"))]
pub struct Args {
    #[command(flatten)]
    selection: Selection,
}

/// What one check concluded.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Status {
    /// Checked, and fine.
    Pass,
    /// Checked, and the server would refuse to start.
    Fail,
    /// Checked, legal, and probably not what anyone meant.
    Warn,
    /// Not applicable to this configuration.
    Skip,
}

impl Status {
    fn glyph(self) -> &'static str {
        match self {
            Status::Pass => "✓",
            Status::Fail => "✗",
            Status::Warn => "!",
            Status::Skip => "–",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Status::Pass => "pass",
            Status::Fail => "fail",
            Status::Warn => "warn",
            Status::Skip => "skip",
        }
    }
}

struct Check {
    name: &'static str,
    status: Status,
    detail: String,
}

impl Check {
    fn new(name: &'static str, status: Status, detail: impl Into<String>) -> Self {
        Self {
            name,
            status,
            detail: detail.into(),
        }
    }
}

/// Splits a connection string into the host and port to probe.
///
/// Deliberately not a URL parser: it only has to find the authority of
/// the shapes these variables actually hold — `redis://host:port`,
/// `postgres://user:pass@host:port/db`, `http://host:port`, and a bare
/// `host:port`. Anything it cannot split is reported as unparseable
/// rather than guessed at.
fn host_port(url: &str, default_port: u16) -> Option<(String, u16)> {
    let authority = url.split_once("://").map_or(url, |(_, rest)| rest);
    let authority = authority.split(['/', '?']).next()?;
    let authority = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    if authority.is_empty() {
        return None;
    }
    // An IPv6 literal is bracketed, so its colons are not port separators.
    if let Some(rest) = authority.strip_prefix('[') {
        let (host, tail) = rest.split_once(']')?;
        let port = match tail.strip_prefix(':') {
            Some(p) => p.parse().ok()?,
            None => default_port,
        };
        return Some((host.to_string(), port));
    }
    match authority.rsplit_once(':') {
        Some((host, port)) => Some((host.to_string(), port.parse().ok()?)),
        None => Some((authority.to_string(), default_port)),
    }
}

/// Whether something accepts a TCP connection there.
///
/// A connection is all this proves. Credentials, permissions, and schema
/// are not checked, and the report says so — a probe that authenticated
/// would need this command to hold a deployment's secrets, and one that
/// wrote anything would make a diagnostic tool a source of side effects.
fn probe(name: &'static str, url: &str, default_port: u16) -> Check {
    let Some((host, port)) = host_port(url, default_port) else {
        return Check::new(
            name,
            Status::Fail,
            format!("cannot read a host and port out of {url:?}"),
        );
    };
    let addrs = match (host.as_str(), port).to_socket_addrs() {
        Ok(a) => a.collect::<Vec<_>>(),
        Err(e) => {
            return Check::new(
                name,
                Status::Fail,
                format!("{host}:{port} does not resolve: {e}"),
            );
        }
    };
    for addr in &addrs {
        if TcpStream::connect_timeout(addr, PROBE_TIMEOUT).is_ok() {
            return Check::new(
                name,
                Status::Pass,
                format!("{host}:{port} accepts connections (credentials not checked)"),
            );
        }
    }
    Check::new(
        name,
        Status::Fail,
        format!("nothing is listening on {host}:{port} within {PROBE_TIMEOUT:?}"),
    )
}

/// The sidecar handshake, when the configured aggregator needs one.
///
/// `Describe` is read-only, so asking costs the sidecar nothing — and it
/// is the only check that can tell a reachable sidecar from a *capable*
/// one, which is the failure the server otherwise finds at startup and a
/// deployment otherwise finds in round one.
fn sidecar_check(needs_reference: bool, needs_scores: bool) -> Check {
    if !needs_reference && !needs_scores {
        let stray = trusted_reference_addr().is_some();
        return Check::new(
            "sidecar",
            if stray { Status::Warn } else { Status::Skip },
            if stray {
                "CONFLUX_TRUSTED_REFERENCE_ADDR is set, but this aggregator never calls a sidecar"
            } else {
                "this aggregator scores from the batch alone"
            },
        );
    }
    let Some(addr) = trusted_reference_addr() else {
        return Check::new(
            "sidecar",
            Status::Fail,
            "this aggregator needs a trusted-reference sidecar, but CONFLUX_TRUSTED_REFERENCE_ADDR \
             is unset — start one with `cargo run -p conflux-trusted-reference`",
        );
    };
    // One question, one runtime: a current-thread runtime built here and
    // dropped after, rather than making the whole CLI async for a single
    // RPC.
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(r) => r,
        Err(e) => return Check::new("sidecar", Status::Fail, format!("no async runtime: {e}")),
    };
    runtime.block_on(async {
        let mut transport = match conflux_net::TrustedReferenceTransport::connect(addr.clone())
            .await
        {
            Ok(t) => t,
            Err(e) => {
                return Check::new("sidecar", Status::Fail, format!("cannot reach {addr}: {e}"));
            }
        };
        let caps = match transport.describe().await {
            Ok(c) => c,
            Err(e) => {
                return Check::new(
                    "sidecar",
                    Status::Fail,
                    format!("{addr} did not answer Describe: {e}"),
                );
            }
        };
        let mut missing = Vec::new();
        if needs_reference && !caps.supports_reference_update {
            missing.push("reference updates");
        }
        if needs_scores && !caps.supports_scoring {
            missing.push("scoring");
        }
        if missing.is_empty() {
            Check::new(
                "sidecar",
                Status::Pass,
                format!(
                    "{addr}: {} (serves what this aggregator needs)",
                    caps.description
                ),
            )
        } else {
            Check::new(
                "sidecar",
                Status::Fail,
                format!(
                    "{addr} cannot serve {}: {}",
                    missing.join(" or "),
                    caps.description
                ),
            )
        }
    })
}

pub fn run(args: Args) -> Result<Report, CliError> {
    let r = resolve(&args.selection)?;
    let config = &r.config;
    let mode = config.mode;
    let mut checks = Vec::new();

    // 1. The configuration itself.
    let validation = config.validate();
    checks.push(Check::new(
        "configuration",
        if !validation.errors.is_empty() {
            Status::Fail
        } else if !validation.warnings.is_empty() {
            Status::Warn
        } else {
            Status::Pass
        },
        if validation.errors.is_empty() && validation.warnings.is_empty() {
            format!(
                "{} parameters resolved, nothing to report",
                base_json(&r)["parameters"].as_array().map_or(0, Vec::len)
            )
        } else {
            format!(
                "{} error(s), {} warning(s) — `cflux config check` lists them",
                validation.errors.len(),
                validation.warnings.len()
            )
        },
    ));

    // 2. Backends: what was selected, and whether this mode permits it.
    let backends = backend_selection_from_env().map_err(CliError::ServerEnv)?;
    let selected = describe_selection(&backends);
    checks.push(match validate_production_backends(mode, &backends) {
        Ok(()) => Check::new(
            "backends",
            Status::Pass,
            format!("{selected} — permitted in {}", mode.label()),
        ),
        Err(e) => Check::new("backends", Status::Fail, e.to_string()),
    });

    // 3. Can each configured backend be reached at all?
    checks.push(match &backends.registry {
        RegistryBackend::Memory => Check::new("redis", Status::Skip, "registry is in-memory"),
        RegistryBackend::Redis { url } => probe("redis", url, 6379),
    });
    checks.push(match &backends.store {
        StoreBackend::Memory => Check::new("store", Status::Skip, "store is in-memory"),
        StoreBackend::Postgres { url } => probe("postgres", url, 5432),
        StoreBackend::S3 { endpoint, .. } => probe("s3", endpoint, 443),
    });
    checks.push(match &backends.accounting {
        AccountingBackend::Disabled => Check::new(
            "accounting",
            Status::Skip,
            "epsilon is not persisted; a restart re-grants a spent budget",
        ),
        AccountingBackend::Postgres { url } => probe("accounting", url, 5432),
    });

    // 4. TLS material, and the posture it produces for this auth setting.
    let present = tls_paths_present();
    let set = present.iter().filter(|(_, ok)| *ok).count();
    if set > 0 && set < present.len() {
        // `tls_material_from_env` reports a partial set as `None`, exactly
        // like "nothing configured" — the one case where a pre-flight can
        // say more than the server's own reader does.
        let missing: Vec<&str> = present
            .iter()
            .filter(|(_, ok)| !ok)
            .map(|(v, _)| *v)
            .collect();
        checks.push(Check::new(
            "tls material",
            Status::Warn,
            format!(
                "{set} of {} paths set; no TLS will be bound until {} is too",
                present.len(),
                missing.join(" and ")
            ),
        ));
    }
    let material = tls_material_from_env().map_err(CliError::ServerEnv)?;
    let had_material = material.is_some();
    checks.push(match resolve_server_tls(mode, config.auth.value, material) {
        Ok(Some(_)) => Check::new("tls", Status::Pass, "mutual TLS will be bound from the configured material"),
        Ok(None) if config.auth.value == AuthMode::Mtls => Check::new(
            "tls",
            Status::Warn,
            "auth = mtls with no material — research mode binds plaintext; production would refuse",
        ),
        Ok(None) => Check::new(
            "tls",
            Status::Skip,
            format!("auth = {} does not bind TLS{}", config.auth.value.as_str(), if had_material { " (material is set and unused)" } else { "" }),
        ),
        Err(e) => Check::new("tls", Status::Fail, e.to_string()),
    });

    // 5. The JWT key, when tokens are what proves identity.
    let jwt_key = jwt_key_from_env().map_err(CliError::ServerEnv)?;
    checks.push(
        match validate_jwt_startup(mode, config.auth.value, jwt_key.as_ref()) {
            Ok(()) if config.auth.value != AuthMode::Jwt => Check::new(
                "jwt key",
                Status::Skip,
                format!(
                    "auth = {} does not verify tokens",
                    config.auth.value.as_str()
                ),
            ),
            Ok(()) if jwt_key.is_some() => {
                Check::new("jwt key", Status::Pass, "a public key is configured")
            }
            Ok(()) => Check::new(
                "jwt key",
                Status::Warn,
                "auth = jwt with no CONFLUX_JWT_PUBLIC_KEY_PATH — research mode verifies nothing",
            ),
            Err(e) => Check::new("jwt key", Status::Fail, e.to_string()),
        },
    );

    // 6. The sidecar, for the methods defined in terms of one.
    let (needs_reference, needs_scores) = sidecar_needs(&config.aggregator.value);
    checks.push(sidecar_check(needs_reference, needs_scores));

    Ok(report(&r, checks, &validation))
}

/// Whether the configured aggregator consumes a trusted reference, or
/// candidate scores, or neither — asked of the built aggregator rather
/// than a list of names kept here, which would drift the first time a
/// method joined the `trusted` family.
fn sidecar_needs(name: &str) -> (bool, bool) {
    match conflux_core::build_aggregator(name, conflux_core::AggregatorParams::default()) {
        Ok(agg) => (
            agg.requires_trusted_reference(),
            agg.requires_candidate_scores(),
        ),
        // An unbuildable name is already an error finding from
        // `config check`; nothing to add here.
        Err(_) => (false, false),
    }
}

fn describe_selection(b: &BackendSelection) -> String {
    let registry = match &b.registry {
        RegistryBackend::Memory => "memory",
        RegistryBackend::Redis { .. } => "redis",
    };
    let store = match &b.store {
        StoreBackend::Memory => "memory",
        StoreBackend::Postgres { .. } => "postgres",
        StoreBackend::S3 { .. } => "s3",
    };
    let accounting = match &b.accounting {
        AccountingBackend::Disabled => "disabled",
        AccountingBackend::Postgres { .. } => "postgres",
    };
    format!("registry={registry} store={store} accounting={accounting}")
}

fn report(
    r: &crate::commands::config::Resolution,
    checks: Vec<Check>,
    validation: &conflux_config::Validation,
) -> Report {
    let width = checks.iter().map(|c| c.name.len()).max().unwrap_or(0);
    let mut text = format!(
        "{} / {} — checking what the server checks at startup\n\n",
        r.config.topology.label(),
        r.config.mode.label()
    );
    for c in &checks {
        text.push_str(&format!(
            "{} {:<width$}  {}\n",
            c.status.glyph(),
            c.name,
            c.detail
        ));
    }
    let failed = checks.iter().filter(|c| c.status == Status::Fail).count();
    let warned = checks.iter().filter(|c| c.status == Status::Warn).count();
    let passed = checks.iter().filter(|c| c.status == Status::Pass).count();
    let skipped = checks.iter().filter(|c| c.status == Status::Skip).count();
    text.push_str(&format!(
        "\n{passed} passed, {failed} failed, {warned} warning(s), {skipped} skipped — nothing was started\n"
    ));

    // The findings a CI run should see on the diff, not only in the log.
    let mut annotations: Vec<Annotation> = checks
        .iter()
        .filter(|c| matches!(c.status, Status::Fail | Status::Warn))
        .map(|c| Annotation {
            level: if c.status == Status::Fail {
                "error"
            } else {
                "warning"
            },
            message: format!("{}: {}", c.name, c.detail),
        })
        .collect();
    // Config findings name their own parameter and source, which is more
    // useful on a pull request than the one-line summary above.
    for f in validation.errors.iter().chain(validation.warnings.iter()) {
        annotations.push(Annotation {
            level: if f.severity == Severity::Error {
                "error"
            } else {
                "warning"
            },
            message: f.to_string(),
        });
    }

    let mut json = base_json(r);
    json.insert(
        "checks".into(),
        json!(
            checks
                .iter()
                .map(|c| json!({ "name": c.name, "status": c.status.label(), "detail": c.detail }))
                .collect::<Vec<_>>()
        ),
    );
    json.insert("ok".into(), json!(failed == 0));
    Report {
        text,
        json: serde_json::Value::Object(json),
        exit_code: if failed == 0 { 0 } else { EXIT_NEGATIVE },
        annotations,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_connection_string_yields_the_host_and_port_to_probe() {
        assert_eq!(
            host_port("redis://localhost:16379", 6379),
            Some(("localhost".into(), 16379))
        );
        assert_eq!(
            host_port("postgres://user:pw@db.internal:15432/conflux", 5432),
            Some(("db.internal".into(), 15432))
        );
        assert_eq!(
            host_port("http://minio:9000", 443),
            Some(("minio".into(), 9000))
        );
        // No port: the backend's default applies.
        assert_eq!(
            host_port("redis://cache", 6379),
            Some(("cache".into(), 6379))
        );
        // An IPv6 literal's colons are not port separators.
        assert_eq!(
            host_port("redis://[::1]:6380", 6379),
            Some(("::1".into(), 6380))
        );
        assert_eq!(host_port("redis://[::1]", 6379), Some(("::1".into(), 6379)));
    }

    #[test]
    fn an_unusable_connection_string_is_refused_rather_than_guessed() {
        assert_eq!(host_port("redis://host:not-a-port", 6379), None);
        assert_eq!(host_port("", 6379), None);
    }

    #[test]
    fn a_batch_only_aggregator_needs_no_sidecar_and_fltrust_does() {
        assert_eq!(sidecar_needs("fedavg"), (false, false));
        assert!(
            sidecar_needs("fltrust").0,
            "fltrust consumes a trusted reference"
        );
        assert!(sidecar_needs("zeno").1, "zeno consumes candidate scores");
    }
}
