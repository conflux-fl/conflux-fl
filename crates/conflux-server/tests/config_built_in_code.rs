//! A server configured in code, and the grid of experiments that needs it.
//!
//! The environment is process-global, so it describes one server. That is
//! exactly right for a deployment — one process is one experiment (ADR
//! 0003) — and wrong for a tool that runs a *grid* of experiments back to
//! back in one process, which is what a baseline sweep is: twelve
//! aggregators, each with its own quorum and model dimension. Mutating
//! the environment between them is unsound once a runtime has threads,
//! so the configuration has to be a value.
//!
//! Note what this is *not*: the two servers below run one at a time, each
//! to completion, each with its own `AppState`. ADR 0003 forbids
//! per-tenant indirection inside one server, and nothing here adds any.
//!
//! Deliberately the only tests in this file, sharing one binary: `serve`
//! still reads deployment material — backends, TLS, the admin token —
//! from the process environment, which is global to a test binary.

use std::time::Duration;

use conflux_config::{Mode, Overrides, Topology};
use conflux_server::{ServeError, ServerConfig, ServerListeners, run_on};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// Resolves a configuration the way a sweep runner would: in code, one
/// combination at a time, with no environment involved.
fn config_for(aggregator: &str) -> ServerConfig {
    let overrides = Overrides {
        aggregator: Some(aggregator.to_string()),
        ..Overrides::default()
    };
    ServerConfig {
        resolved: conflux_config::resolve(
            Topology::CrossDevice,
            Mode::Research,
            None,
            &overrides,
            &Overrides::default(),
        )
        .expect("this combination should resolve"),
        mode: Mode::Research,
        initial_weights_dim: 4,
    }
}

/// A bare HTTP/1.1 `GET`, so the test proves the server is serving rather
/// than merely bound, without an HTTP client dependency.
async fn health(addr: std::net::SocketAddr) -> std::io::Result<String> {
    let mut stream = tokio::net::TcpStream::connect(addr).await?;
    stream
        .write_all(
            format!("GET /health HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n").as_bytes(),
        )
        .await?;
    let mut body = String::new();
    stream.read_to_string(&mut body).await?;
    Ok(body)
}

/// Runs one server to `/health` and back down, returning its gRPC port.
async fn run_once(config: ServerConfig) -> u16 {
    let grpc = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let http = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let grpc_port = grpc.local_addr().unwrap().port();
    let http_addr = http.local_addr().unwrap();

    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
    let server = tokio::spawn(async move {
        run_on(ServerListeners { grpc, http }, config, async {
            let _ = stop_rx.await;
        })
        .await
    });

    let mut answered = false;
    for _ in 0..100 {
        // Each attempt under its own timeout: the listener is already
        // bound, so a connect succeeds whether or not anything is
        // accepting, and a server that never serves would hang the read
        // rather than return an error to retry on.
        if let Ok(Ok(body)) =
            tokio::time::timeout(Duration::from_millis(500), health(http_addr)).await
            && body.starts_with("HTTP/1.1 200")
        {
            answered = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(answered, "the server should have answered /health");

    let _ = stop_tx.send(());
    tokio::time::timeout(Duration::from_secs(10), server)
        .await
        .expect("the server should shut down when told to")
        .expect("the server task should not panic")
        .expect("the server should exit cleanly");
    grpc_port
}

#[tokio::test(flavor = "multi_thread")]
async fn two_experiments_in_one_process_with_different_aggregators() {
    // The sweep case, minimally: two combinations, one process, no
    // environment mutated between them. Before the split this needed two
    // processes, because `CONFLUX_AGGREGATOR` can hold one value.
    let first = config_for("fedavg");
    let second = config_for("median");
    assert_eq!(first.resolved.aggregator.value, "fedavg");
    assert_eq!(second.resolved.aggregator.value, "median");

    let first_port = run_once(first).await;
    let second_port = run_once(second).await;
    assert_ne!(
        first_port, second_port,
        "each run binds its own ephemeral port"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_config_built_in_code_still_cannot_skip_validation() {
    // The counterpart to `conflux-node`'s stub-client guard test, and for
    // the same reason. Splitting resolution from running creates a second
    // way in; if validation had stayed with the environment read, this
    // configuration would start a server whose aggregator name matches no
    // registered strategy and discover it as behaviour in round one.
    let mut config = config_for("fedavg");
    config.resolved.aggregator.value = "not_a_registered_aggregator".to_string();

    let grpc = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let http = TcpListener::bind("127.0.0.1:0").await.unwrap();

    let outcome = run_on(
        ServerListeners { grpc, http },
        config,
        std::future::pending(),
    )
    .await;

    match outcome {
        Err(ServeError::InvalidConfiguration { findings }) => assert!(
            findings
                .iter()
                .any(|f| f.contains("not_a_registered_aggregator")),
            "the refusal should name the value that caused it: {findings:?}"
        ),
        other => panic!("expected a validation refusal, got {other:?}"),
    }
}
