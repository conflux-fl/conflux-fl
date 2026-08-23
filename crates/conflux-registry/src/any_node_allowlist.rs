//! `AnyNodeAllowlist` — picks between `NodeAllowlist` backends at runtime,
//! same delegation pattern as `AnyRegistry`/`AnyStore` (Phase 8a) and for
//! the same reason: `NodeAllowlist`'s methods are native `async fn` in a
//! trait, not dyn-compatible without extra boxing.

use crate::{
    ClientId, InMemoryNodeAllowlist, NodeAllowlist, NodeAuthError, NodeIdentity, RedisNodeAllowlist,
};

pub enum AnyNodeAllowlist {
    InMemory(InMemoryNodeAllowlist),
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
        let backend = RedisNodeAllowlist::connect_with_key("redis://127.0.0.1:16379", key)
            .await
            .expect("connect to the dev Redis container — is it running?");
        let allowlist = AnyNodeAllowlist::Redis(backend);
        let identity = NodeIdentity::SharedToken("secret".to_string());

        allowlist.allow(id("c1"), identity.clone()).await.unwrap();

        assert!(allowlist.check(&id("c1"), &identity).await.unwrap());
        assert_eq!(allowlist.list().await.unwrap(), vec![id("c1")]);
    }
}
