# conflux-client

[![crates.io](https://img.shields.io/crates/v/conflux-client.svg)](https://crates.io/crates/conflux-client)
[![docs.rs](https://img.shields.io/docsrs/conflux-client)](https://docs.rs/conflux-client)
[![Apache 2.0](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](https://github.com/conflux-fl/conflux-fl/blob/main/LICENSE)

Part of **[Conflux FL](https://confluxfl.dev)** — a configurable, extensible,
Rust-native federated learning framework.

Write a `ClientApp` in Rust and train in-process: no Python, no loopback
hop, no serialization between your training code and the node. Implement
one method and the SDK handles registration, round tracking, the
placeholder-initialization handshake, chunking, submission, and a round
closing while you are still working.

The Python SDK (`python/conflux_client/`) implements the same contract, so
the two are interchangeable from the server's point of view.

## Where this sits

Depends on `conflux-net` and `conflux-proto`. An alternative to running Python behind `conflux-node`.

## Documentation

- **[conflux-client in depth](https://confluxfl.dev/crate-deep-dives/conflux-client/)** — the trait, the round lifecycle, and how it compares to the Python SDK
- **[API reference](https://docs.rs/conflux-client)**
- **[Architecture](https://confluxfl.dev/guides/architecture/)** — how the crates fit together
- **[Getting started](https://confluxfl.dev/getting-started/)**

## License

Licensed under the [Apache License, Version 2.0](https://github.com/conflux-fl/conflux-fl/blob/main/LICENSE).
