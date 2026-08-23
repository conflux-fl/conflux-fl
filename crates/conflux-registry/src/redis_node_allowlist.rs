//! `RedisNodeAllowlist` — a `NodeAllowlist` backend that's durable across
//! restarts and shared across processes, the production default for Phase
//! 8b's `require_node_auth` (mirrors `RedisRegistry`'s Phase 7a role for
//! `Registry`).

use crate::{ClientId, NodeAllowlist, NodeAuthError, NodeIdentity};
use redis::aio::ConnectionManager;

/// One Redis hash: field = client id, value = a serialized `NodeIdentity`.
/// A hash, not per-client keys, so `list` is a single `HKEYS` rather than a
/// `SCAN` over a key pattern.
const DEFAULT_ALLOWLIST_KEY: &str = "conflux:node_allowlist:entries";

pub struct RedisNodeAllowlist {
    conn: ConnectionManager,
    key: String,
}

impl RedisNodeAllowlist {
    /// `redis_url` stays argument-based rather than `conflux-config`-driven
    /// — same precedent as `RedisRegistry::connect` (spec §11 Open Item 2
    /// is still open).
    pub async fn connect(redis_url: &str) -> Result<Self, NodeAuthError> {
        Self::connect_with_key(redis_url, DEFAULT_ALLOWLIST_KEY).await
    }

    /// Lets multiple independent allow-lists share one Redis under
    /// different key namespaces — used by this module's own tests for
    /// per-test isolation, same reason `RedisRegistry::connect_with_key`
    /// exists.
    pub async fn connect_with_key(
        redis_url: &str,
        key: impl Into<String>,
    ) -> Result<Self, NodeAuthError> {
        let client =
            redis::Client::open(redis_url).map_err(|e| NodeAuthError::Backend(e.to_string()))?;
        let conn = client
            .get_connection_manager()
            .await
            .map_err(|e| NodeAuthError::Backend(e.to_string()))?;
        Ok(Self {
            conn,
            key: key.into(),
        })
    }
}

/// `NodeIdentity` <-> a single string, so it fits in one Redis hash field.
/// A tagged prefix rather than e.g. JSON — the value is always exactly one
/// of two shapes, so a full serialization format would be more machinery
/// than the data needs.
fn encode(identity: &NodeIdentity) -> String {
    match identity {
        NodeIdentity::CertFingerprint(fp) => format!("cert:{fp}"),
        NodeIdentity::SharedToken(tok) => format!("token:{tok}"),
    }
}

fn decode(raw: &str) -> Option<NodeIdentity> {
    if let Some(fp) = raw.strip_prefix("cert:") {
        Some(NodeIdentity::CertFingerprint(fp.to_string()))
    } else {
        raw.strip_prefix("token:")
            .map(|tok| NodeIdentity::SharedToken(tok.to_string()))
    }
}

impl NodeAllowlist for RedisNodeAllowlist {
    async fn allow(&self, id: ClientId, identity: NodeIdentity) -> Result<(), NodeAuthError> {
        let mut conn = self.conn.clone();
        redis::cmd("HSET")
            .arg(&self.key)
            .arg(&id.0)
            .arg(encode(&identity))
            .query_async::<i64>(&mut conn)
            .await
            .map_err(|e| NodeAuthError::Backend(e.to_string()))?;
        Ok(())
    }

    async fn revoke(&self, id: &ClientId) -> Result<(), NodeAuthError> {
        let mut conn = self.conn.clone();
        redis::cmd("HDEL")
            .arg(&self.key)
            .arg(&id.0)
            .query_async::<i64>(&mut conn)
            .await
            .map_err(|e| NodeAuthError::Backend(e.to_string()))?;
        Ok(())
    }

    async fn check(&self, id: &ClientId, presented: &NodeIdentity) -> Result<bool, NodeAuthError> {
        let mut conn = self.conn.clone();
        let stored: Option<String> = redis::cmd("HGET")
            .arg(&self.key)
            .arg(&id.0)
            .query_async(&mut conn)
            .await
            .map_err(|e| NodeAuthError::Backend(e.to_string()))?;
        Ok(stored.as_deref().and_then(decode).as_ref() == Some(presented))
    }

    async fn list(&self) -> Result<Vec<ClientId>, NodeAuthError> {
        let mut conn = self.conn.clone();
        let members: Vec<String> = redis::cmd("HKEYS")
            .arg(&self.key)
            .query_async(&mut conn)
            .await
            .map_err(|e| NodeAuthError::Backend(e.to_string()))?;
        Ok(members.into_iter().map(ClientId).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// `docker run -d --name conflux-dev-redis -p 16379:6379 redis:7-alpine`
    const TEST_REDIS_URL: &str = "redis://127.0.0.1:16379";

    /// Same per-test-isolation shape as `redis_registry.rs`'s
    /// `unique_key` — process id *and* a counter, since a counter alone
    /// restarts at 0 every fresh `cargo test` invocation and would collide
    /// with a previous run's leftover data on the same real, never-wiped
    /// Redis.
    fn unique_key(test_name: &str) -> String {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        format!(
            "conflux:node_allowlist:test:{test_name}:{}:{n}",
            std::process::id()
        )
    }

    async fn fresh_allowlist(test_name: &str) -> RedisNodeAllowlist {
        RedisNodeAllowlist::connect_with_key(TEST_REDIS_URL, unique_key(test_name))
            .await
            .expect("connect to the dev Redis container — is it running?")
    }

    fn id(s: &str) -> ClientId {
        ClientId(s.to_string())
    }

    #[tokio::test]
    async fn allow_then_check_with_matching_identity_passes() {
        let allowlist = fresh_allowlist("allow_then_check").await;
        let identity = NodeIdentity::SharedToken("secret-1".to_string());
        allowlist.allow(id("c1"), identity.clone()).await.unwrap();

        assert!(allowlist.check(&id("c1"), &identity).await.unwrap());
    }

    #[tokio::test]
    async fn check_with_different_identity_for_same_client_fails() {
        let allowlist = fresh_allowlist("different_identity").await;
        allowlist
            .allow(id("c1"), NodeIdentity::SharedToken("secret-1".to_string()))
            .await
            .unwrap();

        let wrong = NodeIdentity::SharedToken("secret-2".to_string());
        assert!(!allowlist.check(&id("c1"), &wrong).await.unwrap());
    }

    #[tokio::test]
    async fn check_for_never_allowed_client_fails() {
        let allowlist = fresh_allowlist("never_allowed").await;
        let identity = NodeIdentity::SharedToken("secret-1".to_string());

        assert!(!allowlist.check(&id("ghost"), &identity).await.unwrap());
    }

    #[tokio::test]
    async fn revoke_then_check_fails_even_with_originally_correct_identity() {
        let allowlist = fresh_allowlist("revoke_then_check").await;
        let identity = NodeIdentity::SharedToken("secret-1".to_string());
        allowlist.allow(id("c1"), identity.clone()).await.unwrap();

        allowlist.revoke(&id("c1")).await.unwrap();

        assert!(!allowlist.check(&id("c1"), &identity).await.unwrap());
    }

    #[tokio::test]
    async fn list_reflects_current_membership() {
        let allowlist = fresh_allowlist("list_membership").await;
        allowlist
            .allow(id("c1"), NodeIdentity::SharedToken("t1".to_string()))
            .await
            .unwrap();
        allowlist
            .allow(id("c2"), NodeIdentity::SharedToken("t2".to_string()))
            .await
            .unwrap();
        allowlist.revoke(&id("c1")).await.unwrap();

        assert_eq!(allowlist.list().await.unwrap(), vec![id("c2")]);
    }

    #[tokio::test]
    async fn cert_fingerprint_round_trips_through_redis() {
        let allowlist = fresh_allowlist("cert_fingerprint_round_trip").await;
        let identity = NodeIdentity::CertFingerprint("aa:bb:cc".to_string());
        allowlist.allow(id("c1"), identity.clone()).await.unwrap();

        assert!(allowlist.check(&id("c1"), &identity).await.unwrap());
    }

    #[tokio::test]
    async fn two_handles_to_the_same_redis_see_each_others_writes() {
        let key = unique_key("two_handles");
        let a = RedisNodeAllowlist::connect_with_key(TEST_REDIS_URL, key.clone())
            .await
            .unwrap();
        let b = RedisNodeAllowlist::connect_with_key(TEST_REDIS_URL, key)
            .await
            .unwrap();

        let identity = NodeIdentity::SharedToken("shared".to_string());
        a.allow(id("shared"), identity.clone()).await.unwrap();

        assert!(b.check(&id("shared"), &identity).await.unwrap());
    }
}
