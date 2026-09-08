# conflux-node

[![crates.io](https://img.shields.io/crates/v/conflux-node.svg)](https://crates.io/crates/conflux-node)
[![docs.rs](https://img.shields.io/docsrs/conflux-node)](https://docs.rs/conflux-node)
[![Apache 2.0](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](https://github.com/conflux-fl/conflux-fl/blob/main/LICENSE)

Part of **[Conflux FL](https://confluxfl.dev)** — a configurable, extensible,
Rust-native federated learning framework.

The client-side process: it talks to the server over gRPC, bridges to a
local `ClientApp` over loopback, and applies client-side differential
privacy before anything leaves the machine.

It refuses to start in production against the stub `ClientApp` — fixed
dummy weights, no PyTorch — unless explicitly overridden. A live
deployment training on dummy data should not be something you discover
from the accuracy curve.

## Install

```bash
cargo add conflux-node
```

It also ships a `conflux-node` binary:

```bash
cargo install conflux-node
```

## Where this sits

Depends on `conflux-net`, `conflux-proto` and `conflux-privacy`. Ships a `conflux-node` binary; `cflux node start` runs the same code.

## Documentation

- **[conflux-node in depth](https://confluxfl.dev/crate-deep-dives/conflux-node/)** — the bridge, the connection modes, and the startup guard
- **[API reference](https://docs.rs/conflux-node)**
- **[Architecture](https://confluxfl.dev/guides/architecture/)** — how the crates fit together
- **[Getting started](https://confluxfl.dev/getting-started/)**

## License

Licensed under the [Apache License, Version 2.0](https://github.com/conflux-fl/conflux-fl/blob/main/LICENSE).
