# conflux-core

[![crates.io](https://img.shields.io/crates/v/conflux-core.svg)](https://crates.io/crates/conflux-core)
[![docs.rs](https://img.shields.io/docsrs/conflux-core)](https://docs.rs/conflux-core)
[![Apache 2.0](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](https://github.com/conflux-fl/conflux-fl/blob/main/LICENSE)

Part of **[Conflux FL](https://confluxfl.dev)** — a configurable, extensible,
Rust-native federated learning framework.

Twenty-two server-side methods across five families — averaging, robust,
temporal, trusted and optimization — each a literal implementation of the
paper it cites, not an approximation of it.

The families are the extensibility story. A family is shared accumulation
logic plus a small trait capturing what members vary, so a new published
method is usually a short trait implementation rather than a new
aggregator. `FedAvg` is `WeightedAverageAggregator<SampleCountWeighting>`.

Methods whose papers state a minimum batch size — Krum's `n >= 2f + 3`,
Bulyan's `n >= 4f + 3` — are held to it: a batch outside the citation
halts the run rather than returning a number that looks sound.

## Example

```rust
use conflux_core::{AggregatorParams, build_aggregator};

// Selected by name, out of the compile-time registry — the same string
// a configuration file or CONFLUX_AGGREGATOR would carry.
let krum = build_aggregator("krum", AggregatorParams::default()).unwrap();

// A name nothing registers is an error, never a panic.
assert!(build_aggregator("krumm", AggregatorParams::default()).is_err());
```

## Where this sits

Depends on `conflux-config` and `conflux-proto`. The last step before a checkpoint is written.

## Documentation

- **[conflux-core in depth](https://confluxfl.dev/crate-deep-dives/conflux-core/)** — the family pattern in full, with the generics explained
- **[API reference](https://docs.rs/conflux-core)**
- **[Architecture](https://confluxfl.dev/guides/architecture/)** — how the crates fit together
- **[Getting started](https://confluxfl.dev/getting-started/)**

## License

Licensed under the [Apache License, Version 2.0](https://github.com/conflux-fl/conflux-fl/blob/main/LICENSE).
