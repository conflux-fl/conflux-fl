//! `AnyNodeAllowlist` — picks between `NodeAllowlist` backends at runtime,
//! same delegation pattern as `AnyRegistry` (and `conflux-store`'s
//! `AnyStore`) and for the same reason: `NodeAllowlist`'s methods are
//! native `async fn` in a trait, not dyn-compatible without extra boxing.

use crate::{
    ClientId, InMemoryNodeAllowlist, NodeAllowlist, NodeAuthError, NodeIdentity, RedisNodeAllowlist,
};

/// Whichever allow-list backend this deployment selected at startup.
///
/// An enum rather than `Box<dyn NodeAllowlist>` because the trait's
/// methods are `async fn` in native syntax, which is not object-safe.
pub enum AnyNodeAllowlist {
    /// Process-local. Lost on restart.
    InMemory(InMemoryNodeAllowlist),
    /// Shared and durable across restarts and processes.
    Redis(RedisNodeAllowlist),
}

impl NodeAllowlist for AnyNodeAllowlist {
    async fn allow(&self, id: ClientId, identity: NodeIdentity) -> Result<(), NodeAuthError> {
        match self {
            Self::InMemory(a) => a.allow(id, identity).await,
            Self::Redis(a) => a.allow(id, identity).await,
        }
    }

    async fn revoke(&self, id: &ClientId) -> Result<(), NodeAuthError> {
        match self {
            Self::InMemory(a) => a.revoke(id).await,
            Self::Redis(a) => a.revoke(id).await,
        }
    }

    async fn check(&self, id: &ClientId, presented: &NodeIdentity) -> Result<bool, NodeAuthError> {
        match self {
            Self::InMemory(a) => a.check(id, presented).await,
            Self::Redis(a) => a.check(id, presented).await,
        }
    }

    async fn list(&self) -> Result<Vec<ClientId>, NodeAuthError> {
        match self {
            Self::InMemory(a) => a.list().await,
            Self::Redis(a) => a.list().await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(s: &str) -> ClientId {
        ClientId(s.to_string())
    }

    #[tokio::test]
    async fn in_memory_variant_delegates_correctly() {
        let allowlist = AnyNodeAllowlist::InMemory(InMemoryNodeAllowlist::new());
        let identity = NodeIdentity::SharedToken("secret".to_string());

        allowlist.allow(id("c1"), identity.clone()).await.unwrap();

        assert!(allowlist.check(&id("c1"), &identity).await.unwrap());
        assert_eq!(allowlist.list().await.unwrap(), vec![id("c1")]);
    }

    /// `docker run -d --name conflux-dev-redis -p 16379:6379 redis:7-alpine`
    #[tokio::test]
    async fn redis_variant_delegates_correctly() {
        let key = format!(
            "conflux:node_allowlist:test:any_node_allowlist_redis:{}",
            std::process::id()
        );
        // Read the Redis URL from env like the sibling redis tests do —
        // CI points it at its service container (`:6379`); the fallback is
        // the local dev container's port (`:16379`).
        let url = std::env::var("CONFLUX_TEST_REDIS_URL")
            .unwrap_or_else(|_| "redis://127.0.0.1:16379".to_string());
        let backend = RedisNodeAllowlist::connect_with_key(&url, key)
            .await
            .expect("connect to the dev Redis container — is it running?");
        let allowlist = AnyNodeAllowlist::Redis(backend);
        let identity = NodeIdentity::SharedToken("secret".to_string());

        allowlist.allow(id("c1"), identity.clone()).await.unwrap();

        assert!(allowlist.check(&id("c1"), &identity).await.unwrap());
        assert_eq!(allowlist.list().await.unwrap(), vec![id("c1")]);
    }
}
