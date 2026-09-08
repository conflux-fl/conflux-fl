# cflux

[![crates.io](https://img.shields.io/crates/v/cflux.svg)](https://crates.io/crates/cflux)
[![Apache 2.0](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](https://github.com/conflux-fl/conflux-fl/blob/main/LICENSE)

Part of **[Conflux FL](https://confluxfl.dev)** — a configurable, extensible,
Rust-native federated learning framework.

Inspect the method catalog, resolve a configuration and see **every value
with the tier that set it**, run every startup check without starting
anything, scaffold a deployment, read checkpoints, and start the real
server or node.

`cflux doctor` runs the server's own startup functions rather than an
imitation of them, so a check that passes here is the check that will pass
at startup. Every command supports `--format json`, and exit codes
distinguish *found a problem* from *could not run*.

## Install

```bash
cargo install cflux
```

Or without a Rust toolchain — prebuilt binaries, checksum verified before
anything is unpacked:

```bash
curl -fsSL https://confluxfl.dev/install.sh | sh
```

## Example

```bash
cflux catalog list      # every method, its family and its paper
cflux config check      # every resolved value, and the tier that set it
cflux doctor            # every startup check, nothing started

cflux server start
```

## Where this sits

Depends on the framework crates. A binary only — there is no library here.

## Documentation

- **[cflux in depth](https://confluxfl.dev/crate-deep-dives/cflux/)** — every subcommand, its output formats, and its exit codes
- **[Installation](https://confluxfl.dev/installation/)** — prebuilt binaries and a one-line installer
- **[Architecture](https://confluxfl.dev/guides/architecture/)** — how the crates fit together
- **[Getting started](https://confluxfl.dev/getting-started/)**

## License

Licensed under the [Apache License, Version 2.0](https://github.com/conflux-fl/conflux-fl/blob/main/LICENSE).
