//! Per-field backend selection (Phase 8a) — the hybrid design from
//! `docs/FLOWER_COMPARISON.md`'s follow-up discussion: registry/store/
//! accounting choices stay fully independent (matching how those traits
//! have been decoupled since Phase 1), but `mode = production` can never
//! silently start on a backend that still resolves to its in-memory/
//! disabled default. `validate_production_backends` is the same
//! fail-fast shape `allow_stub_client` already uses (spec §7) — a
//! resolved safety posture that's actually *checked*, not just logged.

use conflux_config::Mode;

#[derive(Debug, Clone, Default)]
pub enum RegistryBackend {
    #[default]
    Memory,
    Redis {
        url: String,
    },
}

#[derive(Debug, Clone, Default)]
pub enum StoreBackend {
    #[default]
    Memory,
    Postgres {
        url: String,
    },
    S3 {
        endpoint: String,
        bucket: String,
        access_key: String,
        secret_key: String,
    },
}

#[derive(Debug, Clone, Default)]
pub enum AccountingBackend {
    #[default]
    Disabled,
    Postgres {
        url: String,
    },
}

#[derive(Debug, Clone, Default)]
pub struct BackendSelection {
    pub registry: RegistryBackend,
    pub store: StoreBackend,
    pub accounting: AccountingBackend,
}

#[derive(Debug, thiserror::Error)]
pub enum BackendSelectionError {
    #[error(
        "mode = production requires a durable registry backend (set \
         CONFLUX_REGISTRY_BACKEND=redis and CONFLUX_REDIS_URL) — refusing to \
         start with in-memory client registrations that would be lost on restart"
    )]
    ProductionRequiresDurableRegistry,
    #[error(
        "mode = production requires a durable store backend (set \
         CONFLUX_STORE_BACKEND=postgres or s3, with the matching connection \
         env vars) — refusing to start with in-memory checkpoints that would \
         be lost on restart"
    )]
    ProductionRequiresDurableStore,
    #[error(
        "mode = production requires persistent privacy accounting (set \
         CONFLUX_ACCOUNTING_PERSISTENCE=true and CONFLUX_POSTGRES_URL) — \
         refusing to start with an epsilon budget that would silently reset \
         on restart"
    )]
    ProductionRequiresPersistentAccounting,
}

pub fn validate_production_backends(
    mode: Mode,
    selection: &BackendSelection,
) -> Result<(), BackendSelectionError> {
    if mode != Mode::Production {
        return Ok(());
    }
    if matches!(selection.registry, RegistryBackend::Memory) {
        return Err(BackendSelectionError::ProductionRequiresDurableRegistry);
    }
    if matches!(selection.store, StoreBackend::Memory) {
        return Err(BackendSelectionError::ProductionRequiresDurableStore);
    }
    if matches!(selection.accounting, AccountingBackend::Disabled) {
        return Err(BackendSelectionError::ProductionRequiresPersistentAccounting);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn research_mode_never_fails_regardless_of_backend_selection() {
        assert!(validate_production_backends(Mode::Research, &BackendSelection::default()).is_ok());
    }

    #[test]
    fn production_fails_on_in_memory_registry() {
        let selection = BackendSelection {
            store: StoreBackend::Postgres {
                url: "postgres://x".to_string(),
            },
            accounting: AccountingBackend::Postgres {
                url: "postgres://x".to_string(),
            },
            ..Default::default()
        };

        let err = validate_production_backends(Mode::Production, &selection).unwrap_err();
        assert!(matches!(
            err,
            BackendSelectionError::ProductionRequiresDurableRegistry
        ));
    }

    #[test]
    fn production_fails_on_in_memory_store() {
        let selection = BackendSelection {
            registry: RegistryBackend::Redis {
                url: "redis://x".to_string(),
            },
            accounting: AccountingBackend::Postgres {
                url: "postgres://x".to_string(),
            },
            ..Default::default()
        };

        let err = validate_production_backends(Mode::Production, &selection).unwrap_err();
        assert!(matches!(
            err,
            BackendSelectionError::ProductionRequiresDurableStore
        ));
    }

    #[test]
    fn production_fails_on_disabled_accounting_persistence() {
        let selection = BackendSelection {
            registry: RegistryBackend::Redis {
                url: "redis://x".to_string(),
            },
            store: StoreBackend::Postgres {
                url: "postgres://x".to_string(),
            },
            ..Default::default()
        };

        let err = validate_production_backends(Mode::Production, &selection).unwrap_err();
        assert!(matches!(
            err,
            BackendSelectionError::ProductionRequiresPersistentAccounting
        ));
    }

    #[test]
    fn production_succeeds_when_everything_is_durable() {
        let selection = BackendSelection {
            registry: RegistryBackend::Redis {
                url: "redis://x".to_string(),
            },
            store: StoreBackend::Postgres {
                url: "postgres://x".to_string(),
            },
            accounting: AccountingBackend::Postgres {
                url: "postgres://x".to_string(),
            },
        };

        assert!(validate_production_backends(Mode::Production, &selection).is_ok());
    }
}
