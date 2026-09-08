# conflux-buffer

[![crates.io](https://img.shields.io/crates/v/conflux-buffer.svg)](https://crates.io/crates/conflux-buffer)
[![docs.rs](https://img.shields.io/docsrs/conflux-buffer)](https://docs.rs/conflux-buffer)
[![Apache 2.0](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](https://github.com/conflux-fl/conflux-fl/blob/main/LICENSE)

Part of **[Conflux FL](https://confluxfl.dev)** — a configurable, extensible,
Rust-native federated learning framework.

Updates arrive asynchronously; a round has to close at some point. The
buffer stages submissions and flushes on whichever comes first — quorum
reached, or timeout elapsed — and **says which one it was**. A run that
quietly stops closing on quorum and starts closing on timeout has lost
participants without failing.

## Install

```bash
cargo add conflux-buffer
```

## Where this sits

Depends on `conflux-proto`. Feeds the reputation filter and then aggregation.

## Documentation

- **[conflux-buffer in depth](https://confluxfl.dev/crate-deep-dives/conflux-buffer/)** — the flush conditions, the state machine, and what a timeout costs
- **[API reference](https://docs.rs/conflux-buffer)**
- **[Architecture](https://confluxfl.dev/guides/architecture/)** — how the crates fit together
- **[Getting started](https://confluxfl.dev/getting-started/)**

## License

Licensed under the [Apache License, Version 2.0](https://github.com/conflux-fl/conflux-fl/blob/main/LICENSE).
