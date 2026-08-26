//! Model checkpoint + experiment metadata persistence.
//!
//! See `docs/spec/conflux-spec-v1.md` §8.

mod any_store;
mod postgres_store;
mod s3_store;

pub use any_store::AnyStore;
pub use postgres_store::PostgresStore;
pub use s3_store::S3Store;

/// Persists the sequence of `(noise_multiplier, sample_rate)` pairs an
/// `RdpAccountant` (`conflux-privacy`) has recorded, so a restarted
/// `conflux-server` can replay them into a fresh accountant instead of
/// silently resetting cumulative epsilon to zero. Spec §10 names this the
/// reason `ExperimentStore`/Postgres exists at all — see
/// `docs/phases/phase-7d-accountant-persistence.md`.
///
/// Only `PostgresStore` implements this — `InMemoryStore`/`FileStore` have
/// no restart-durability story to extend, and a no-op impl for them would
/// give a misleading "yes, persisted" answer for a backend that isn't.
///
/// Phase 14 (`AccountingScope::PerClient`): `append_round_for_client`/
/// `load_client_rounds` are the per-client counterparts, persisting the
/// same raw `(noise_multiplier, sample_rate)` shape — deliberately *not*
/// a precomputed cumulative-epsilon number, even though that's smaller
/// to store. A precomputed epsilon is only valid for whatever `delta` it
/// was computed with; persisting raw rounds and recomputing on load
/// (exactly like the experiment-wide history already does) stays correct
/// under any `delta` a future run resolves, not just the one in effect
/// when a round was recorded.
pub trait PrivacyRoundLog: Send + Sync {
    fn append_round(
        &self,
        noise_multiplier: f32,
        sample_rate: f32,
    ) -> impl Future<Output = Result<(), StoreError>> + Send;
    fn load_rounds(&self) -> impl Future<Output = Result<Vec<(f32, f32)>, StoreError>> + Send;

    fn append_round_for_client(
        &self,
        client_id: &str,
        noise_multiplier: f32,
        sample_rate: f32,
    ) -> impl Future<Output = Result<(), StoreError>> + Send;
    /// Every client's full round history at once — mirrors
    /// `load_rounds`'s "load everything, replay it all" shape; a
    /// restarted server needs every client's history to rebuild the
    /// accountant, not one client's at a time.
    fn load_client_rounds(
        &self,
    ) -> impl Future<Output = Result<std::collections::HashMap<String, Vec<(f32, f32)>>, StoreError>>
    + Send;
}

use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("no checkpoint has been saved yet")]
    NoCheckpoint,
    #[error("I/O error at {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error(
        "checkpoint file {path} has a truncated weight buffer \
         ({len} bytes, not a multiple of 4)"
    )]
    MalformedCheckpoint { path: String, len: usize },
    /// Neither `InMemoryStore` nor `FileStore` (Phase 2a) needed this — a
    /// `HashMap`/local filesystem doesn't fail this way. `PostgresStore`
    /// (Phase 7b) is the first backend to talk to a separate service, so
    /// the trait needed a variant for "the backend itself is
    /// unreachable/erroring" — same reasoning as `conflux-registry`'s
    /// `RegistryError::Backend`, added for `RedisRegistry` in this same
    /// phase.
    #[error("store backend error: {0}")]
    Backend(String),
}

/// Load the round's starting weights, save a new checkpoint after each
/// round. Spec §8's Step 0 (`load_latest_weights`) and Step 4
/// (`save_checkpoint`).
///
/// `async fn` (native syntax, no `async-trait` needed): `PostgresStore`
/// does real network I/O, so this can't stay synchronous. Not
/// dyn-compatible without extra work, but nothing in this codebase needs
/// `dyn Store` — every caller holds a concrete type.
pub trait Store: Send + Sync {
    fn load_latest_weights(&self) -> impl Future<Output = Result<Vec<f32>, StoreError>> + Send;
    fn save_checkpoint(
        &self,
        round: u64,
        weights: &[f32],
    ) -> impl Future<Output = Result<(), StoreError>> + Send;
}

/// Research/testing backend — the latest checkpoint lives in process
/// memory and is lost on restart. Seeded with an initial global model at
/// construction so `load_latest_weights` always has something to return.
pub struct InMemoryStore {
    latest: Mutex<(u64, Vec<f32>)>,
}

impl InMemoryStore {
    pub fn new(initial_weights: Vec<f32>) -> Self {
        Self {
            latest: Mutex::new((0, initial_weights)),
        }
    }
}

impl Store for InMemoryStore {
    async fn load_latest_weights(&self) -> Result<Vec<f32>, StoreError> {
        Ok(self.latest.lock().expect("store mutex poisoned").1.clone())
    }

    async fn save_checkpoint(&self, round: u64, weights: &[f32]) -> Result<(), StoreError> {
        let mut latest = self.latest.lock().expect("store mutex poisoned");
        *latest = (round, weights.to_vec());
        Ok(())
    }
}

/// One flat file per round under `dir` (`checkpoint-<round>.bin`, a raw
/// little-endian `f32` array — no header, no metadata). `S3Store` (Phase 7)
/// will implement the same `Store` trait against object storage instead.
pub struct FileStore {
    dir: PathBuf,
}

impl FileStore {
    pub fn new(dir: impl Into<PathBuf>) -> Result<Self, StoreError> {
        let dir = dir.into();
        std::fs::create_dir_all(&dir).map_err(|source| StoreError::Io {
            path: dir.display().to_string(),
            source,
        })?;
        Ok(Self { dir })
    }

    fn checkpoint_path(&self, round: u64) -> PathBuf {
        self.dir.join(format!("checkpoint-{round}.bin"))
    }

    fn latest_round(&self) -> Result<Option<u64>, StoreError> {
        let entries = std::fs::read_dir(&self.dir).map_err(|source| StoreError::Io {
            path: self.dir.display().to_string(),
            source,
        })?;

        let mut latest: Option<u64> = None;
        for entry in entries {
            let entry = entry.map_err(|source| StoreError::Io {
                path: self.dir.display().to_string(),
                source,
            })?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if let Some(round) = name
                .strip_prefix("checkpoint-")
                .and_then(|s| s.strip_suffix(".bin"))
                .and_then(|s| s.parse::<u64>().ok())
            {
                latest = Some(latest.map_or(round, |current| current.max(round)));
            }
        }
        Ok(latest)
    }
}

impl Store for FileStore {
    // Still plain `std::fs` calls under the hood — this crate has no
    // async runtime dependency of its own, and `InMemoryStore` needs
    // none either. That means a `FileStore` call briefly blocks whatever
    // thread polls it; fine for `InMemoryStore`-scale I/O but a real
    // cleanup candidate (`tokio::task::spawn_blocking`) if `FileStore`
    // ever needs to stop blocking the executor under load — out of scope
    // for this phase, which is about adding `PostgresStore`, not
    // revisiting Phase 2a's backends.
    async fn load_latest_weights(&self) -> Result<Vec<f32>, StoreError> {
        let round = self.latest_round()?.ok_or(StoreError::NoCheckpoint)?;
        read_weights(&self.checkpoint_path(round))
    }

    async fn save_checkpoint(&self, round: u64, weights: &[f32]) -> Result<(), StoreError> {
        let path = self.checkpoint_path(round);
        let mut bytes = Vec::with_capacity(weights.len() * 4);
        for w in weights {
            bytes.extend_from_slice(&w.to_le_bytes());
        }
        std::fs::write(&path, bytes).map_err(|source| StoreError::Io {
            path: path.display().to_string(),
            source,
        })
    }
}

fn read_weights(path: &Path) -> Result<Vec<f32>, StoreError> {
    let bytes = std::fs::read(path).map_err(|source| StoreError::Io {
        path: path.display().to_string(),
        source,
    })?;
    if bytes.len() % 4 != 0 {
        return Err(StoreError::MalformedCheckpoint {
            path: path.display().to_string(),
            len: bytes.len(),
        });
    }
    Ok(bytes
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    #[tokio::test]
    async fn in_memory_store_returns_seed_before_any_save() {
        let store = InMemoryStore::new(vec![1.0, 2.0, 3.0]);

        assert_eq!(
            store.load_latest_weights().await.unwrap(),
            vec![1.0, 2.0, 3.0]
        );
    }

    #[tokio::test]
    async fn in_memory_store_round_trips() {
        let store = InMemoryStore::new(vec![0.0]);

        store.save_checkpoint(1, &[1.0, 2.0]).await.unwrap();

        assert_eq!(store.load_latest_weights().await.unwrap(), vec![1.0, 2.0]);
    }

    fn temp_dir(test_name: &str) -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "conflux-store-test-{test_name}-{}-{n}",
            std::process::id()
        ))
    }

    #[tokio::test]
    async fn file_store_round_trips() {
        let dir = temp_dir("round_trips");
        let store = FileStore::new(&dir).unwrap();

        store.save_checkpoint(1, &[1.5, -2.5, 3.0]).await.unwrap();

        assert_eq!(
            store.load_latest_weights().await.unwrap(),
            vec![1.5, -2.5, 3.0]
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn file_store_loads_highest_round() {
        let dir = temp_dir("highest_round");
        let store = FileStore::new(&dir).unwrap();

        store.save_checkpoint(1, &[1.0]).await.unwrap();
        store.save_checkpoint(3, &[3.0]).await.unwrap();
        store.save_checkpoint(2, &[2.0]).await.unwrap();

        assert_eq!(store.load_latest_weights().await.unwrap(), vec![3.0]);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn file_store_errors_on_empty_directory() {
        let dir = temp_dir("empty");
        let store = FileStore::new(&dir).unwrap();

        let err = store.load_latest_weights().await.unwrap_err();

        assert!(matches!(err, StoreError::NoCheckpoint));
        std::fs::remove_dir_all(&dir).ok();
    }
}
