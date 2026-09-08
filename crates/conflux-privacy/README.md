# conflux-privacy

[![crates.io](https://img.shields.io/crates/v/conflux-privacy.svg)](https://crates.io/crates/conflux-privacy)
[![docs.rs](https://img.shields.io/docsrs/conflux-privacy)](https://docs.rs/conflux-privacy)
[![Apache 2.0](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](https://github.com/conflux-fl/conflux-fl/blob/main/LICENSE)

Part of **[Conflux FL](https://confluxfl.dev)** — a configurable, extensible,
Rust-native federated learning framework.

Local differential privacy — clip, then add calibrated noise — and the
Rényi accountant that tracks what it costs. `GaussianClippingPrivacy`
follows Abadi et al. (2016); the accountant uses the Sampled Gaussian
Mechanism closed form from Mironov, Talwar & Zhang (2019), falling back to
a non-subsampled bound whenever the closed form does not apply, so epsilon
can only ever be over-reported.

Cumulative epsilon is logged after every round. Budget exhaustion is a
configured decision — halt, or continue with the guarantee explicitly
withdrawn — never a silent one.

## Where this sits

Depends on `conflux-config`, into whose registry it submits. Applied client-side by `conflux-node` and server-side by `conflux-server`.

## Documentation

- **[conflux-privacy in depth](https://confluxfl.dev/crate-deep-dives/conflux-privacy/)** — the mechanism, the accountant's numerics, and per-client budgeting
- **[API reference](https://docs.rs/conflux-privacy)**
- **[Architecture](https://confluxfl.dev/guides/architecture/)** — how the crates fit together
- **[Getting started](https://confluxfl.dev/getting-started/)**

## License

Licensed under the [Apache License, Version 2.0](https://github.com/conflux-fl/conflux-fl/blob/main/LICENSE).
