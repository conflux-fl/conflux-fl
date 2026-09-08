# conflux-net

[![crates.io](https://img.shields.io/crates/v/conflux-net.svg)](https://crates.io/crates/conflux-net)
[![docs.rs](https://img.shields.io/docsrs/conflux-net)](https://docs.rs/conflux-net)
[![Apache 2.0](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](https://github.com/conflux-fl/conflux-fl/blob/main/LICENSE)

Part of **[Conflux FL](https://confluxfl.dev)** — a configurable, extensible,
Rust-native federated learning framework.

Dual-mode transport: **push**, where the server dispatches to connected
clients, and **pull**, where clients ask for work when ready. Cross-silo
deployments use push; cross-device defaults to pull, because a phone
should not be assumed reachable.

mTLS and JWT authentication both live here, as does the size ceiling on
what one client may submit.

## Install

```bash
cargo add conflux-net
```

## Where this sits

Depends on `conflux-proto`. Used by both binaries and by `conflux-client`.

## Documentation

- **[conflux-net in depth](https://confluxfl.dev/crate-deep-dives/conflux-net/)** — the two modes, the auth paths, and the size ceiling
- **[API reference](https://docs.rs/conflux-net)**
- **[Architecture](https://confluxfl.dev/guides/architecture/)** — how the crates fit together
- **[Getting started](https://confluxfl.dev/getting-started/)**

## License

Licensed under the [Apache License, Version 2.0](https://github.com/conflux-fl/conflux-fl/blob/main/LICENSE).
