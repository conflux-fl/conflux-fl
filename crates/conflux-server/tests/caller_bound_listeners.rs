//! The caller binds the ports; the server serves them.
//!
//! The property an in-process federation needs from the server side. A
//! federation run in one process has to tell its nodes where the server
//! is, and it has to know that *before* the server starts — otherwise it
//! is picking a port by convention and racing whatever else on the
//! machine wants it. Binding `127.0.0.1:0` and reading back the OS's
//! choice is the only race-free way to learn a free port, and only
//! whoever bound the socket can read it back.
//!
//! So [`run_from_env_on`] takes the listeners. This test binds both, notes
//! both ports, hands them over, and then proves the running server is
//! reachable on exactly those two — over real gRPC and real HTTP, not by
//! asking the server what it thinks it bound.
//!
//! Deliberately the only test in this file: `serve` reads the process
//! environment, which is global to a test binary, and an integration
//! test file is its own binary.

use std::time::Duration;

use conflux_net::PullTransport;
use conflux_server::{ServerListeners, run_from_env_on};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// A bare HTTP/1.1 `GET`, so the test needs no HTTP client dependency to
/// prove the admin surface is really serving rather than merely bound.
async fn get(addr: std::net::SocketAddr, path: &str) -> std::io::Result<String> {
    let mut stream = tokio::net::TcpStream::connect(addr).await?;
    stream
        .write_all(
            format!("GET {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n").as_bytes(),
        )
        .await?;
    let mut body = String::new();
    stream.read_to_string(&mut body).await?;
    Ok(body)
}

#[tokio::test]
async fn a_server_serves_the_listeners_it_was_given() {
    // Bind first. These two ports are now known to the caller and to
    // nobody else — which is the whole point, and is not possible with
    // an API that takes an address.
    let grpc = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let http = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let grpc_addr = grpc.local_addr().unwrap();
    let http_addr = http.local_addr().unwrap();
    assert_ne!(grpc_addr.port(), 0, "the OS must have assigned a real port");
    assert_ne!(http_addr.port(), 0, "the OS must have assigned a real port");
    assert_ne!(grpc_addr.port(), http_addr.port());

    // `oneshot` rather than a plain future: the test has to be able to
    // stop the server it started, or the runtime outlives the test.
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
    let server = tokio::spawn(async move {
        run_from_env_on(ServerListeners { grpc, http }, async {
            let _ = stop_rx.await;
        })
        .await
    });

    // Poll rather than sleep: startup resolves config and connects
    // backends, and how long that takes is not this test's business.
    //
    // Each attempt is itself under a timeout, because the listener is
    // already bound: a connect succeeds whether or not anything is
    // accepting, so a server that never starts serving would leave the
    // read hanging rather than returning an error to retry on.
    let mut health = None;
    for _ in 0..100 {
        if let Ok(Ok(body)) =
            tokio::time::timeout(Duration::from_millis(500), get(http_addr, "/health")).await
        {
            health = Some(body);
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let health = health.expect("the HTTP listener we bound should be serving /health");
    assert!(
        health.starts_with("HTTP/1.1 200"),
        "expected 200 from /health, got: {health}"
    );

    // And the gRPC port, exercised the way a node exercises it — connect
    // and register — because a bound-but-unserved socket would accept a
    // TCP connection and prove nothing.
    let mut node = PullTransport::connect(format!("http://{grpc_addr}"))
        .await
        .expect("the gRPC listener we bound should accept a real client");
    let response = node
        .register("caller-bound-listener-test", "")
        .await
        .expect("register should reach the server");
    assert!(
        response.accepted,
        "the server should have accepted this registration: {}",
        response.message
    );

    let _ = stop_tx.send(());
    let outcome = tokio::time::timeout(Duration::from_secs(10), server)
        .await
        .expect("the server should shut down when told to")
        .expect("the server task should not panic");
    assert!(outcome.is_ok(), "server exited with an error: {outcome:?}");
}
