# conflux-federation

[![crates.io](https://img.shields.io/crates/v/conflux-federation.svg)](https://crates.io/crates/conflux-federation)
[![docs.rs](https://img.shields.io/docsrs/conflux-federation)](https://docs.rs/conflux-federation)
[![Apache 2.0](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](https://github.com/conflux-fl/conflux-fl/blob/main/LICENSE)

Part of **[Conflux FL](https://confluxfl.dev)** — a configurable, extensible,
Rust-native federated learning framework.

Run a whole federation — one server, N nodes and N clients — inside a
single process. Every hop is the real one: clients reach their node over
the local gRPC hop, nodes reach the server over loopback gRPC, updates
are serialized, chunked and reassembled exactly as they are across
machines, and the server runs its usual buffer, quorum-or-timeout flush,
privacy, reputation, aggregation and checkpoint pipeline.

**This is not a simulation.** The only thing given up is process
isolation, so a number produced here is a number about the real pipeline.
Nothing it writes is marked `simulated`, because nothing here is.

## Install

```bash
cargo add conflux-federation
```

## The whole thing in one call

```rust,no_run
use conflux_federation::{ClientApp, FederationConfig, TrainResult};

struct MyApp { /* this client's data */ }

impl ClientApp for MyApp {
    fn train(&mut self, weights: &[f32], _round: u64) -> TrainResult {
        TrainResult::new(weights.to_vec(), 100)
    }
}

# async fn demo() -> Result<(), Box<dyn std::error::Error>> {
let summary = conflux_federation::run(
    FederationConfig { nodes: 4, rounds: 10, ..Default::default() },
    |_i| MyApp { },
).await?;
# Ok(())
# }
```

Or take the handle and attach your own clients — Python trainers in their
own processes, say:

```rust,no_run
# async fn demo() -> Result<(), Box<dyn std::error::Error>> {
use conflux_federation::{Federation, FederationConfig};

let fed = Federation::start(FederationConfig { nodes: 4, ..Default::default() }).await?;
for (i, url) in fed.node_urls().iter().enumerate() {
    println!("node {i}: {url} — client id {}", fed.client_id(i));
}
fed.shutdown().await?;
# Ok(())
# }
```

## Why it binds the ports for you

N nodes need N distinct local ports, and every way of choosing them
without binding is a guess. A base port plus an index collides with
whatever else is on the machine; checking a port is free and *then*
binding it is a race rather than a check. Binding `127.0.0.1:0` lets the
OS assign — but only whoever binds can read back what it assigned, which
is why this crate binds every listener itself and hands each one to
`conflux_server::run_from_env_on` or `conflux_node::run_on`.

Binding first also removes the usual "poll the port until the server is
up" step: a node connecting to a bound-but-not-yet-accepted listener
completes its TCP connection from the kernel's backlog and waits in the
HTTP/2 handshake until the server accepts.

## Try it

```bash
cargo run --example one_process_regression -p conflux-federation
```

Three clients recover `y = w·x + b`. Each one's data holds a different
feature nearly constant, so no client can learn the model alone — the
example prints each solo fit next to the federated one rather than
asserting the difference.

## Where this sits

| Crate | Role |
|---|---|
| `conflux-server` | The round pipeline |
| `conflux-node` | The two-hop bridge |
| `conflux-client` | The Rust `ClientApp` SDK |
| **`conflux-federation`** | **Runs all three in one process** |

Full crate reference: <https://confluxfl.dev/reference/crates/>
