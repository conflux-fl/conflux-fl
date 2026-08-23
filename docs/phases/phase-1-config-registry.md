# Phase 1 — `conflux-config` + `conflux-registry`

## Scope
Build the two-axis config resolution engine (`conflux-config`) and the
client lifecycle registry (`conflux-registry`). This phase does **not**
build: any actual algorithm implementations that will later register into
the strategy registry (FedAvg, GaussianClippingPrivacy, UniformRandomSelector
— Phase 2), TOML/CLI/env parsing into the `Overrides` structs this phase
defines (Phase 5 integration wires real parsing in; this phase's `resolve()`
takes already-parsed overrides), `PerClient` accounting itself (deferred to
Phase 8 — this phase only fails fast if it's selected), or `RedisRegistry`
(Phase 7).

## Inputs (what must already exist)
- Phase 0's workspace scaffold and `conflux-proto` (done, see
  `docs/phases/phase-0-workspace-proto.md`).
- The exact `ConfigSource` enum from spec §4.2:
  ```rust
  pub enum ConfigSource {
      Cli,
      EnvVar(String),
      ExperimentFile(String),
      ModeProfile(String),
      TopologyProfile(String),
      BuiltinFallback,
  }
  ```
- The topology table (spec §3) and mode profile TOML (spec §4.1).
- The unified config reference table (spec §9) — the authoritative list of
  every parameter, which axis owns it, and research/production defaults.
- Relevant ADRs: [0001](../adr/0001-two-axis-configuration.md) (the two
  axes), [0006](../adr/0006-global-epsilon-accounting.md) (fail fast on
  `PerClient`), [0007](../adr/0007-explainable-config-resolution.md)
  (mandatory provenance logging).

## Deliverables

### `conflux-config`
- Enums for every parameter with a closed set of values (`ConnectionMode`,
  `AuthMode`, `SeedMode`, `BudgetExhaustedAction`, `AccountingScope`,
  `LogFormat`), plus `Topology` (`CrossSilo | CrossDevice | Crowdsource |
  Edge`) and `Mode` (`Research | Production`) with a `defaults()` method
  each.
- An `Overrides` struct (one `Option<T>` per §9 parameter) — the shape every
  override tier (file/env/cli) is expressed in.
- `resolve(topology, mode, file, env, cli) -> Result<ResolvedConfig,
  ConfigError>` implementing the precedence order from §4.1: builtin
  fallback → topology profile → mode profile → file → env → cli. (Ordering
  *within* the "explicit override" tier — cli beats env beats file — is a
  Phase 1 decision filling part of spec §11 Open Item 2, not something the
  spec pins down; document it as such.)
- `ResolvedConfig` — same field set as `Overrides`, each wrapped with its
  `ConfigSource` — plus a method producing the provenance log lines from
  §4.2 in both `json` and `text` format.
- `ConfigError::PerClientAccountingNotImplemented`, returned by `resolve()`
  when `accounting_scope` resolves to `PerClient` (ADR 0006 — fail fast, not
  silent fallback to `Global`).
- A minimal `inventory`-based strategy registry (`StrategyEntry`,
  `StrategyKind`, `lookup()`) per ADR 0002 — the mechanism only; no real
  entries ship until Phase 2+ crates submit their own.

### `conflux-registry`
- `ClientId` newtype, `ClientInfo` (id, registered-at, last-heartbeat).
- `Registry` trait: `register`, `heartbeat`, `evict_expired(ttl)`,
  `active_clients()`.
- `InMemoryRegistry` — the only implementation this phase ships (plan §4,
  row S1); `RedisRegistry` is Phase 7.

## Test plan
- `conflux-config`: precedence resolution (each of the 6 `ConfigSource`
  tiers wins when it's the most specific one set), `inherits`-equivalent
  topology/mode default lookup, JSON/text log line format matches spec
  §4.2's examples, `PerClient` selection returns
  `ConfigError::PerClientAccountingNotImplemented` rather than resolving.
- `conflux-config` strategy registry: a test-submitted `StrategyEntry` is
  found by `lookup()`.
- `conflux-registry`: register → heartbeat → evict lifecycle; duplicate
  registration errors; heartbeat on an unknown client errors; TTL expiry
  actually evicts; `active_clients()` excludes evicted clients.

## Definition of done
- [x] `cargo test -p conflux-config -p conflux-registry` passes.
- [x] `cargo build --workspace` and `cargo clippy --workspace --all-targets`
      stay clean.
- [x] Every `ResolvedConfig` field carries a correct `ConfigSource`,
      verified by test, not just by eyeballing log output.
- [x] `docs/STATUS.md` updated: Phase 1 done, Phase 2 (leaf crates
      `conflux-store`/`conflux-selector`/`conflux-privacy`/`conflux-reputation`,
      parallelizable per plan §4) next.
