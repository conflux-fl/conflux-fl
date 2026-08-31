//! Client lifecycle (register/heartbeat/evict).
//!
//! Before a round can pick clients to train on, the server needs to know
//! which clients currently exist and are still alive. This crate is the
//! source of truth for that: a client calls `register` once, then
//! `heartbeat` periodically to prove it's still around; anything that
//! stops heartbeating past a TTL drops out of `active_clients` on the
//! next sweep. Two backends ship behind the same `Registry` trait —
//! `InMemoryRegistry` for a single process, `RedisRegistry` for a
//! deployment that needs the registry durable across restarts and shared
//! across multiple server processes.

#![warn(missing_docs)]

mod any_node_allowlist;
mod any_registry;
mod node_allowlist;
mod redis_node_allowlist;
mod redis_registry;

pub use any_node_allowlist::AnyNodeAllowlist;
pub use any_registry::AnyRegistry;
pub use node_allowlist::{InMemoryNodeAllowlist, NodeAllowlist, NodeAuthError, NodeIdentity};
pub use redis_node_allowlist::RedisNodeAllowlist;
pub use redis_registry::RedisRegistry;

use std::collections::HashMap;
use std::future::Future;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Identifies one client across registration, heartbeat, and selection.
///
/// Wrapping the raw `String` in its own type (a "newtype") means a
/// `ClientId` can never be silently passed where, say, an experiment id or
/// task id was expected — the compiler catches the mix-up instead of it
/// surfacing as a runtime bug.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ClientId(pub String);

impl std::fmt::Display for ClientId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Every error a `Registry` implementation can return.
///
/// `thiserror`'s derive macro generates the `Display`/`std::error::Error`
/// impls from the `#[error("...")]` strings below, so callers get a real
/// typed enum to match on instead of a bag of `String`s — see the
/// `Conventions` section of `CLAUDE.md`.
#[derive(Debug, thiserror::Error)]
pub enum RegistryError {
    #[error("client {0} is already registered")]
    /// This client is already registered. Callers generally treat this as
    /// success — a second registration is a retry, not an error.
    AlreadyRegistered(ClientId),
    #[error("client {0} is not registered")]
    /// No such client. Usually means it was evicted after its TTL lapsed
    /// and should register again.
    NotRegistered(ClientId),
    /// An in-process `HashMap` (`InMemoryRegistry`) never fails, but a
    /// backend that does real I/O (`RedisRegistry`) can — a connection
    /// drop, a timeout, a Redis error reply. This variant is how a caller
    /// tells "the backend itself is unreachable/erroring" apart from the
    /// two business-logic errors above.
    #[error("registry backend error: {0}")]
    Backend(String),
}

#[derive(Debug, Clone)]
/// What the registry knows about one registered client.
pub struct ClientInfo {
    /// The client's identity.
    pub id: ClientId,
    /// When it first registered.
    pub registered_at: Instant,
    /// When it last checked in. Eviction compares this against the
    /// configured TTL.
    pub last_heartbeat: Instant,
}

/// Who's known to the server right now, and are they still alive.
///
/// A `trait` (rather than a concrete struct) so `conflux-server` can depend
/// on client-lifecycle behavior without committing to a storage backend:
/// `InMemoryRegistry` here, `RedisRegistry` for a durable, multi-process
/// deployment — same interface, different durability/scaling tradeoffs.
///
/// `Send + Sync` because `conflux-server` shares one registry across
/// concurrently handled requests — every implementation must be safe to
/// call from multiple threads/tasks at once.
///
/// Methods are `async fn` (stable native syntax, no `async-trait` needed)
/// because a real backend does network I/O — `InMemoryRegistry`'s bodies
/// have no `.await` in them at all, but `RedisRegistry`'s do. Native async
/// fn in traits isn't automatically object-safe, but nothing in this
/// codebase needs `dyn Registry` (every caller holds a concrete type), so
/// that's not a cost paid for no reason.
pub trait Registry: Send + Sync {
    /// Admits a client. Errors with `AlreadyRegistered` if it is already
    /// known — which most callers treat as success.
    fn register(&self, id: ClientId) -> impl Future<Output = Result<(), RegistryError>> + Send;
    /// Records that a client is still alive, resetting its eviction
    /// clock. Errors with `NotRegistered` if it was already evicted.
    fn heartbeat(&self, id: &ClientId) -> impl Future<Output = Result<(), RegistryError>> + Send;
    /// Removes clients whose last heartbeat is older than `ttl` — a
    /// best-effort sweep (an `InMemoryRegistry` sweep can't fail; a
    /// `RedisRegistry` one that hits a transient error just does less
    /// than it could, and the next scheduled sweep tries again), so this
    /// stays infallible rather than giving callers an error they have no
    /// good action for.
    fn evict_expired(&self, ttl: Duration) -> impl Future<Output = ()> + Send;
    /// Clients currently known and not evicted. Fallible — unlike a sweep,
    /// a caller reading this list (`run_round`'s client selection) needs
    /// to tell "genuinely zero clients" apart from "the backend is
    /// unreachable"; collapsing both into an empty `Vec` would make an
    /// outage look identical to an idle experiment.
    fn active_clients(&self) -> impl Future<Output = Result<Vec<ClientId>, RegistryError>> + Send;
}

/// The simplest `Registry` implementation — a mutex-guarded in-memory map.
/// Everything is lost on restart and nothing is shared across processes;
/// `RedisRegistry` is the backend for when either of those matters.
///
/// `Mutex` rather than `RwLock`: every operation here, including
/// `active_clients`, is a quick map scan/mutation — there's no read-heavy
/// workload yet that would justify separate read/write locking.
pub struct InMemoryRegistry {
    clients: Mutex<HashMap<ClientId, ClientInfo>>,
}

impl InMemoryRegistry {
    /// An empty registry.
    pub fn new() -> Self {
        Self {
            clients: Mutex::new(HashMap::new()),
        }
    }
}

impl Default for InMemoryRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl Registry for InMemoryRegistry {
    async fn register(&self, id: ClientId) -> Result<(), RegistryError> {
        let mut clients = self.clients.lock().expect("registry mutex poisoned");
        if clients.contains_key(&id) {
            return Err(RegistryError::AlreadyRegistered(id));
        }
        let now = Instant::now();
        clients.insert(
            id.clone(),
            ClientInfo {
                id,
                registered_at: now,
                last_heartbeat: now,
            },
        );
        Ok(())
    }

    async fn heartbeat(&self, id: &ClientId) -> Result<(), RegistryError> {
        let mut clients = self.clients.lock().expect("registry mutex poisoned");
        match clients.get_mut(id) {
            Some(info) => {
                info.last_heartbeat = Instant::now();
                Ok(())
            }
            None => Err(RegistryError::NotRegistered(id.clone())),
        }
    }

    async fn evict_expired(&self, ttl: Duration) {
        let mut clients = self.clients.lock().expect("registry mutex poisoned");
        let now = Instant::now();
        clients.retain(|_, info| now.duration_since(info.last_heartbeat) < ttl);
    }

    async fn active_clients(&self) -> Result<Vec<ClientId>, RegistryError> {
        let clients = self.clients.lock().expect("registry mutex poisoned");
        Ok(clients.keys().cloned().collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(s: &str) -> ClientId {
        ClientId(s.to_string())
    }

    #[tokio::test]
    async fn register_then_appears_in_active_clients() {
        let registry = InMemoryRegistry::new();
        registry.register(id("c1")).await.unwrap();

        assert_eq!(registry.active_clients().await.unwrap(), vec![id("c1")]);
    }

    #[tokio::test]
    async fn duplicate_register_errors() {
        let registry = InMemoryRegistry::new();
        registry.register(id("c1")).await.unwrap();

        let err = registry.register(id("c1")).await.unwrap_err();
        assert!(matches!(err, RegistryError::AlreadyRegistered(client) if client == id("c1")));
    }

    #[tokio::test]
    async fn heartbeat_unknown_client_errors() {
        let registry = InMemoryRegistry::new();

        let err = registry.heartbeat(&id("ghost")).await.unwrap_err();
        assert!(matches!(err, RegistryError::NotRegistered(client) if client == id("ghost")));
    }

    #[tokio::test]
    async fn heartbeat_known_client_succeeds() {
        let registry = InMemoryRegistry::new();
        registry.register(id("c1")).await.unwrap();

        registry.heartbeat(&id("c1")).await.unwrap();
    }

    #[tokio::test]
    async fn evict_expired_removes_stale_clients() {
        let registry = InMemoryRegistry::new();
        registry.register(id("stale")).await.unwrap();

        tokio::time::sleep(Duration::from_millis(20)).await;
        registry.register(id("fresh")).await.unwrap();

        registry.evict_expired(Duration::from_millis(10)).await;

        assert_eq!(registry.active_clients().await.unwrap(), vec![id("fresh")]);
    }

    #[tokio::test]
    async fn evict_expired_keeps_clients_within_ttl() {
        let registry = InMemoryRegistry::new();
        registry.register(id("c1")).await.unwrap();

        registry.evict_expired(Duration::from_secs(60)).await;

        assert_eq!(registry.active_clients().await.unwrap(), vec![id("c1")]);
    }

    /// A client evicted for going stale isn't permanently blocked from
    /// this id — `AlreadyRegistered` should only fire while the id is
    /// still tracked. If eviction only cleared some side table and left
    /// the id "reserved," a client that dropped off and came back would
    /// be locked out under its own name forever.
    #[tokio::test]
    async fn client_can_re_register_after_being_evicted() {
        let registry = InMemoryRegistry::new();
        registry.register(id("c1")).await.unwrap();

        registry.evict_expired(Duration::from_millis(0)).await;
        assert_eq!(
            registry.active_clients().await.unwrap(),
            Vec::<ClientId>::new()
        );

        registry.register(id("c1")).await.unwrap();
        assert_eq!(registry.active_clients().await.unwrap(), vec![id("c1")]);
    }

    /// A heartbeat sent right before a sweep must actually postpone
    /// eviction, not just get recorded and ignored — otherwise a client
    /// heartbeating on schedule could still get evicted out from under it
    /// by a sweep that raced ahead of the liveness update reaching it.
    #[tokio::test]
    async fn heartbeat_resets_the_eviction_clock() {
        let registry = InMemoryRegistry::new();
        registry.register(id("c1")).await.unwrap();

        tokio::time::sleep(Duration::from_millis(30)).await;
        registry.heartbeat(&id("c1")).await.unwrap();

        // Without the heartbeat above, 30ms of age would already exceed
        // this 20ms TTL and the sweep below would evict it.
        registry.evict_expired(Duration::from_millis(20)).await;

        assert_eq!(registry.active_clients().await.unwrap(), vec![id("c1")]);
    }
}
