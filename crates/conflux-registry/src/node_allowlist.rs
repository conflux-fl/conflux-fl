//! Node authentication: a check of *which* client is allowed to join, on
//! top of whatever transport-level handshake already got it connected —
//! a client that can open a connection isn't necessarily one the operator
//! wants participating in an experiment.
//!
//! Gated by `conflux-config`'s `require_node_auth` (research default
//! `false`, production default `true`). This module builds the data model
//! and storage backends (`NodeAllowlist`, `InMemoryNodeAllowlist`,
//! `RedisNodeAllowlist`); `conflux-server` is what actually enforces it —
//! `dispatcher.rs`'s `register()` checks the allow-list before touching
//! the registry at all when `require_node_auth` is on, and `http.rs`
//! exposes an admin endpoint (`allow_node`) an operator uses to populate
//! it.

use std::collections::HashMap;
use std::future::Future;
use std::sync::Mutex;

use crate::ClientId;

/// Proof of identity a node presents at registration. Two independent
/// mechanisms because node auth shouldn't force mTLS to also be turned on
/// for a given deployment — a `SharedToken` deployment gets real node auth
/// without a certificate authority to run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NodeIdentity {
    /// SHA-256 hex digest of the DER-encoded peer certificate, when mTLS
    /// is in use. `conflux-net`'s TLS layer extracts this from the actual
    /// peer certificate on the connection (see
    /// `conflux-net::peer_cert_fingerprint`); `conflux-server` only falls
    /// back to `SharedToken` when no peer certificate is present.
    CertFingerprint(String),
    /// A pre-shared secret, when mTLS isn't in use.
    SharedToken(String),
}

impl NodeIdentity {
    /// Whether `presented` is the same identity as `self`, compared in
    /// constant time.
    ///
    /// Not `==`: the derived `PartialEq` stops at the first differing
    /// byte, so how long a comparison takes leaks how much of a
    /// credential an attacker guessed right. A shared token is a secret,
    /// and the allow-list check is the one place it is compared against
    /// untrusted input, so this is where the comparison has to be
    /// length-independent. The variant kind and the length may still
    /// leak — both are already known to the peer.
    pub fn matches(&self, presented: &NodeIdentity) -> bool {
        let (a, b) = match (self, presented) {
            (Self::CertFingerprint(a), Self::CertFingerprint(b)) => (a, b),
            (Self::SharedToken(a), Self::SharedToken(b)) => (a, b),
            _ => return false,
        };
        constant_time_eq(a.as_bytes(), b.as_bytes())
    }
}

/// Byte equality that touches every byte regardless of where the first
/// mismatch is. `black_box` keeps the optimizer from turning the loop back
/// into an early-exit compare.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff: u8 = 0;
    for (x, y) in a.iter().zip(b) {
        diff |= std::hint::black_box(x ^ y);
    }
    diff == 0
}

/// Every error a `NodeAllowlist` implementation can return.
#[derive(Debug, thiserror::Error)]
pub enum NodeAuthError {
    #[error("client {0} is not on the node allow-list")]
    /// The client is not on the allow-list, or presented an identity that
    /// doesn't match the one it was allowed under.
    NotAllowed(ClientId),
    /// Mirrors `RegistryError::Backend` — `InMemoryNodeAllowlist` never
    /// needs this, `RedisNodeAllowlist` does since it does real I/O.
    #[error("node allow-list backend error: {0}")]
    Backend(String),
}

/// Who's allowed to register, and with which identity.
///
/// A trait, not a concrete struct, for the same reason `Registry` (in this
/// crate) and `Store` (in `conflux-store`) are: `conflux-server` depends on
/// allow-list behavior without committing to a storage backend —
/// `InMemoryNodeAllowlist` here, `RedisNodeAllowlist` for a durable,
/// multi-process deployment.
///
/// `Send + Sync` (shared across concurrently handled `register()` calls in
/// `conflux-server`'s dispatcher) and native `async fn` (a real backend
/// does I/O) — same reasoning as `Registry`'s own doc comment.
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
    /// failing to answer the question at all. Implementations compare
    /// with [`NodeIdentity::matches`], never `==`, so the answer takes the
    /// same time whether the credential was close or nowhere near.
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
    /// An empty allow-list — nothing is permitted until something is added.
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
        Ok(entries
            .get(id)
            .is_some_and(|stored| stored.matches(presented)))
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

    /// A credential that's genuinely valid — just for a *different*
    /// client — must still be rejected. The whole point of node auth is
    /// binding an identity to one specific client id; a check that only
    /// asked "does this token exist anywhere on the allow-list" would let
    /// one compromised or leaked node credential impersonate any other
    /// allowed client.
    #[tokio::test]
    async fn credential_valid_for_one_client_is_rejected_for_another() {
        let allowlist = InMemoryNodeAllowlist::new();
        let token_for_c1 = NodeIdentity::SharedToken("c1-secret".to_string());
        let token_for_c2 = NodeIdentity::SharedToken("c2-secret".to_string());
        allowlist
            .allow(id("c1"), token_for_c1.clone())
            .await
            .unwrap();
        allowlist
            .allow(id("c2"), token_for_c2.clone())
            .await
            .unwrap();

        // c1's real token, presented while claiming to be c2, must fail —
        // even though that exact token is valid for someone.
        assert!(!allowlist.check(&id("c2"), &token_for_c1).await.unwrap());
        // Each client's own token still works for itself.
        assert!(allowlist.check(&id("c1"), &token_for_c1).await.unwrap());
        assert!(allowlist.check(&id("c2"), &token_for_c2).await.unwrap());
    }

    /// `matches` must agree with `==` on every combination — it exists to
    /// change *how long* the comparison takes, never what it answers.
    #[test]
    fn matches_agrees_with_equality() {
        let token = NodeIdentity::SharedToken("abc".to_string());
        let other_token = NodeIdentity::SharedToken("abd".to_string());
        let longer_token = NodeIdentity::SharedToken("abcd".to_string());
        let cert = NodeIdentity::CertFingerprint("abc".to_string());

        assert!(token.matches(&token.clone()));
        assert!(!token.matches(&other_token));
        assert!(!token.matches(&longer_token));
        assert!(!token.matches(&cert));
        assert!(cert.matches(&cert.clone()));
        assert!(!cert.matches(&token));
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
