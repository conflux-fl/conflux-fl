# conflux-reputation

[![crates.io](https://img.shields.io/crates/v/conflux-reputation.svg)](https://crates.io/crates/conflux-reputation)
[![docs.rs](https://img.shields.io/docsrs/conflux-reputation)](https://docs.rs/conflux-reputation)
[![Apache 2.0](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](https://github.com/conflux-fl/conflux-fl/blob/main/LICENSE)

Part of **[Conflux FL](https://confluxfl.dev)** — a configurable, extensible,
Rust-native federated learning framework.

Contribution scoring and Byzantine detection, applied between the buffer
and aggregation. Opt-in by design: a scorer running unconditionally in
front of every aggregator would be an uncited filter no paper asks for,
and would mask the aggregator's own behavior.

Every rejected update is logged with its score and the threshold that
rejected it.

## Where this sits

No internal dependencies. Sits between `conflux-buffer` and `conflux-core` in the round pipeline.

## Documentation

- **[conflux-reputation in depth](https://confluxfl.dev/crate-deep-dives/conflux-reputation/)** — the scorers, the threshold semantics, and why filtering is opt-in
- **[API reference](https://docs.rs/conflux-reputation)**
- **[Architecture](https://confluxfl.dev/guides/architecture/)** — how the crates fit together
- **[Getting started](https://confluxfl.dev/getting-started/)**

## License

Licensed under the [Apache License, Version 2.0](https://github.com/conflux-fl/conflux-fl/blob/main/LICENSE).
