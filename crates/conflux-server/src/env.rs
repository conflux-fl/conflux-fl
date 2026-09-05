//! The deployment-material `CONFLUX_*` environment: which backends to
//! use, what TLS material to bind with, which JWT key to verify against,
//! and where the trusted-reference sidecar lives.
//!
//! These are *deployment* details rather than experiment parameters,
//! which is why they never became `conflux_config::Overrides` fields —
//! a connection string is not something a topology profile tunes. They
//! live in the library rather than the binary so anything that needs to
//! know what a deployment selected can ask once, in one place. `cflux
//! doctor` is the second caller: a pre-flight that read these variables
//! its own way would be checking a different deployment than the one
//! that starts.
//!
//! Every reader is fallible. The binary turns an error into a panic at
//! startup, which is the same fail-fast it always did; a library that
//! panicked would take that choice away from every other caller.

use conflux_net::jwt::JwtKeyMaterial;

use crate::auth_enforcement::TlsMaterial;
use crate::backend_selection::{
    AccountingBackend, BackendSelection, RegistryBackend, StoreBackend,
};

/// A `CONFLUX_*` variable that names something the process cannot use.
#[derive(Debug, thiserror::Error)]
pub enum EnvError {
    /// A selection was made without the variable it needs to be usable —
    /// `CONFLUX_STORE_BACKEND=postgres` with no connection string, say.
    #[error("{selection} requires {missing}")]
    MissingCompanion {
        /// The variable and value that made the selection.
        selection: String,
        /// The variable that must accompany it.
        missing: &'static str,
    },

    /// A path was named but could not be read.
    #[error("failed to read {var} ({path}): {source}")]
    Unreadable {
        /// The variable holding the path.
        var: &'static str,
        /// The path it held.
        path: String,
        /// Why the read failed.
        source: std::io::Error,
    },

    /// A file was read but is not usable as what it claims to be.
    #[error("{var} ({path}) is unusable: {message}")]
    Unusable {
        /// The variable holding the path.
        var: &'static str,
        /// The path it held.
        path: String,
        /// What was wrong with the contents.
        message: String,
    },
}

/// Reads a path variable's file, or `None` when the variable is unset.
fn read_path(
    var: &'static str,
    get: &impl Fn(&str) -> Option<String>,
) -> Result<Option<(String, Vec<u8>)>, EnvError> {
    let Some(path) = get(var) else {
        return Ok(None);
    };
    let bytes = std::fs::read(&path).map_err(|source| EnvError::Unreadable {
        var,
        path: path.clone(),
        source,
    })?;
    Ok(Some((path, bytes)))
}

/// Which registry, store, and accounting backends this process should
/// use. Unset means the in-memory/disabled default;
/// [`crate::validate_production_backends`] is what turns that into a
/// refusal when `mode = production`, so this reader has no opinion about
/// mode at all.
pub fn backend_selection_from_env() -> Result<BackendSelection, EnvError> {
    backend_selection_from_vars(|name| std::env::var(name).ok())
}

/// [`backend_selection_from_env`] over any lookup function, so every
/// branch is testable without touching the process environment.
pub fn backend_selection_from_vars(
    get: impl Fn(&str) -> Option<String>,
) -> Result<BackendSelection, EnvError> {
    fn require(
        get: &impl Fn(&str) -> Option<String>,
        var: &'static str,
        selection: &str,
    ) -> Result<String, EnvError> {
        get(var).ok_or_else(|| EnvError::MissingCompanion {
            selection: selection.to_string(),
            missing: var,
        })
    }

    let registry = match get("CONFLUX_REGISTRY_BACKEND").as_deref() {
        Some("redis") => RegistryBackend::Redis {
            url: require(&get, "CONFLUX_REDIS_URL", "CONFLUX_REGISTRY_BACKEND=redis")?,
        },
        _ => RegistryBackend::Memory,
    };

    let store = match get("CONFLUX_STORE_BACKEND").as_deref() {
        Some("postgres") => StoreBackend::Postgres {
            url: require(
                &get,
                "CONFLUX_POSTGRES_URL",
                "CONFLUX_STORE_BACKEND=postgres",
            )?,
        },
        Some("s3") => StoreBackend::S3 {
            endpoint: require(&get, "CONFLUX_S3_ENDPOINT", "CONFLUX_STORE_BACKEND=s3")?,
            bucket: require(&get, "CONFLUX_S3_BUCKET", "CONFLUX_STORE_BACKEND=s3")?,
            access_key: require(&get, "CONFLUX_S3_ACCESS_KEY", "CONFLUX_STORE_BACKEND=s3")?,
            secret_key: require(&get, "CONFLUX_S3_SECRET_KEY", "CONFLUX_STORE_BACKEND=s3")?,
        },
        _ => StoreBackend::Memory,
    };

    let accounting = match get("CONFLUX_ACCOUNTING_PERSISTENCE").as_deref() {
        Some("true") => AccountingBackend::Postgres {
            url: require(
                &get,
                "CONFLUX_POSTGRES_URL",
                "CONFLUX_ACCOUNTING_PERSISTENCE=true",
            )?,
        },
        _ => AccountingBackend::Disabled,
    };

    Ok(BackendSelection {
        registry,
        store,
        accounting,
    })
}

/// The server's own certificate, key, and the CA it verifies clients
/// against. `None` unless all three paths are set: a partial set is not
/// enough to bind mutual TLS, and [`crate::resolve_server_tls`] is what
/// decides whether the absence is fatal for this mode and `auth` value.
///
/// A path that *is* set but cannot be read is an error rather than a
/// silent `None` — an operator who named a certificate meant it.
pub fn tls_material_from_env() -> Result<Option<TlsMaterial>, EnvError> {
    tls_material_from_vars(|name| std::env::var(name).ok())
}

/// [`tls_material_from_env`] over any lookup function.
pub fn tls_material_from_vars(
    get: impl Fn(&str) -> Option<String>,
) -> Result<Option<TlsMaterial>, EnvError> {
    let cert = read_path("CONFLUX_TLS_CERT_PATH", &get)?;
    let key = read_path("CONFLUX_TLS_KEY_PATH", &get)?;
    let client_ca = read_path("CONFLUX_TLS_CLIENT_CA_PATH", &get)?;
    match (cert, key, client_ca) {
        (Some((_, cert_pem)), Some((_, key_pem)), Some((_, client_ca_pem))) => {
            Ok(Some(TlsMaterial {
                cert_pem,
                key_pem,
                client_ca_pem,
            }))
        }
        _ => Ok(None),
    }
}

/// Which of the three TLS path variables are set, in declaration order —
/// what a pre-flight needs to tell "no TLS configured" apart from "two
/// of three set", which [`tls_material_from_env`] reports identically as
/// `None`.
pub fn tls_paths_present() -> [(&'static str, bool); 3] {
    [
        "CONFLUX_TLS_CERT_PATH",
        "CONFLUX_TLS_KEY_PATH",
        "CONFLUX_TLS_CLIENT_CA_PATH",
    ]
    .map(|var| (var, std::env::var(var).is_ok()))
}

/// The PEM public key `auth = jwt` verifies tokens against. `None` when
/// unset — [`crate::validate_jwt_startup`] turns that into a refusal
/// when it matters. A path that is set but unreadable, or readable but
/// not a usable key, is an error: silently continuing unauthenticated is
/// the one outcome nobody wants from a misconfigured auth setting.
pub fn jwt_key_from_env() -> Result<Option<JwtKeyMaterial>, EnvError> {
    let Some((path, pem)) = read_path("CONFLUX_JWT_PUBLIC_KEY_PATH", &|name: &str| {
        std::env::var(name).ok()
    })?
    else {
        return Ok(None);
    };
    JwtKeyMaterial::from_public_key_pem(&pem)
        .map(Some)
        .map_err(|e| EnvError::Unusable {
            var: "CONFLUX_JWT_PUBLIC_KEY_PATH",
            path,
            message: e.to_string(),
        })
}

/// Where the trusted-reference sidecar listens, when one is configured.
pub fn trusted_reference_addr() -> Option<String> {
    std::env::var("CONFLUX_TRUSTED_REFERENCE_ADDR").ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn vars(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn an_empty_environment_selects_the_in_memory_defaults() {
        let s = backend_selection_from_vars(|_| None).unwrap();
        assert!(matches!(s.registry, RegistryBackend::Memory));
        assert!(matches!(s.store, StoreBackend::Memory));
        assert!(matches!(s.accounting, AccountingBackend::Disabled));
    }

    #[test]
    fn each_durable_backend_is_read_with_its_connection_details() {
        let m = vars(&[
            ("CONFLUX_REGISTRY_BACKEND", "redis"),
            ("CONFLUX_REDIS_URL", "redis://localhost:16379"),
            ("CONFLUX_STORE_BACKEND", "s3"),
            ("CONFLUX_S3_ENDPOINT", "http://localhost:19000"),
            ("CONFLUX_S3_BUCKET", "conflux"),
            ("CONFLUX_S3_ACCESS_KEY", "key"),
            ("CONFLUX_S3_SECRET_KEY", "secret"),
        ]);
        let s = backend_selection_from_vars(|k| m.get(k).cloned()).unwrap();
        assert!(matches!(s.registry, RegistryBackend::Redis { .. }));
        assert!(matches!(s.store, StoreBackend::S3 { ref bucket, .. } if bucket == "conflux"));
    }

    #[test]
    fn a_selection_without_its_connection_string_names_what_is_missing() {
        let m = vars(&[("CONFLUX_STORE_BACKEND", "postgres")]);
        let err = backend_selection_from_vars(|k| m.get(k).cloned()).unwrap_err();
        assert_eq!(
            err.to_string(),
            "CONFLUX_STORE_BACKEND=postgres requires CONFLUX_POSTGRES_URL"
        );
    }

    #[test]
    fn accounting_persistence_reuses_the_postgres_url_and_says_so_when_absent() {
        let m = vars(&[("CONFLUX_ACCOUNTING_PERSISTENCE", "true")]);
        let err = backend_selection_from_vars(|k| m.get(k).cloned()).unwrap_err();
        assert!(err.to_string().contains("CONFLUX_POSTGRES_URL"), "{err}");
    }

    #[test]
    fn tls_material_needs_all_three_paths_and_reports_an_unreadable_one() {
        // Two of three: not enough to bind, and not an error either —
        // `resolve_server_tls` decides whether the absence is fatal.
        let m = vars(&[
            ("CONFLUX_TLS_CERT_PATH", "/nonexistent/cert.pem"),
            ("CONFLUX_TLS_KEY_PATH", "/nonexistent/key.pem"),
        ]);
        // `TlsMaterial` deliberately has no `Debug` (it holds key
        // material), so `unwrap_err` — which would need it — is out.
        let Err(err) = tls_material_from_vars(|k| m.get(k).cloned()) else {
            panic!("an unreadable certificate path must be an error");
        };
        assert!(err.to_string().contains("CONFLUX_TLS_CERT_PATH"), "{err}");
        assert!(matches!(tls_material_from_vars(|_| None), Ok(None)));
    }
}
