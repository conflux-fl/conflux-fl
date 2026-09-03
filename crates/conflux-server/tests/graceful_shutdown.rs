//! The server must exit cleanly on `SIGTERM`, not be killed mid-round.
//!
//! The failure this prevents: a binary that does not handle `SIGTERM` or
//! Ctrl-C. `docker stop`, a Kubernetes eviction, and systemd all send
//! `SIGTERM` first and `SIGKILL` after a grace period — with no handler,
//! the default disposition terminates the process immediately, so the
//! grace period is spent doing nothing and the round in flight is lost
//! along with any checkpoint being written.
//!
//! This drives the **real binary** rather than a library function, because
//! signal handling is process-level behavior and the wiring between the
//! handler, the two servers, and the round loop is exactly what a
//! library-level test would not exercise. That is also why it is the only
//! test in this crate that spawns a subprocess.

#![cfg(unix)]

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Binds two ephemeral ports and immediately releases them, so the spawned
/// server gets addresses that are almost certainly free.
///
/// Racy in principle. In practice the window is microseconds and the
/// alternative — a fixed port — makes the test fail whenever anything else
/// on the machine happens to hold it, including a second copy of this test.
fn free_addr() -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);
    addr.to_string()
}

/// Polls `/health` until the server answers or the deadline passes.
fn wait_until_serving(http_addr: &str, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        let reachable = std::net::TcpStream::connect_timeout(
            &http_addr.parse().unwrap(),
            Duration::from_millis(200),
        )
        .is_ok();
        if reachable {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    false
}

/// `SIGTERM` must produce a clean exit, and it must be *prompt* — a handler
/// that only checks the flag once an hour would technically pass a
/// "does it exit" test while still being killed by every real orchestrator.
#[test]
fn sigterm_exits_cleanly_and_promptly() {
    let grpc_addr = free_addr();
    let http_addr = free_addr();

    let mut child = Command::new(env!("CARGO_BIN_EXE_conflux-server"))
        // Research mode with the in-memory backends: this test is about
        // the signal path, and involving Redis or Postgres would make it
        // fail for reasons that have nothing to do with shutdown.
        .env("CONFLUX_TOPOLOGY", "cross_device")
        .env("CONFLUX_MODE", "research")
        .env("CONFLUX_GRPC_ADDR", &grpc_addr)
        .env("CONFLUX_HTTP_ADDR", &http_addr)
        // Quorum of 1 with no clients means every round hits the round
        // timeout and then `EmptyBatch` — i.e. the loop spends this test
        // in its retry path, which is the interesting state to interrupt.
        .env("CONFLUX_QUORUM", "1")
        .env("CONFLUX_ROUND_TIMEOUT_SECS", "1")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("failed to spawn conflux-server");

    assert!(
        wait_until_serving(&http_addr, Duration::from_secs(30)),
        "server never started listening on {http_addr}"
    );

    let pid = child.id();
    let killed = Command::new("kill")
        .arg("-TERM")
        .arg(pid.to_string())
        .status()
        .expect("failed to run kill");
    assert!(killed.success(), "kill -TERM did not succeed");

    // Generous relative to what this should take (the round timeout is 1s),
    // tight relative to a real orchestrator's grace period (10-30s), so a
    // handler that hangs still fails here.
    let deadline = Instant::now() + Duration::from_secs(20);
    let status = loop {
        match child.try_wait().expect("failed to poll child") {
            Some(status) => break status,
            None if Instant::now() >= deadline => {
                let _ = child.kill();
                panic!(
                    "server did not exit within 20s of SIGTERM — it was either \
                     killed by the default disposition (no handler) or the \
                     handler is not reaching the servers and the round loop"
                );
            }
            None => std::thread::sleep(Duration::from_millis(100)),
        }
    };

    assert!(
        status.success(),
        "SIGTERM should produce a clean exit, got {status:?} — a non-zero \
         status means something panicked on the way out rather than draining"
    );
}

/// The same contract for Ctrl-C, which is what a developer actually sends
/// and the path most likely to be exercised by hand.
#[test]
fn sigint_exits_cleanly() {
    let grpc_addr = free_addr();
    let http_addr = free_addr();

    let mut child = Command::new(env!("CARGO_BIN_EXE_conflux-server"))
        .env("CONFLUX_TOPOLOGY", "cross_device")
        .env("CONFLUX_MODE", "research")
        .env("CONFLUX_GRPC_ADDR", &grpc_addr)
        .env("CONFLUX_HTTP_ADDR", &http_addr)
        .env("CONFLUX_QUORUM", "1")
        .env("CONFLUX_ROUND_TIMEOUT_SECS", "1")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("failed to spawn conflux-server");

    assert!(
        wait_until_serving(&http_addr, Duration::from_secs(30)),
        "server never started listening on {http_addr}"
    );

    let status = Command::new("kill")
        .arg("-INT")
        .arg(child.id().to_string())
        .status()
        .expect("failed to run kill");
    assert!(status.success());

    let deadline = Instant::now() + Duration::from_secs(20);
    let status = loop {
        match child.try_wait().expect("failed to poll child") {
            Some(status) => break status,
            None if Instant::now() >= deadline => {
                let _ = child.kill();
                panic!("server did not exit within 20s of SIGINT");
            }
            None => std::thread::sleep(Duration::from_millis(100)),
        }
    };

    assert!(status.success(), "SIGINT should produce a clean exit");
}
