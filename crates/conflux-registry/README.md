# conflux-registry

[![crates.io](https://img.shields.io/crates/v/conflux-registry.svg)](https://crates.io/crates/conflux-registry)
[![docs.rs](https://img.shields.io/docsrs/conflux-registry)](https://docs.rs/conflux-registry)
[![Apache 2.0](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](https://github.com/conflux-fl/conflux-fl/blob/main/LICENSE)

Part of **[Conflux FL](https://confluxfl.dev)** — a configurable, extensible,
Rust-native federated learning framework.

Registration, heartbeats, TTL eviction, and the node allow-list that
gates which clients may join. In-memory for research; Redis when
registrations must survive a restart, which production mode requires
rather than suggests.

## Install

```bash
cargo add conflux-registry
```

## Where this sits

No internal dependencies. Used by `conflux-server` to answer "who is available this round?"

## Documentation

- **[conflux-registry in depth](https://confluxfl.dev/crate-deep-dives/conflux-registry/)** — the lifecycle states, TTL semantics, and how the allow-list is enforced
- **[API reference](https://docs.rs/conflux-registry)**
- **[Architecture](https://confluxfl.dev/guides/architecture/)** — how the crates fit together
- **[Getting started](https://confluxfl.dev/getting-started/)**

## License

Licensed under the [Apache License, Version 2.0](https://github.com/conflux-fl/conflux-fl/blob/main/LICENSE).
