# Phase 7c — Observability

## Scope
Replace ad hoc `eprintln!`/`println!` operational logging across the
library crates with structured `tracing` events — leveled (`info`/`warn`/
`error`), with real fields (round number, client id, score, threshold)
instead of interpolated strings, so a real deployment can filter by level
(`RUST_LOG`) and eventually export to a collector. No external
infrastructure needed — pure library/binary code.

**Explicitly out of scope, and deliberately not touched**: ADR 0007's
config-resolution log lines (`conflux-config::ResolvedConfig::to_log_lines`).
Those have an exact, spec-mandated JSON/text format that Phase 1's tests
assert byte-for-byte against spec §4.2's worked examples — routing them
through `tracing`'s own formatter would change that format and break the
contract. They stay plain `println!` in `main.rs`, unchanged.

## Inputs
- Every `eprintln!`/`println!` call across the library crates as of Phase
  7b: `conflux-buffer::log_flush`, `conflux-reputation::filter_by_threshold`,
  `conflux-server::round::check_privacy_budget` and the round-loop
  retry/stop messages in `main.rs`, `conflux-node::bridge`'s retry
  messages, `conflux-store::postgres_store`'s connection-error log,
  `conflux-registry`'s (none currently — `evict_expired` swallows errors
  silently, which this phase also fixes by logging them instead).
- ADR [0007](../adr/0007-explainable-config-resolution.md)'s "say so, out
  loud" principle — this phase extends the same principle to more of the
  runtime, using a real structured-logging mechanism instead of
  hand-rolled `eprintln!` strings.

## Deliverables
- `tracing` as a dependency of `conflux-buffer`, `conflux-reputation`,
  `conflux-registry`, `conflux-store`, `conflux-server`, `conflux-node`
  (all already pull it in transitively via `tonic`; this makes it a direct
  dependency where code actually calls its macros).
- Every prior `eprintln!` converted to a `tracing::{info,warn,error}!` call
  with structured fields, not just a relocated string.
- `conflux-registry::RedisRegistry::evict_expired`'s previously-swallowed
  error now logged at `warn` level instead of silently discarded.
- `tracing_subscriber::fmt().with_env_filter(...).init()` in both
  `conflux-server` and `conflux-node`'s `main.rs`, so `RUST_LOG` controls
  verbosity in both binaries.

## Test plan
- Using the `tracing-test` crate's `#[traced_test]` + `logs_contain(...)`:
  `conflux-buffer`'s flush logs the round/reason/collected/quorum fields on
  both quorum and timeout paths; `conflux-reputation`'s rejection log fires
  exactly once per rejected (not accepted) update, with the right client id
  and scores.

## Definition of done
- [x] `cargo test -p conflux-buffer -p conflux-reputation` passes,
      including the new `tracing`-capturing tests.
- [x] `cargo build --workspace` and `cargo clippy --workspace --all-targets`
      stay clean.
- [x] `docs/STATUS.md` updated.

Also converted (beyond the original scope, found along the way):
`conflux-registry::RedisRegistry::evict_expired`'s previously-swallowed
error, `conflux-store::PostgresStore`'s connection-driver error,
`conflux-server`'s round-loop/privacy-budget logs, and
`conflux-node::NodeBridge`'s retry logs. `conflux-server`/`conflux-node`
`main.rs` both now call `tracing_subscriber::fmt().with_env_filter(...)`.
