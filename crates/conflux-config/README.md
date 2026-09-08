# conflux-config

[![crates.io](https://img.shields.io/crates/v/conflux-config.svg)](https://crates.io/crates/conflux-config)
[![docs.rs](https://img.shields.io/docsrs/conflux-config)](https://docs.rs/conflux-config)
[![Apache 2.0](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](https://github.com/conflux-fl/conflux-fl/blob/main/LICENSE)

Part of **[Conflux FL](https://confluxfl.dev)** — a configurable, extensible,
Rust-native federated learning framework.

Every parameter resolves through one chain — built-in fallback, topology
profile, mode profile, experiment file, environment, CLI — and **every
resolved value logs the tier that set it** at startup. The two axes are
orthogonal: topology answers *what kind of participants and network?*,
mode answers *am I iterating, or is this live?*

Also home to the compile-time strategy registry. An implementation
registers itself with `inventory::submit!`, so configuration selects it by
name (`aggregator = "krum"`) without the server knowing it exists.

## Install

```bash
cargo add conflux-config
```

## Example

```rust
use conflux_config::{Mode, Overrides, Topology, resolve};

let config = resolve(
    Topology::CrossSilo,
    Mode::Production,
    None,
    &Overrides::default(),
    &Overrides::default(),
)
.unwrap();

// Every value knows which tier decided it.
for line in config.to_log_lines(config.config_log_format.value) {
    println!("{line}");
}
```

## Where this sits

Beneath everything. The family crates depend on it to register themselves into its strategy registry.

## Documentation

- **[conflux-config in depth](https://confluxfl.dev/crate-deep-dives/conflux-config/)** — the resolution chain, profile inheritance, and the validation rules
- **[API reference](https://docs.rs/conflux-config)**
- **[Architecture](https://confluxfl.dev/guides/architecture/)** — how the crates fit together
- **[Getting started](https://confluxfl.dev/getting-started/)**

## License

Licensed under the [Apache License, Version 2.0](https://github.com/conflux-fl/conflux-fl/blob/main/LICENSE).
