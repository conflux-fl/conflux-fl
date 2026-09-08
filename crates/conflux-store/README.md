# conflux-store

[![crates.io](https://img.shields.io/crates/v/conflux-store.svg)](https://crates.io/crates/conflux-store)
[![docs.rs](https://img.shields.io/docsrs/conflux-store)](https://docs.rs/conflux-store)
[![Apache 2.0](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](https://github.com/conflux-fl/conflux-fl/blob/main/LICENSE)

Part of **[Conflux FL](https://confluxfl.dev)** — a configurable, extensible,
Rust-native federated learning framework.

Checkpoints and experiment metadata, behind one `Store` trait with four
backends: in-memory, file, Postgres and S3. Production mode refuses the
in-memory one — losing a round's model to a restart is not a trade-off
anyone opted into.

## Where this sits

No internal dependencies. The round pipeline loads from it at the start of a round and checkpoints at the end.

## Documentation

- **[conflux-store in depth](https://confluxfl.dev/crate-deep-dives/conflux-store/)** — the trait, each backend's durability guarantees, and checkpoint layout
- **[API reference](https://docs.rs/conflux-store)**
- **[Architecture](https://confluxfl.dev/guides/architecture/)** — how the crates fit together
- **[Getting started](https://confluxfl.dev/getting-started/)**

## License

Licensed under the [Apache License, Version 2.0](https://github.com/conflux-fl/conflux-fl/blob/main/LICENSE).
