//! Runs a client through its whole lifecycle against `InMemoryRegistry`:
//! register, heartbeat, get evicted after its TTL lapses — plus the two
//! failure modes (`register`ing twice, `heartbeat`ing an id that was never
//! registered) and a `NodeAllowlist` check alongside it.
//!
//! Run with:
//! ```bash
//! cargo run --example lifecycle -p conflux-registry
//! ```

use std::time::Duration;

use conflux_registry::{
    ClientId, InMemoryNodeAllowlist, InMemoryRegistry, NodeAllowlist, NodeIdentity, Registry,
};

#[tokio::main]
async fn main() {
    let registry = InMemoryRegistry::new();
    let client = ClientId("client-a".to_string());

    registry.register(client.clone()).await.unwrap();
    println!("registered {client}");

    let err = registry.register(client.clone()).await.unwrap_err();
    println!("registering {client} again -> {err}");

    let ghost = ClientId("ghost".to_string());
    let err = registry.heartbeat(&ghost).await.unwrap_err();
    println!("heartbeat for never-registered {ghost} -> {err}");

    registry.heartbeat(&client).await.unwrap();
    println!("heartbeat for {client} -> ok");

    let active = registry.active_clients().await.unwrap();
    println!("active clients before TTL lapses: {active:?}");

    tokio::time::sleep(Duration::from_millis(50)).await;
    registry.evict_expired(Duration::from_millis(20)).await;

    let active = registry.active_clients().await.unwrap();
    println!("active clients after a 20ms TTL and a 50ms sleep: {active:?}");

    // Node auth: a client can be registered/heartbeating and still be
    // rejected at the transport layer if it isn't on the allow-list, or
    // presents the wrong credential.
    let allowlist = InMemoryNodeAllowlist::new();
    let token = NodeIdentity::SharedToken("s3cr3t".to_string());
    allowlist
        .allow(client.clone(), token.clone())
        .await
        .unwrap();

    let correct = allowlist.check(&client, &token).await.unwrap();
    println!("allowlist check for {client} with its real token -> {correct}");

    let wrong = NodeIdentity::SharedToken("guess".to_string());
    let incorrect = allowlist.check(&client, &wrong).await.unwrap();
    println!("allowlist check for {client} with a wrong token -> {incorrect}");
}
