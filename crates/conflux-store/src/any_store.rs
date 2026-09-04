//! `AnyStore` — picks between `Store` backends at runtime.
//!
//! An enum, not `Arc<dyn Store>` — for the same reason
//! `conflux-registry::AnyRegistry` is an enum too: `Store`'s methods are
//! native `async fn` in a trait, which isn't object-safe without extra
//! boxing, so `dyn Store` isn't the easy option here. Wrapping each
//! concrete backend in an enum variant and matching on `self` in every
//! trait method gives the same "one type, several possible backends,
//! chosen at runtime" behavior a `Box<dyn Store>` would, without needing
//! object safety at all — the cost is one match arm per variant per
//! method, which stays manageable at three backends.
//!
//! `FileStore` deliberately isn't a variant here — `conflux-server`'s
//! `AppState` only ever selects between in-memory, Postgres, and S3 as
//! its runtime store backend, so there's nothing selecting `FileStore`
//! to unify. It stays available as a plain `Store` impl for standalone
//! use (e.g. a local research run that wants durability across restarts
//! without a database).

use crate::{InMemoryStore, PostgresStore, S3Store, Store, StoreError};

/// Whichever checkpoint backend this deployment selected at startup.
///
/// An enum rather than `Box<dyn Store>` because `Store`'s methods use
/// native `async fn`, which is not object-safe.
pub enum AnyStore {
    /// Process-local. Lost on restart.
    InMemory(InMemoryStore),
    /// Durable, and shared by every server process.
    Postgres(PostgresStore),
    /// Durable object storage — S3 or anything speaking its API, such as MinIO.
    S3(S3Store),
}

impl Store for AnyStore {
    async fn load_latest_weights(&self) -> Result<Vec<f32>, StoreError> {
        match self {
            Self::InMemory(s) => s.load_latest_weights().await,
            Self::Postgres(s) => s.load_latest_weights().await,
            Self::S3(s) => s.load_latest_weights().await,
        }
    }

    async fn save_checkpoint(&self, round: u64, weights: &[f32]) -> Result<(), StoreError> {
        match self {
            Self::InMemory(s) => s.save_checkpoint(round, weights).await,
            Self::Postgres(s) => s.save_checkpoint(round, weights).await,
            Self::S3(s) => s.save_checkpoint(round, weights).await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// This test module's backend URL, overridable from the environment so
    /// CI can point at its own service containers. See `.env.example`.
    fn test_backend_url(var: &str, default: &str) -> String {
        std::env::var(var).unwrap_or_else(|_| default.to_string())
    }

    #[tokio::test]
    async fn in_memory_variant_delegates_correctly() {
        let store = AnyStore::InMemory(InMemoryStore::new(vec![1.0, 2.0]));

        store.save_checkpoint(1, &[3.0, 4.0]).await.unwrap();

        assert_eq!(store.load_latest_weights().await.unwrap(), vec![3.0, 4.0]);
    }

    /// `docker run -d --name conflux-dev-postgres -e POSTGRES_PASSWORD=conflux
    /// -e POSTGRES_DB=conflux -p 15432:5432 postgres:16-alpine`
    #[tokio::test]
    async fn postgres_variant_delegates_correctly() {
        let backend = PostgresStore::connect_with_table(
            &test_backend_url(
                "CONFLUX_TEST_POSTGRES_URL",
                "postgres://postgres:conflux@127.0.0.1:15432/conflux",
            ),
            format!(
                "conflux_checkpoints_test_any_store_pg_{}",
                std::process::id()
            ),
        )
        .await
        .expect("connect to the dev Postgres container — is it running?");
        let store = AnyStore::Postgres(backend);

        store.save_checkpoint(1, &[3.0, 4.0]).await.unwrap();

        assert_eq!(store.load_latest_weights().await.unwrap(), vec![3.0, 4.0]);
    }

    /// `docker run -d --name conflux-dev-minio -p 19000:9000 -p 19001:9001
    /// -e MINIO_ROOT_USER=confluxadmin -e MINIO_ROOT_PASSWORD=confluxsecret
    /// minio/minio server /data --console-address ":9001"`
    #[tokio::test]
    async fn s3_variant_delegates_correctly() {
        let backend = S3Store::connect_with_prefix(
            &test_backend_url("CONFLUX_TEST_S3_ENDPOINT", "http://127.0.0.1:19000"),
            "conflux-test-bucket",
            "confluxadmin",
            "confluxsecret",
            format!("test-any-store-s3-{}", std::process::id()),
        )
        .await
        .expect("connect to the dev MinIO container — is it running?");
        let store = AnyStore::S3(backend);

        store.save_checkpoint(1, &[3.0, 4.0]).await.unwrap();

        assert_eq!(store.load_latest_weights().await.unwrap(), vec![3.0, 4.0]);
    }
}
