# conflux-proto

[![crates.io](https://img.shields.io/crates/v/conflux-proto.svg)](https://crates.io/crates/conflux-proto)
[![docs.rs](https://img.shields.io/docsrs/conflux-proto)](https://docs.rs/conflux-proto)
[![Apache 2.0](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](https://github.com/conflux-fl/conflux-fl/blob/main/LICENSE)

Part of **[Conflux FL](https://confluxfl.dev)** — a configurable, extensible,
Rust-native federated learning framework.

One `.proto` schema serves both network hops: server to `conflux-node`,
and `conflux-node` to a local `ClientApp` over loopback gRPC. Same message
types, two hops — so the wire contract cannot drift between them, and a
field added for one is immediately available to the other.

## Where this sits

At the bottom of the graph, with no internal dependencies. Everything that speaks the wire depends on this.

## Documentation

- **[conflux-proto in depth](https://confluxfl.dev/crate-deep-dives/conflux-proto/)** — the schema field by field, and why local IPC reuses the network types
- **[API reference](https://docs.rs/conflux-proto)**
- **[Architecture](https://confluxfl.dev/guides/architecture/)** — how the crates fit together
- **[Getting started](https://confluxfl.dev/getting-started/)**

## License

Licensed under the [Apache License, Version 2.0](https://github.com/conflux-fl/conflux-fl/blob/main/LICENSE).
