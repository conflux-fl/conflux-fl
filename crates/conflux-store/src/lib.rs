//! Model checkpoint + experiment metadata persistence.
//!
//! This crate owns two related but separate jobs: saving/loading the
//! global model's weights across rounds (the `Store` trait), and
//! persisting the raw history a differential-privacy accountant needs to
//! survive a restart without its epsilon budget silently resetting (the
//! `PrivacyRoundLog` trait). Four concrete backends ship today —
//! `InMemoryStore`, `FileStore`, `PostgresStore`, `S3Store` — unified at
//! runtime by the `AnyStore` enum so a caller can pick a backend by
//! config without needing `Box<dyn Store>`.

#![warn(missing_docs)]

mod any_store;
mod postgres_store;
mod s3_store;

pub use any_store::AnyStore;
pub use postgres_store::PostgresStore;
pub use s3_store::S3Store;

/// Persists the sequence of `(noise_multiplier, sample_rate)` pairs an
/// `RdpAccountant` (`conflux-privacy`) has recorded, so a restarted
/// `conflux-server` can replay them into a fresh accountant instead of
/// silently resetting cumulative epsilon to zero.
///
/// Only `PostgresStore` implements this — `InMemoryStore`/`FileStore` have
/// no restart-durability story to extend, and a no-op impl for them would
/// give a misleading "yes, persisted" answer for a backend that isn't.
///
/// `append_round_for_client`/`load_client_rounds` are the per-client
/// counterparts, used when privacy accounting is scoped `PerClient` rather
/// than `Global` — one running epsilon per client instead of one for the
/// whole experiment. They persist the same raw `(noise_multiplier,
/// sample_rate)` shape per client, deliberately *not* a precomputed
/// cumulative-epsilon number, even though that's smaller to store. A
/// precomputed epsilon is only valid for whatever `delta` it was computed
/// with; persisting raw rounds and recomputing on load (exactly like the
/// experiment-wide history already does) stays correct under any `delta`
/// a future run resolves, not just the one in effect when a round was
/// recorded.
pub trait PrivacyRoundLog: Send + Sync {
    /// Records one experiment-wide round's raw privacy parameters.
    ///
    /// Raw parameters, not a running epsilon: epsilon depends on
    /// `delta`, so a stored total goes stale the moment a later run
    /// resolves a different one.
    fn append_round(
        &self,
        noise_multiplier: f32,
        sample_rate: f32,
    ) -> impl Future<Output = Result<(), StoreError>> + Send;
    /// Every recorded round, oldest first — enough to rebuild an
    /// accountant's state after a restart.
    fn load_rounds(&self) -> impl Future<Output = Result<Vec<(f32, f32)>, StoreError>> + Send;

    /// Records one round against a single client, for `PerClient`
    /// accounting scope.
    ///
    /// Both this and `append_round` are called on every round regardless
    /// of which scope is configured, so switching scope between restarts
    /// never loses the history that wasn't active at the time.
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
/// Why a checkpoint or privacy-log operation failed.
pub enum StoreError {
    /// Nothing has been saved yet. The first round's `load_latest_weights`
    /// hits this, and the caller substitutes its initial weights.
    #[error("no checkpoint has been saved yet")]
    NoCheckpoint,
    /// The filesystem refused a read or write.
    #[error("I/O error at {path}: {source}")]
    Io {
        /// The path being read or written.
        path: String,
        /// The underlying OS error.
        #[source]
        source: std::io::Error,
    },
    #[error(
        "checkpoint file {path} has a truncated weight buffer \
         ({len} bytes, not a multiple of 4)"
    )]
    /// A checkpoint file's byte length isn't a multiple of 4, so it
    /// cannot be a packed `f32` vector — a truncated write, usually.
    MalformedCheckpoint {
        /// The checkpoint file that could not be decoded.
        path: String,
        /// Its length in bytes, which is not a multiple of 4.
        len: usize,
    },
    /// Wraps an error from a backend that talks to a separate service —
    /// `PostgresStore`'s SQL driver, `S3Store`'s HTTP client — where the
    /// request itself failed (connection refused, auth rejected, query
    /// error). Neither `InMemoryStore` (a `HashMap`) nor `FileStore` (the
    /// local filesystem) can fail this way, so only the network-backed
    /// backends ever construct this variant. The wrapped `String` is the
    /// underlying driver's own error message — enough to diagnose an
    /// outage from a log line without this crate needing to model every
    /// possible Postgres/S3 failure mode as its own variant.
    #[error("store backend error: {0}")]
    Backend(String),
}

/// Loads the weights a new round starts from, and saves a checkpoint after
/// each round completes — the two persistence calls every backend must
/// answer, regardless of where the bytes actually live.
///
/// The methods are `async fn` directly in the trait (native syntax, no
/// `async-trait` crate needed): `PostgresStore` and `S3Store` do real
/// network I/O to answer either call, so this can't stay synchronous.
/// The tradeoff is that a trait with native `async fn` methods isn't
/// object-safe without extra boxing, so `dyn Store` doesn't work out of
/// the box. Nothing in this codebase needs it to — every caller holds a
/// concrete type, and `AnyStore` (below) is how a caller picks between
/// backends at runtime without needing dynamic dispatch at all.
pub trait Store: Send + Sync {
    /// The most recent checkpoint's weights, or `NoCheckpoint` if none
    /// has been saved.
    fn load_latest_weights(&self) -> impl Future<Output = Result<Vec<f32>, StoreError>> + Send;
    /// Persists `weights` as `round`'s checkpoint, replacing any
    /// existing checkpoint for that round.
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
    /// A store seeded with `initial_weights`, returned by
    /// `load_latest_weights` until a real checkpoint is saved over it.
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
/// little-endian `f32` array — no header, no metadata). `PostgresStore` and
/// `S3Store` implement the same `Store` trait against a database and
/// object storage respectively, for deployments that need a shared,
/// durable backend instead of the local disk this one writes to.
pub struct FileStore {
    dir: PathBuf,
}

impl FileStore {
    /// A store writing `checkpoint-<round>.bin` files into `dir`,
    /// creating the directory if it doesn't exist.
    pub fn new(dir: impl Into<PathBuf>) -> Result<Self, StoreError> {
        let dir = dir.into();
        std::fs::create_dir_all(&dir).map_err(|source| StoreError::Io {
            path: dir.display().to_string(),
            source,
        })?;
        Ok(Self { dir })
    }

    fn checkpoint_path(&self, round: u64) -> PathBuf {
        checkpoint_path_in(&self.dir, round)
    }
}

/// Free functions rather than methods, so the closures handed to
/// `spawn_blocking` can own a cloned `PathBuf` instead of borrowing
/// `&self` across a task boundary (which would need the store itself to
/// be `'static`).
fn checkpoint_path_in(dir: &Path, round: u64) -> PathBuf {
    dir.join(format!("checkpoint-{round}.bin"))
}

fn latest_round_in(dir: &Path) -> Result<Option<u64>, StoreError> {
    let entries = std::fs::read_dir(dir).map_err(|source| StoreError::Io {
        path: dir.display().to_string(),
        source,
    })?;

    let mut latest: Option<u64> = None;
    for entry in entries {
        let entry = entry.map_err(|source| StoreError::Io {
            path: dir.display().to_string(),
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

impl Store for FileStore {
    // Filesystem work runs on `spawn_blocking`, not on the thread polling
    // the future.
    //
    // The `std::fs` calls underneath are synchronous, and calling them
    // directly from an `async fn` blocks the executor thread for the
    // whole duration of the syscall. Tokio's runtime has a small, fixed
    // pool of those threads; a checkpoint write is a multi-megabyte
    // `write` that can stall on a slow disk, and while it stalls that
    // thread cannot poll *anything* — not the gRPC service accepting
    // client submissions, not the round timer. At the scale a local
    // research run writes checkpoints this was tolerable, which is why it
    // stood; on a real deployment it converts one slow disk into
    // server-wide unresponsiveness.
    //
    // `spawn_blocking` moves the work to a pool sized for exactly this,
    // leaving the async threads free. It costs one task spawn per call,
    // which is nothing beside a disk write.
    async fn load_latest_weights(&self) -> Result<Vec<f32>, StoreError> {
        let dir = self.dir.clone();
        blocking(move || {
            let round = latest_round_in(&dir)?.ok_or(StoreError::NoCheckpoint)?;
            read_weights(&checkpoint_path_in(&dir, round))
        })
        .await
    }

    async fn save_checkpoint(&self, round: u64, weights: &[f32]) -> Result<(), StoreError> {
        let path = self.checkpoint_path(round);
        let mut bytes = Vec::with_capacity(weights.len() * 4);
        for w in weights {
            bytes.extend_from_slice(&w.to_le_bytes());
        }
        blocking(move || {
            std::fs::write(&path, bytes).map_err(|source| StoreError::Io {
                path: path.display().to_string(),
                source,
            })
        })
        .await
    }
}

/// Runs `work` on tokio's blocking pool.
///
/// A `JoinError` here means the closure itself panicked (the task is
/// never cancelled — nothing holds its handle to abort it), so it is
/// surfaced as a `StoreError` rather than resumed. Re-panicking on the
/// caller's thread would take down whichever request happened to be
/// waiting, which is precisely the coupling this whole function exists
/// to remove.
async fn blocking<T, F>(work: F) -> Result<T, StoreError>
where
    F: FnOnce() -> Result<T, StoreError> + Send + 'static,
    T: Send + 'static,
{
    match tokio::task::spawn_blocking(work).await {
        Ok(result) => result,
        Err(join_error) => Err(StoreError::Io {
            path: "<blocking task>".to_string(),
            source: std::io::Error::other(format!("checkpoint task failed: {join_error}")),
        }),
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

    /// A checkpoint file is just raw bytes on disk — nothing stops it from
    /// being truncated mid-write by a crash, or corrupted by a bad disk.
    /// `read_weights` checks the buffer's *length* (a multiple of 4 bytes,
    /// one `f32` each) before trusting any of it; a file that fails that
    /// check must come back as a clean `MalformedCheckpoint` error, never
    /// a wrong-but-successful parse of partial float data.
    #[tokio::test]
    async fn file_store_errors_on_truncated_checkpoint_bytes() {
        let dir = temp_dir("malformed");
        let store = FileStore::new(&dir).unwrap();

        // 6 bytes: a valid f32 (4 bytes) plus 2 leftover bytes that can't
        // form a whole f32 — as if a crash cut the write short.
        std::fs::write(dir.join("checkpoint-1.bin"), [0u8, 1, 2, 3, 4, 5]).unwrap();

        let err = store.load_latest_weights().await.unwrap_err();

        match err {
            StoreError::MalformedCheckpoint { len, .. } => assert_eq!(len, 6),
            other => panic!("expected MalformedCheckpoint, got {other:?}"),
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    /// `latest_round` scans every entry in `dir` looking for the
    /// `checkpoint-<round>.bin` naming convention. A directory is not a
    /// namespace this crate controls exclusively — an editor swap file, a
    /// `.gitkeep`, or any other stray entry must be silently skipped
    /// rather than crashing the scan or being mistaken for a checkpoint.
    #[tokio::test]
    async fn file_store_ignores_files_that_do_not_match_the_checkpoint_naming_convention() {
        let dir = temp_dir("stray_files");
        let store = FileStore::new(&dir).unwrap();
        store.save_checkpoint(1, &[1.0]).await.unwrap();

        std::fs::write(dir.join(".gitkeep"), b"").unwrap();
        std::fs::write(dir.join("checkpoint-not-a-number.bin"), b"junk").unwrap();
        std::fs::write(dir.join("notes.txt"), b"unrelated file").unwrap();

        assert_eq!(store.load_latest_weights().await.unwrap(), vec![1.0]);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// `InMemoryStore` guards its state with a `Mutex` specifically because
    /// nothing else about it is thread-safe — this drives many concurrent
    /// `save_checkpoint` calls at once and checks that every reader
    /// afterwards sees one complete, non-garbled write (a fully-formed
    /// `Vec<f32>` from exactly one of the concurrent calls) rather than
    /// bytes torn between two writers.
    #[tokio::test]
    async fn concurrent_saves_to_in_memory_store_never_produce_a_torn_write() {
        use std::sync::Arc;

        let store = Arc::new(InMemoryStore::new(vec![0.0]));
        let mut handles = Vec::new();
        for i in 1..=20u64 {
            let store = Arc::clone(&store);
            handles.push(tokio::spawn(async move {
                store
                    .save_checkpoint(i, &[i as f32, i as f32 * 2.0])
                    .await
                    .unwrap();
            }));
        }
        for handle in handles {
            handle.await.unwrap();
        }

        let latest = store.load_latest_weights().await.unwrap();
        // Whichever round happened to write last, its two values must be
        // internally consistent (the second is always double the first) —
        // proof no two concurrent writers' bytes got interleaved.
        assert_eq!(latest.len(), 2);
        assert_eq!(latest[1], latest[0] * 2.0);
    }
}
