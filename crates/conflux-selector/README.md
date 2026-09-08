# conflux-selector

[![crates.io](https://img.shields.io/crates/v/conflux-selector.svg)](https://crates.io/crates/conflux-selector)
[![docs.rs](https://img.shields.io/docsrs/conflux-selector)](https://docs.rs/conflux-selector)
[![Apache 2.0](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](https://github.com/conflux-fl/conflux-fl/blob/main/LICENSE)

Part of **[Conflux FL](https://confluxfl.dev)** — a configurable, extensible,
Rust-native federated learning framework.

Which clients take part in a round. `UniformRandomSelector` is a literal
implementation of McMahan et al. (2017) — seeded, so a research run is
reproducible.

Strategies register into `conflux-config`'s registry, so adding one is a
trait implementation and a `submit!`, not a change to the server.

## Install

```bash
cargo add conflux-selector
```

## Where this sits

Depends on `conflux-config`, into whose registry it submits.

## Documentation

- **[conflux-selector in depth](https://confluxfl.dev/crate-deep-dives/conflux-selector/)** — the sampling strategies and how to add one
- **[API reference](https://docs.rs/conflux-selector)**
- **[Architecture](https://confluxfl.dev/guides/architecture/)** — how the crates fit together
- **[Getting started](https://confluxfl.dev/getting-started/)**

## License

Licensed under the [Apache License, Version 2.0](https://github.com/conflux-fl/conflux-fl/blob/main/LICENSE).
