# conflux-server

[![crates.io](https://img.shields.io/crates/v/conflux-server.svg)](https://crates.io/crates/conflux-server)
[![docs.rs](https://img.shields.io/docsrs/conflux-server)](https://docs.rs/conflux-server)
[![Apache 2.0](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](https://github.com/conflux-fl/conflux-fl/blob/main/LICENSE)

Part of **[Conflux FL](https://confluxfl.dev)** — a configurable, extensible,
Rust-native federated learning framework.

The full round pipeline: load weights, pick clients, dispatch, stage
submissions until quorum or timeout, apply server-side privacy, score and
filter, aggregate, checkpoint, repeat — until convergence or the privacy
budget runs out.

One process runs one experiment. There is no per-tenant indirection to add,
which keeps failure domains and configuration honest. An authenticated HTTP
surface reports `/health`, `/round/status` and `/rounds`; a transient
backend error backs off and retries, while a violated guarantee halts the
loop and says why.

## Install

```bash
cargo add conflux-server
```

It also ships a `conflux-server` binary:

```bash
cargo install conflux-server
```

## Where this sits

Depends on most of the workspace. Ships a `conflux-server` binary; `cflux server start` runs the same code, in-process.

## Documentation

- **[conflux-server in depth](https://confluxfl.dev/crate-deep-dives/conflux-server/)** — the pipeline stage by stage, the error taxonomy, and the admin surface
- **[API reference](https://docs.rs/conflux-server)**
- **[Architecture](https://confluxfl.dev/guides/architecture/)** — how the crates fit together
- **[Getting started](https://confluxfl.dev/getting-started/)**

## License

Licensed under the [Apache License, Version 2.0](https://github.com/conflux-fl/conflux-fl/blob/main/LICENSE).
