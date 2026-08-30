//! Runnable "try it" for the [crate-deep-dives article on
//! `conflux-store`](https://confluxfl.dev/crate-deep-dives/conflux-store/).
//!
//! Run with:
//!   cargo run --example checkpoint_roundtrip -p conflux-store
//!
//! Exercises the two things this crate is really about: the `Store` trait
//! (load the round's starting weights, save a checkpoint after each round)
//! and `AnyStore`, the enum that lets a caller hold "some `Store` backend,
//! decided at runtime" without `Box<dyn Store>`. Uses `InMemoryStore`
//! (wrapped in `AnyStore`) and `FileStore` (used directly — it isn't an
//! `AnyStore` variant; nothing in the framework selects it as a runtime
//! choice) — the two backends that need no external service, so this runs
//! anywhere with zero setup. `PostgresStore`/`S3Store` implement the
//! identical `Store` trait against a real database/object store instead,
//! and are `AnyStore` variants alongside `InMemoryStore`.

use conflux_store::{AnyStore, FileStore, InMemoryStore, Store, StoreError};

#[tokio::main]
async fn main() {
    // `AnyStore` is a plain enum, not `Arc<dyn Store>` — `Store`'s methods
    // are native `async fn`, which isn't object-safe without extra work.
    // Wrapping each backend in a variant and matching on it in `AnyStore`'s
    // own `Store` impl gets the same "pick a backend at runtime" behavior
    // through static dispatch instead.
    let store = AnyStore::InMemory(InMemoryStore::new(vec![0.1, 0.2, 0.3]));

    let seed = store.load_latest_weights().await.unwrap();
    println!("seed weights before any round has completed: {seed:?}");

    store.save_checkpoint(1, &[1.0, 2.0, 3.0]).await.unwrap();
    store.save_checkpoint(2, &[1.5, 2.5, 3.5]).await.unwrap();

    let latest = store.load_latest_weights().await.unwrap();
    println!("latest weights after rounds 1 and 2: {latest:?}");

    // A second, independently-typed backend behind the same `Store` trait
    // — `FileStore` persists to a temp directory on local disk instead of
    // process memory. It implements `Store` directly rather than through
    // `AnyStore`: nothing in `conflux-server` picks it as a runtime
    // backend choice, so there is no enum variant to unify it into.
    let dir = std::env::temp_dir().join(format!("conflux-store-example-{}", std::process::id()));
    let store = FileStore::new(&dir).unwrap();

    let err = store.load_latest_weights().await.unwrap_err();
    println!("FileStore with nothing saved yet errors instead of guessing: {err}");
    assert!(matches!(err, StoreError::NoCheckpoint));

    store.save_checkpoint(7, &[9.0, 9.5]).await.unwrap();
    let latest = store.load_latest_weights().await.unwrap();
    println!("FileStore after saving round 7: {latest:?}");

    std::fs::remove_dir_all(&dir).ok();
}
