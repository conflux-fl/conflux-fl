//! `PostgresStore` — a `Store` backend durable across restarts, without
//! `FileStore`'s one-file-per-round sprawl. Also implements
//! `PrivacyRoundLog` (Phase 7d) — the actual fix for the gap Phase 7b
//! flagged and didn't close: `RdpAccountant`'s cumulative epsilon used to
//! live only in `conflux-server`'s in-process state and reset on restart.
//!
//! See `docs/phases/phase-7b-postgres-store.md` and
//! `docs/phases/phase-7d-accountant-persistence.md`.

use tokio_postgres::{Client, NoTls};

use crate::{PrivacyRoundLog, Store, StoreError};

const DEFAULT_TABLE: &str = "conflux_checkpoints";

pub struct PostgresStore {
    client: Client,
    table: String,
    /// Derived from `table`, not a second constructor parameter — this
    /// way the same per-test-unique table name Phase 7b's tests already
    /// pass gives both tables the same isolation for free.
    privacy_rounds_table: String,
}

impl PostgresStore {
    /// `postgres_url` is a plain `postgres://user:pass@host:port/db`
    /// string — stays argument-based rather than `conflux-config`-driven,
    /// matching `RedisRegistry`/`main.rs`'s precedent (spec §11 Open Item
    /// 2 is still unresolved).
    pub async fn connect(postgres_url: &str) -> Result<Self, StoreError> {
        Self::connect_with_table(postgres_url, DEFAULT_TABLE).await
    }

    /// Lets multiple independent stores share one Postgres under
    /// different tables — this module's own tests use it so `cargo
    /// test`'s parallel execution doesn't have them racing on shared rows
    /// (the same class of problem `conflux-registry`'s Redis tests hit in
    /// this same phase, fixed the same way: give each test its own
    /// namespace instead of hoping disjoint value ranges never collide).
    pub async fn connect_with_table(
        postgres_url: &str,
        table: impl Into<String>,
    ) -> Result<Self, StoreError> {
        let table = table.into();
        let (client, connection) = tokio_postgres::connect(postgres_url, NoTls)
            .await
            .map_err(|e| StoreError::Backend(e.to_string()))?;

        // tokio-postgres splits the client from its connection driver;
        // the driver has to be polled somewhere or nothing ever actually
        // talks to the socket.
        tokio::spawn(async move {
            if let Err(e) = connection.await {
                tracing::error!(error = %e, "postgres connection driver exited");
            }
        });

        // One row per round: `round BIGINT PRIMARY KEY, weights BYTEA`.
        // ADR 0003 (no multi-tenancy) is why there's no experiment-scoping
        // column — one process, one experiment, one table.
        let privacy_rounds_table = format!("{table}_privacy_rounds");
        let create_tables = format!(
            "CREATE TABLE IF NOT EXISTS {table} (
                round BIGINT PRIMARY KEY,
                weights BYTEA NOT NULL
            );
            CREATE TABLE IF NOT EXISTS {privacy_rounds_table} (
                round_index BIGSERIAL PRIMARY KEY,
                noise_multiplier REAL NOT NULL,
                sample_rate REAL NOT NULL
            );"
        );
        client
            .batch_execute(&create_tables)
            .await
            .map_err(|e| StoreError::Backend(e.to_string()))?;

        Ok(Self {
            client,
            table,
            privacy_rounds_table,
        })
    }
}

impl PrivacyRoundLog for PostgresStore {
    async fn append_round(
        &self,
        noise_multiplier: f32,
        sample_rate: f32,
    ) -> Result<(), StoreError> {
        let query = format!(
            "INSERT INTO {} (noise_multiplier, sample_rate) VALUES ($1, $2)",
            self.privacy_rounds_table
        );
        self.client
            .execute(&query, &[&noise_multiplier, &sample_rate])
            .await
            .map_err(|e| StoreError::Backend(e.to_string()))?;
        Ok(())
    }

    async fn load_rounds(&self) -> Result<Vec<(f32, f32)>, StoreError> {
        let query = format!(
            "SELECT noise_multiplier, sample_rate FROM {} ORDER BY round_index",
            self.privacy_rounds_table
        );
        let rows = self
            .client
            .query(&query, &[])
            .await
            .map_err(|e| StoreError::Backend(e.to_string()))?;
        Ok(rows
            .into_iter()
            .map(|row| (row.get(0), row.get(1)))
            .collect())
    }
}

impl Store for PostgresStore {
    async fn load_latest_weights(&self) -> Result<Vec<f32>, StoreError> {
        let query = format!(
            "SELECT weights FROM {} ORDER BY round DESC LIMIT 1",
            self.table
        );
        let row = self
            .client
            .query_opt(&query, &[])
            .await
            .map_err(|e| StoreError::Backend(e.to_string()))?;

        let Some(row) = row else {
            return Err(StoreError::NoCheckpoint);
        };
        let bytes: Vec<u8> = row.get(0);

        if !bytes.len().is_multiple_of(4) {
            return Err(StoreError::MalformedCheckpoint {
                path: format!("postgres table {}", self.table),
                len: bytes.len(),
            });
        }
        Ok(bytes
            .chunks_exact(4)
            .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect())
    }

    async fn save_checkpoint(&self, round: u64, weights: &[f32]) -> Result<(), StoreError> {
        let mut bytes = Vec::with_capacity(weights.len() * 4);
        for w in weights {
            bytes.extend_from_slice(&w.to_le_bytes());
        }
        // ON CONFLICT DO UPDATE: a retried round (the documented
        // `RoundBuffer` race from Phase 6) shouldn't fail to checkpoint
        // just because that round number was already written once.
        let query = format!(
            "INSERT INTO {} (round, weights) VALUES ($1, $2)
             ON CONFLICT (round) DO UPDATE SET weights = EXCLUDED.weights",
            self.table
        );
        self.client
            .execute(&query, &[&(round as i64), &bytes])
            .await
            .map_err(|e| StoreError::Backend(e.to_string()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// `docker run -d --name conflux-dev-postgres -e POSTGRES_PASSWORD=conflux
    /// -e POSTGRES_DB=conflux -p 15432:5432 postgres:16-alpine` — see
    /// `docs/phases/phase-7b-postgres-store.md`.
    const TEST_POSTGRES_URL: &str = "postgres://postgres:conflux@127.0.0.1:15432/conflux";

    fn unique_table(test_name: &str) -> String {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        format!(
            "conflux_checkpoints_test_{test_name}_{}_{n}",
            std::process::id()
        )
    }

    async fn connect(test_name: &str) -> PostgresStore {
        PostgresStore::connect_with_table(TEST_POSTGRES_URL, unique_table(test_name))
            .await
            .expect("connect to the dev Postgres container — is it running?")
    }

    #[tokio::test]
    async fn round_trips_through_the_database() {
        let store = connect("round_trips").await;

        store.save_checkpoint(1, &[1.5, -2.5, 3.0]).await.unwrap();

        assert_eq!(
            store.load_latest_weights().await.unwrap(),
            vec![1.5, -2.5, 3.0]
        );
    }

    #[tokio::test]
    async fn loads_the_highest_round() {
        let store = connect("highest_round").await;

        store.save_checkpoint(1, &[1.0]).await.unwrap();
        store.save_checkpoint(3, &[3.0]).await.unwrap();
        store.save_checkpoint(2, &[2.0]).await.unwrap();

        assert_eq!(store.load_latest_weights().await.unwrap(), vec![3.0]);
    }

    #[tokio::test]
    async fn errors_on_no_checkpoint() {
        let store = connect("no_checkpoint").await;

        let err = store.load_latest_weights().await.unwrap_err();

        assert!(matches!(err, StoreError::NoCheckpoint));
    }

    #[tokio::test]
    async fn upsert_lets_a_retried_round_overwrite_instead_of_erroring() {
        let store = connect("upsert").await;

        store.save_checkpoint(1, &[1.0]).await.unwrap();
        store.save_checkpoint(1, &[9.0]).await.unwrap(); // retry, same round

        assert_eq!(store.load_latest_weights().await.unwrap(), vec![9.0]);
    }

    #[tokio::test]
    async fn load_rounds_is_empty_before_any_append() {
        let store = connect("empty_rounds").await;

        assert_eq!(store.load_rounds().await.unwrap(), vec![]);
    }

    #[tokio::test]
    async fn appended_rounds_replay_in_recording_order() {
        let store = connect("append_order").await;

        store.append_round(1.0, 0.1).await.unwrap();
        store.append_round(2.0, 0.2).await.unwrap();
        store.append_round(3.0, 0.3).await.unwrap();

        assert_eq!(
            store.load_rounds().await.unwrap(),
            vec![(1.0, 0.1), (2.0, 0.2), (3.0, 0.3)]
        );
    }
}
