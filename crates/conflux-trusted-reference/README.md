# conflux-trusted-reference

[![crates.io](https://img.shields.io/crates/v/conflux-trusted-reference.svg)](https://crates.io/crates/conflux-trusted-reference)
[![docs.rs](https://img.shields.io/docsrs/conflux-trusted-reference)](https://docs.rs/conflux-trusted-reference)
[![Apache 2.0](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](https://github.com/conflux-fl/conflux-fl/blob/main/LICENSE)

Part of **[Conflux FL](https://confluxfl.dev)** — a configurable, extensible,
Rust-native federated learning framework.

FLTrust and Zeno are defined in terms of a server that holds a small
trusted dataset and can train or score against it. That capability does
not belong inside `conflux-server` — it is a different trust posture and a
different failure domain — so it runs as an optional sidecar behind a
capability handshake.

Deployments not using those methods never run it, and `conflux-server`
never depends on it.

## Where this sits

Depends on `conflux-proto`. An optional separate process, reached over gRPC.

## Documentation

- **[conflux-trusted-reference in depth](https://confluxfl.dev/crate-deep-dives/conflux-trusted-reference/)** — the handshake, the capability model, and why it is a sidecar
- **[API reference](https://docs.rs/conflux-trusted-reference)**
- **[Architecture](https://confluxfl.dev/guides/architecture/)** — how the crates fit together
- **[Getting started](https://confluxfl.dev/getting-started/)**

## License

Licensed under the [Apache License, Version 2.0](https://github.com/conflux-fl/conflux-fl/blob/main/LICENSE).
