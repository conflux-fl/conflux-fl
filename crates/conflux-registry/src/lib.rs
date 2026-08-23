//! Client lifecycle (register/heartbeat/evict).
//!
//! See `docs/spec/conflux-spec-v1.md` §8.

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
    AlreadyRegistered(ClientId),
    #[error("client {0} is not registered")]
    NotRegistered(ClientId),
    /// Phase 1's `InMemoryRegistry` never needed this — an in-process
    /// `HashMap` doesn't fail. `RedisRegistry` (Phase 7) is the first
    /// backend to actually do I/O, so the trait needed a variant for "the
    /// backend itself is unreachable/erroring," distinct from the two
    /// business-logic errors above.
    #[error("registry backend error: {0}")]
    Backend(String),
}

#[derive(Debug, Clone)]
pub struct ClientInfo {
    pub id: ClientId,
    pub registered_at: Instant,
    pub last_heartbeat: Instant,
}

/// Who's known to the server right now, and are they still alive.
///
/// A `trait` (rather than a concrete struct) so `conflux-server` can depend
/// on client-lifecycle behavior without committing to a storage backend:
/// `InMemoryRegistry` here, `RedisRegistry` in Phase 7
/// (`docs/spec/conflux-spec-v1.md` §10) — same interface, different
/// durability/scaling tradeoffs.
///
/// `Send + Sync` because `conflux-server` shares one registry across
/// concurrently handled requests (Phase 5) — every implementation must be
/// safe to call from multiple threads/tasks at once.
///
/// Methods are `async fn` (stable native syntax, no `async-trait` needed)
/// because a real backend does network I/O — `InMemoryRegistry`'s bodies
/// have no `.await` in them at all, but `RedisRegistry`'s do. Native async
/// fn in traits isn't automatically object-safe, but nothing in this
/// codebase needs `dyn Registry` (every caller holds a concrete type), so
/// that's not a cost paid for no reason.
pub trait Registry: Send + Sync {
    fn register(&self, id: ClientId) -> impl Future<Output = Result<(), RegistryError>> + Send;
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

/// The only `Registry` this phase ships — a mutex-guarded in-memory map.
/// `RedisRegistry` (durable across restarts, shared across processes) is
/// Phase 7.
///
/// `Mutex` rather than `RwLock`: every operation here, including
/// `active_clients`, is a quick map scan/mutation — there's no read-heavy
/// workload yet that would justify separate read/write locking.
pub struct InMemoryRegistry {
    clients: Mutex<HashMap<ClientId, ClientInfo>>,
}

impl InMemoryRegistry {
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
}
