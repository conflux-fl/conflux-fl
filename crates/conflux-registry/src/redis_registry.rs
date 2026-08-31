//! `RedisRegistry` — a `Registry` backend that's durable across restarts
//! and shared across processes, unlike `InMemoryRegistry`, whose state
//! lives only in that one process's memory.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use redis::aio::ConnectionManager;

use crate::{ClientId, Registry, RegistryError};

/// One Redis sorted set: member = client id, score = last-heartbeat time
/// (Unix millis). `evict_expired` is `ZREMRANGEBYSCORE`; `active_clients`
/// is `ZRANGE`. A sorted set rather than Redis's own per-key `EXPIRE`
/// because `evict_expired` takes its `ttl` as a call-time argument (the
/// `Registry` trait's design) — the TTL isn't known until then, so
/// there's no single per-key expiry to set at write time.
const DEFAULT_CLIENTS_KEY: &str = "conflux:registry:clients";

/// A `Registry` backed by a real Redis, so client lifecycle state
/// survives a restart and is shared by every server process.
pub struct RedisRegistry {
    conn: ConnectionManager,
    key: String,
}

impl RedisRegistry {
    /// `redis_url` is a plain `redis://host:port/db` string, passed straight
    /// through to the `redis` crate — it isn't routed through
    /// `conflux-config`'s layered resolution chain. `conflux-server` reads
    /// it directly from `CONFLUX_REDIS_URL` at startup instead, since a
    /// connection string like this doesn't need topology/mode layering, an
    /// explainable source, or a CLI override — just a value at boot.
    pub async fn connect(redis_url: &str) -> Result<Self, RegistryError> {
        Self::connect_with_key(redis_url, DEFAULT_CLIENTS_KEY).await
    }

    /// Lets multiple independent registries share one Redis under
    /// different key namespaces (e.g. per-environment) — this module's own
    /// tests use it to stay isolated from each other under `cargo test`'s
    /// parallel execution, rather than sharing one well-known key and
    /// racing on it.
    pub async fn connect_with_key(
        redis_url: &str,
        key: impl Into<String>,
    ) -> Result<Self, RegistryError> {
        let client =
            redis::Client::open(redis_url).map_err(|e| RegistryError::Backend(e.to_string()))?;
        let conn = client
            .get_connection_manager()
            .await
            .map_err(|e| RegistryError::Backend(e.to_string()))?;
        Ok(Self {
            conn,
            key: key.into(),
        })
    }
}

fn now_millis() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is before the Unix epoch")
        .as_millis() as f64
}

impl Registry for RedisRegistry {
    async fn register(&self, id: ClientId) -> Result<(), RegistryError> {
        let mut conn = self.conn.clone();
        // ZADD NX CH: only add if the member doesn't already exist;
        // CH makes it report whether anything actually changed.
        let changed: i64 = redis::cmd("ZADD")
            .arg(&self.key)
            .arg("NX")
            .arg("CH")
            .arg(now_millis())
            .arg(&id.0)
            .query_async(&mut conn)
            .await
            .map_err(|e| RegistryError::Backend(e.to_string()))?;
        if changed == 0 {
            return Err(RegistryError::AlreadyRegistered(id));
        }
        Ok(())
    }

    async fn heartbeat(&self, id: &ClientId) -> Result<(), RegistryError> {
        let mut conn = self.conn.clone();
        // ZADD XX CH: only update if the member already exists.
        let changed: i64 = redis::cmd("ZADD")
            .arg(&self.key)
            .arg("XX")
            .arg("CH")
            .arg(now_millis())
            .arg(&id.0)
            .query_async(&mut conn)
            .await
            .map_err(|e| RegistryError::Backend(e.to_string()))?;
        if changed == 0 {
            return Err(RegistryError::NotRegistered(id.clone()));
        }
        Ok(())
    }

    async fn evict_expired(&self, ttl: Duration) {
        let mut conn = self.conn.clone();
        let cutoff = now_millis() - ttl.as_millis() as f64;
        // Best-effort per the trait's contract (see its doc comment) — a
        // transient error here just means this sweep did less than it
        // could, and the next scheduled sweep tries again. It's still
        // worth logging rather than swallowing silently: a *persistent*
        // failure (not just one bad sweep) would otherwise be invisible
        // until stale clients visibly pile up in `active_clients`.
        let result: Result<i64, _> = redis::cmd("ZREMRANGEBYSCORE")
            .arg(&self.key)
            .arg("-inf")
            .arg(cutoff)
            .query_async(&mut conn)
            .await;
        if let Err(e) = result {
            tracing::warn!(error = %e, key = %self.key, "evict_expired sweep failed; will retry next sweep");
        }
    }

    async fn active_clients(&self) -> Result<Vec<ClientId>, RegistryError> {
        let mut conn = self.conn.clone();
        let members: Vec<String> = redis::cmd("ZRANGE")
            .arg(&self.key)
            .arg(0)
            .arg(-1)
            .query_async(&mut conn)
            .await
            .map_err(|e| RegistryError::Backend(e.to_string()))?;
        Ok(members.into_iter().map(ClientId).collect())
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
    use std::sync::atomic::{AtomicU64, Ordering};

    /// These tests need a real Redis reachable here; start one with
    /// `docker run -d --name conflux-dev-redis -p 16379:6379 redis:7-alpine`.
    fn test_redis_url() -> String {
        test_backend_url("CONFLUX_TEST_REDIS_URL", "redis://127.0.0.1:16379")
    }

    /// Every test gets its own Redis key so `cargo test`'s parallel
    /// execution against one real, shared Redis doesn't let tests race on
    /// the same sorted set (this actually happened the first time these
    /// tests ran: `duplicate_register_errors` saw a client another test
    /// had just written). The process id is part of the key too, not just
    /// a per-process counter — a counter alone restarts at 0 on every
    /// fresh `cargo test` invocation, so two separate runs against the
    /// same real (persistent, un-wiped) Redis could otherwise land on the
    /// same key and collide with a previous run's leftover data — which
    /// is exactly what happened the second time these tests ran.
    fn unique_key(test_name: &str) -> String {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        format!(
            "conflux:registry:test:{test_name}:{}:{n}",
            std::process::id()
        )
    }

    async fn fresh_registry(test_name: &str) -> RedisRegistry {
        let registry = RedisRegistry::connect_with_key(&test_redis_url(), unique_key(test_name))
            .await
            .expect("connect to the dev Redis container — is it running?");
        // Belt-and-suspenders: the key is already unique, but clear it
        // anyway so a test's correctness never depends on nothing else
        // having ever touched this exact key.
        let mut conn = registry.conn.clone();
        let _: Result<i64, _> = redis::cmd("DEL")
            .arg(&registry.key)
            .query_async(&mut conn)
            .await;
        registry
    }

    fn id(s: &str) -> ClientId {
        ClientId(s.to_string())
    }

    #[tokio::test]
    async fn register_then_appears_in_active_clients() {
        let registry = fresh_registry("register_then_appears").await;

        registry.register(id("c1")).await.unwrap();

        assert_eq!(registry.active_clients().await.unwrap(), vec![id("c1")]);
    }

    #[tokio::test]
    async fn duplicate_register_errors() {
        let registry = fresh_registry("duplicate_register").await;
        registry.register(id("c1")).await.unwrap();

        let err = registry.register(id("c1")).await.unwrap_err();

        assert!(matches!(err, RegistryError::AlreadyRegistered(client) if client == id("c1")));
    }

    #[tokio::test]
    async fn heartbeat_unknown_client_errors() {
        let registry = fresh_registry("heartbeat_unknown").await;

        let err = registry.heartbeat(&id("ghost")).await.unwrap_err();

        assert!(matches!(err, RegistryError::NotRegistered(client) if client == id("ghost")));
    }

    #[tokio::test]
    async fn evict_expired_removes_stale_clients() {
        let registry = fresh_registry("evict_expired").await;
        registry.register(id("stale")).await.unwrap();

        tokio::time::sleep(Duration::from_millis(250)).await;
        registry.register(id("fresh")).await.unwrap();

        registry.evict_expired(Duration::from_millis(200)).await;

        assert_eq!(registry.active_clients().await.unwrap(), vec![id("fresh")]);
    }

    #[tokio::test]
    async fn two_handles_to_the_same_redis_see_each_others_writes() {
        let key = unique_key("two_handles");
        let a = RedisRegistry::connect_with_key(&test_redis_url(), key.clone())
            .await
            .unwrap();
        let b = RedisRegistry::connect_with_key(&test_redis_url(), key)
            .await
            .unwrap();

        // The actual reason this backend exists — InMemoryRegistry is
        // structurally incapable of this.
        a.register(id("shared")).await.unwrap();

        assert_eq!(b.active_clients().await.unwrap(), vec![id("shared")]);
    }
}
