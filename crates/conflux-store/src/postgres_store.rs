//! `PostgresStore` — a `Store` backend durable across restarts, without
//! `FileStore`'s one-file-per-round sprawl on local disk. Also implements
//! `PrivacyRoundLog`: without it, an `RdpAccountant`'s cumulative epsilon
//! lives only in `conflux-server`'s in-process state and silently resets
//! to zero on restart, which is a real problem for the "how much privacy
//! budget is left" guarantee the accountant exists to enforce.

use tokio_postgres::{Client, NoTls};

use crate::{PrivacyRoundLog, Store, StoreError};

const DEFAULT_TABLE: &str = "conflux_checkpoints";

/// A `Store` (and `PrivacyRoundLog`) backed by real Postgres, so
/// checkpoints and privacy history survive a restart.
pub struct PostgresStore {
    client: Client,
    table: String,
    /// Derived from `table`, not a second constructor parameter — this
    /// way, callers that already pass a per-test or per-deployment unique
    /// `table` name get the same isolation for the privacy-round table
    /// for free, with nothing extra to plumb through.
    privacy_rounds_table: String,
    /// Same derivation pattern as `privacy_rounds_table`, one more table
    /// for per-client round history (used by `PerClient` privacy
    /// accounting).
    client_privacy_rounds_table: String,
}

impl PostgresStore {
    /// `postgres_url` is a plain `postgres://user:pass@host:port/db`
    /// string, passed directly by whatever constructs this store (an env
    /// var read by `conflux-server`'s startup code, typically) — this
    /// crate does not itself resolve config or parse env vars.
    pub async fn connect(postgres_url: &str) -> Result<Self, StoreError> {
        Self::connect_with_table(postgres_url, DEFAULT_TABLE).await
    }

    /// Lets multiple independent stores share one Postgres under
    /// different tables — this module's own tests use it so `cargo
    /// test`'s parallel execution doesn't have them racing on shared rows:
    /// each test gets its own table name instead of hoping disjoint value
    /// ranges never collide.
    pub async fn connect_with_table(
        postgres_url: &str,
        table: impl Into<String>,
    ) -> Result<Self, StoreError> {
        let table = table.into();
        // The table name is spliced into SQL text below (a bind parameter
        // cannot name a table), so it must be a plain identifier — a
        // table name of `x; DROP TABLE y` is refused here, before it
        // reaches the database.
        if !is_plain_identifier(&table) {
            return Err(StoreError::Backend(format!(
                "table name {table:?} is not a plain SQL identifier \
                 (letters, digits, underscores; not starting with a digit)"
            )));
        }
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
        // No experiment-scoping column — one `conflux-server` process runs
        // exactly one experiment, so one table is always one experiment's
        // checkpoints; running a second experiment means running a second
        // process against its own table/database.
        let privacy_rounds_table = format!("{table}_privacy_rounds");
        // One row per (client, round) — `client_id` alongside the same
        // `noise_multiplier`/`sample_rate` shape the experiment-wide table
        // already uses, per `PrivacyRoundLog`'s own doc comment on why raw
        // rounds (not a precomputed epsilon) are what gets persisted.
        let client_privacy_rounds_table = format!("{table}_client_privacy_rounds");
        let create_tables = format!(
            "CREATE TABLE IF NOT EXISTS {table} (
                round BIGINT PRIMARY KEY,
                weights BYTEA NOT NULL
            );
            CREATE TABLE IF NOT EXISTS {privacy_rounds_table} (
                round_index BIGSERIAL PRIMARY KEY,
                noise_multiplier REAL NOT NULL,
                sample_rate REAL NOT NULL
            );
            CREATE TABLE IF NOT EXISTS {client_privacy_rounds_table} (
                round_index BIGSERIAL PRIMARY KEY,
                client_id TEXT NOT NULL,
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
            client_privacy_rounds_table,
        })
    }
}

/// `[A-Za-z_][A-Za-z0-9_]*` — the identifier grammar that needs no
/// quoting and cannot smuggle a second statement.
fn is_plain_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
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

    async fn append_round_for_client(
        &self,
        client_id: &str,
        noise_multiplier: f32,
        sample_rate: f32,
    ) -> Result<(), StoreError> {
        let query = format!(
            "INSERT INTO {} (client_id, noise_multiplier, sample_rate) VALUES ($1, $2, $3)",
            self.client_privacy_rounds_table
        );
        self.client
            .execute(&query, &[&client_id, &noise_multiplier, &sample_rate])
            .await
            .map_err(|e| StoreError::Backend(e.to_string()))?;
        Ok(())
    }

    async fn load_client_rounds(
        &self,
    ) -> Result<std::collections::HashMap<String, Vec<(f32, f32)>>, StoreError> {
        let query = format!(
            "SELECT client_id, noise_multiplier, sample_rate FROM {} ORDER BY round_index",
            self.client_privacy_rounds_table
        );
        let rows = self
            .client
            .query(&query, &[])
            .await
            .map_err(|e| StoreError::Backend(e.to_string()))?;
        let mut by_client: std::collections::HashMap<String, Vec<(f32, f32)>> =
            std::collections::HashMap::new();
        for row in rows {
            let client_id: String = row.get(0);
            by_client
                .entry(client_id)
                .or_default()
                .push((row.get(1), row.get(2)));
        }
        Ok(by_client)
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
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| f32::from_le_bytes(*c))
            .collect())
    }

    async fn save_checkpoint(&self, round: u64, weights: &[f32]) -> Result<(), StoreError> {
        let mut bytes = Vec::with_capacity(weights.len() * 4);
        for w in weights {
            bytes.extend_from_slice(&w.to_le_bytes());
        }
        // ON CONFLICT DO UPDATE: a retried round (e.g. a round buffer
        // that flushes the same round number twice under a race) shouldn't
        // fail to checkpoint just because that round number was already
        // written once — the later write should simply win.
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

    use std::collections::HashMap;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// This test module's backend URL, overridable from the environment so
    /// CI can point at its own service containers. See `.env.example`.
    fn test_backend_url(var: &str, default: &str) -> String {
        std::env::var(var).unwrap_or_else(|_| default.to_string())
    }

    /// `docker run -d --name conflux-dev-postgres -e POSTGRES_PASSWORD=conflux
    /// -e POSTGRES_DB=conflux -p 15432:5432 postgres:16-alpine`
    fn test_postgres_url() -> String {
        test_backend_url(
            "CONFLUX_TEST_POSTGRES_URL",
            "postgres://postgres:conflux@127.0.0.1:15432/conflux",
        )
    }

    fn unique_table(test_name: &str) -> String {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        format!(
            "conflux_checkpoints_test_{test_name}_{}_{n}",
            std::process::id()
        )
    }

    async fn connect(test_name: &str) -> PostgresStore {
        PostgresStore::connect_with_table(&test_postgres_url(), unique_table(test_name))
            .await
            .expect("connect to the dev Postgres container — is it running?")
    }

    /// Table names are spliced into SQL, so anything but a plain
    /// identifier is refused before a connection is even attempted.
    #[tokio::test]
    async fn a_table_name_that_is_not_a_plain_identifier_is_refused() {
        let Err(err) =
            PostgresStore::connect_with_table("postgres://unused", "x; DROP TABLE y").await
        else {
            panic!("a table name carrying a statement separator must be refused");
        };
        assert!(matches!(err, StoreError::Backend(msg) if msg.contains("plain SQL identifier")));

        assert!(is_plain_identifier("conflux_checkpoints_1"));
        assert!(!is_plain_identifier("1abc"));
        assert!(!is_plain_identifier(""));
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

    // PerClient accounting persistence.

    #[tokio::test]
    async fn load_client_rounds_is_empty_before_any_append() {
        let store = connect("empty_client_rounds").await;

        assert_eq!(store.load_client_rounds().await.unwrap(), HashMap::new());
    }

    #[tokio::test]
    async fn appended_client_rounds_replay_in_recording_order_per_client() {
        let store = connect("client_append_order").await;

        store
            .append_round_for_client("client-a", 1.0, 0.1)
            .await
            .unwrap();
        store
            .append_round_for_client("client-b", 2.0, 0.2)
            .await
            .unwrap();
        store
            .append_round_for_client("client-a", 3.0, 0.3)
            .await
            .unwrap();

        let loaded = store.load_client_rounds().await.unwrap();
        assert_eq!(
            loaded.get("client-a").unwrap(),
            &vec![(1.0, 0.1), (3.0, 0.3)]
        );
        assert_eq!(loaded.get("client-b").unwrap(), &vec![(2.0, 0.2)]);
    }

    /// The real "restart recovery" property `PrivacyRoundLog` exists
    /// for: a fresh `PostgresStore` instance against the *same* table
    /// (simulating a server restart) recovers every client's exact
    /// history — mirrors `appended_rounds_replay_in_recording_order`'s
    /// global-scope equivalent, now per-client.
    #[tokio::test]
    async fn client_rounds_survive_a_simulated_restart() {
        let table = unique_table("client_restart_recovery");
        {
            let store = PostgresStore::connect_with_table(&test_postgres_url(), table.clone())
                .await
                .unwrap();
            store
                .append_round_for_client("client-a", 1.0, 0.1)
                .await
                .unwrap();
            store
                .append_round_for_client("client-a", 1.0, 0.1)
                .await
                .unwrap();
            store
                .append_round_for_client("client-b", 1.0, 0.1)
                .await
                .unwrap();
        } // `store` (and its connection) dropped — simulates the process exiting

        let restarted = PostgresStore::connect_with_table(&test_postgres_url(), table)
            .await
            .unwrap();
        let recovered = restarted.load_client_rounds().await.unwrap();

        assert_eq!(recovered.get("client-a").unwrap().len(), 2);
        assert_eq!(recovered.get("client-b").unwrap().len(), 1);
    }
}
