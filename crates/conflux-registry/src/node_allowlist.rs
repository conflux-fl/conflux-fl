//! Node authentication core (Phase 8b) — closes gap 2 from
//! `docs/FLOWER_COMPARISON.md`: registration previously had no check of
//! *which* client is allowed to join, only whether it could complete a
//! handshake at all.
//!
//! Gated by `conflux-config`'s `require_node_auth` (research default
//! `false`, production default `true`) — this module builds the data
//! model and storage backends; enforcement at the `register()` RPC and
//! the admin surface to populate the allow-list are Phase 8c.

use std::collections::HashMap;
use std::future::Future;
use std::sync::Mutex;

use crate::ClientId;

/// Proof of identity a node presents at registration. Two independent
/// mechanisms because node auth shouldn't force mTLS (Phase 7e) to also be
/// turned on for a given deployment — a `SharedToken` deployment gets real
/// node auth without a certificate authority to run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NodeIdentity {
    /// SHA-256 hex digest of the DER-encoded peer certificate, when mTLS
    /// is in use (Phase 8c wires the actual extraction from
    /// `conflux-net`'s TLS layer).
    CertFingerprint(String),
    /// A pre-shared secret, when mTLS isn't in use.
    SharedToken(String),
}

/// Every error a `NodeAllowlist` implementation can return.
#[derive(Debug, thiserror::Error)]
pub enum NodeAuthError {
    #[error("client {0} is not on the node allow-list")]
    NotAllowed(ClientId),
    /// Mirrors `RegistryError::Backend` (Phase 7a) — `InMemoryNodeAllowlist`
    /// never needs this, `RedisNodeAllowlist` does since it does real I/O.
    #[error("node allow-list backend error: {0}")]
    Backend(String),
}

/// Who's allowed to register, and with which identity.
///
/// A trait, not a concrete struct, for the same reason `Registry` (Phase 1)
/// and `Store` (Phase 2a) are: `conflux-server` depends on allow-list
/// behavior without committing to a storage backend —
/// `InMemoryNodeAllowlist` here, `RedisNodeAllowlist` for a durable,
/// multi-process deployment.
///
/// `Send + Sync` (shared across concurrently handled `register()` calls,
/// Phase 8c) and native `async fn` (a real backend does I/O) — same
/// reasoning as `Registry`'s own doc comment.
pub trait NodeAllowlist: Send + Sync {
    /// Grants `id` access, remembering `identity` as the proof that must
    /// be presented later. Re-`allow`ing an already-allowed `id` replaces
    /// its stored identity rather than erroring — rotating a node's
    /// credential shouldn't require a separate "update" method.
    fn allow(
        &self,
        id: ClientId,
        identity: NodeIdentity,
    ) -> impl Future<Output = Result<(), NodeAuthError>> + Send;
    /// Removes `id` from the allow-list. Revoking an `id` that was never
    /// allowed is not an error — the end state ("not allowed") is what the
    /// caller wanted either way.
    fn revoke(&self, id: &ClientId) -> impl Future<Output = Result<(), NodeAuthError>> + Send;
    /// `Ok(true)` only when `id` is allowed *and* `presented` matches the
    /// identity it was allowed with — a valid-but-wrong credential for a
    /// real `id`, or any credential for an unknown `id`, both resolve to
    /// `Ok(false)`, not an error. `Err` is reserved for the backend itself
    /// failing to answer the question at all.
    fn check(
        &self,
        id: &ClientId,
        presented: &NodeIdentity,
    ) -> impl Future<Output = Result<bool, NodeAuthError>> + Send;
    /// Every currently-allowed client id.
    fn list(&self) -> impl Future<Output = Result<Vec<ClientId>, NodeAuthError>> + Send;
}

/// Research-default backend — ephemeral is fine, since `require_node_auth`
/// defaults off in research anyway (nothing to lose on restart if the
/// allow-list is rarely even populated).
pub struct InMemoryNodeAllowlist {
    entries: Mutex<HashMap<ClientId, NodeIdentity>>,
}

impl InMemoryNodeAllowlist {
    pub fn new() -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
        }
    }
}

impl Default for InMemoryNodeAllowlist {
    fn default() -> Self {
        Self::new()
    }
}

impl NodeAllowlist for InMemoryNodeAllowlist {
    async fn allow(&self, id: ClientId, identity: NodeIdentity) -> Result<(), NodeAuthError> {
        let mut entries = self.entries.lock().expect("allow-list mutex poisoned");
        entries.insert(id, identity);
        Ok(())
    }

    async fn revoke(&self, id: &ClientId) -> Result<(), NodeAuthError> {
        let mut entries = self.entries.lock().expect("allow-list mutex poisoned");
        entries.remove(id);
        Ok(())
    }

    async fn check(&self, id: &ClientId, presented: &NodeIdentity) -> Result<bool, NodeAuthError> {
        let entries = self.entries.lock().expect("allow-list mutex poisoned");
        Ok(entries.get(id) == Some(presented))
    }

    async fn list(&self) -> Result<Vec<ClientId>, NodeAuthError> {
        let entries = self.entries.lock().expect("allow-list mutex poisoned");
        Ok(entries.keys().cloned().collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(s: &str) -> ClientId {
        ClientId(s.to_string())
    }

    #[tokio::test]
    async fn allow_then_check_with_matching_identity_passes() {
        let allowlist = InMemoryNodeAllowlist::new();
        let identity = NodeIdentity::SharedToken("secret-1".to_string());
        allowlist.allow(id("c1"), identity.clone()).await.unwrap();

        assert!(allowlist.check(&id("c1"), &identity).await.unwrap());
    }

    #[tokio::test]
    async fn check_with_different_identity_for_same_client_fails() {
        let allowlist = InMemoryNodeAllowlist::new();
        allowlist
            .allow(id("c1"), NodeIdentity::SharedToken("secret-1".to_string()))
            .await
            .unwrap();

        let wrong = NodeIdentity::SharedToken("secret-2".to_string());
        assert!(!allowlist.check(&id("c1"), &wrong).await.unwrap());
    }

    #[tokio::test]
    async fn check_for_never_allowed_client_fails() {
        let allowlist = InMemoryNodeAllowlist::new();
        let identity = NodeIdentity::SharedToken("secret-1".to_string());

        assert!(!allowlist.check(&id("ghost"), &identity).await.unwrap());
    }

    #[tokio::test]
    async fn revoke_then_check_fails_even_with_originally_correct_identity() {
        let allowlist = InMemoryNodeAllowlist::new();
        let identity = NodeIdentity::SharedToken("secret-1".to_string());
        allowlist.allow(id("c1"), identity.clone()).await.unwrap();

        allowlist.revoke(&id("c1")).await.unwrap();

        assert!(!allowlist.check(&id("c1"), &identity).await.unwrap());
    }

    #[tokio::test]
    async fn list_reflects_current_membership() {
        let allowlist = InMemoryNodeAllowlist::new();
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
    async fn cert_fingerprint_and_shared_token_are_distinct_identities() {
        let allowlist = InMemoryNodeAllowlist::new();
        allowlist
            .allow(
                id("c1"),
                NodeIdentity::CertFingerprint("aabbcc".to_string()),
            )
            .await
            .unwrap();

        let wrong_kind = NodeIdentity::SharedToken("aabbcc".to_string());
        assert!(!allowlist.check(&id("c1"), &wrong_kind).await.unwrap());
    }
}
