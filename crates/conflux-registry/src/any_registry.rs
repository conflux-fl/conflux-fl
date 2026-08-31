//! `AnyRegistry` — picks between `Registry` backends at runtime.
//!
//! An enum, not `Arc<dyn Registry>`: `Registry`'s methods are native
//! `async fn` in a trait, which isn't dyn-compatible without extra boxing
//! machinery. For a small, closed set of backends, a `match`-delegating
//! enum gets the same "pick one at runtime" result without that cost —
//! `conflux-server` reads `CONFLUX_REGISTRY_BACKEND` at startup and
//! constructs the matching variant once.

use std::time::Duration;

use crate::{ClientId, InMemoryRegistry, RedisRegistry, Registry, RegistryError};

pub enum AnyRegistry {
    InMemory(InMemoryRegistry),
    Redis(RedisRegistry),
}

impl Registry for AnyRegistry {
    async fn register(&self, id: ClientId) -> Result<(), RegistryError> {
        match self {
            Self::InMemory(r) => r.register(id).await,
            Self::Redis(r) => r.register(id).await,
        }
    }

    async fn heartbeat(&self, id: &ClientId) -> Result<(), RegistryError> {
        match self {
            Self::InMemory(r) => r.heartbeat(id).await,
            Self::Redis(r) => r.heartbeat(id).await,
        }
    }

    async fn evict_expired(&self, ttl: Duration) {
        match self {
            Self::InMemory(r) => r.evict_expired(ttl).await,
            Self::Redis(r) => r.evict_expired(ttl).await,
        }
    }

    async fn active_clients(&self) -> Result<Vec<ClientId>, RegistryError> {
        match self {
            Self::InMemory(r) => r.active_clients().await,
            Self::Redis(r) => r.active_clients().await,
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

    fn id(s: &str) -> ClientId {
        ClientId(s.to_string())
    }

    #[tokio::test]
    async fn in_memory_variant_delegates_correctly() {
        let registry = AnyRegistry::InMemory(InMemoryRegistry::new());

        registry.register(id("c1")).await.unwrap();

        assert_eq!(registry.active_clients().await.unwrap(), vec![id("c1")]);
    }

    /// `docker run -d --name conflux-dev-redis -p 16379:6379 redis:7-alpine`.
    /// The key includes the process id — a bare literal collided with
    /// leftover data from a previous `cargo test` run the first time this
    /// test was written; see `redis_registry.rs`'s own tests for the same
    /// fix, applied here from the start this time.
    #[tokio::test]
    async fn redis_variant_delegates_correctly() {
        let key = format!(
            "conflux:registry:test:any_registry_redis:{}",
            std::process::id()
        );
        let backend = RedisRegistry::connect_with_key(
            &test_backend_url("CONFLUX_TEST_REDIS_URL", "redis://127.0.0.1:16379"),
            key,
        )
        .await
        .expect("connect to the dev Redis container — is it running?");
        let registry = AnyRegistry::Redis(backend);

        registry.register(id("c1")).await.unwrap();

        assert_eq!(registry.active_clients().await.unwrap(), vec![id("c1")]);
    }
}
