# Phase 8a — Hybrid backend selection

## Scope
Make `AppState` able to run against any combination of the backends built
in Phase 7 (`InMemoryRegistry`/`RedisRegistry`,
`InMemoryStore`/`PostgresStore`/`S3Store`, accounting persistence on/off),
selected per-field via `main.rs` env vars — **the hybrid design from
`docs/FLOWER_COMPARISON.md`'s follow-up discussion**: full per-field
flexibility (registry and store choices stay independent, matching how
those traits have been decoupled since Phase 1), plus a fail-fast
validation tied to `mode` so `mode = production` can never silently start
on ephemeral, restart-losing state — the same shape of gap the Flower
cross-check's Problem 3 surfaced on a real system.

**Not a new config axis**: backend *type* selection and connection
strings stay env-var/argument-based (matching every Phase 7 backend's own
`connect`/`connect_with_*` precedent) rather than routed through
`conflux-config`'s `Overrides`/`ResolvedConfig` — a Redis URL is a
deployment specific, not an experiment-tuning parameter with a topology/
mode-driven default. Only the *fail-fast requirement* is mode-driven,
because that's a genuine safety posture, not a connection detail.

## Inputs
- `conflux-registry::{Registry, InMemoryRegistry, RedisRegistry}` (Phases
  1, 7a) and `conflux-store::{Store, InMemoryStore, PostgresStore,
  S3Store}` (Phases 2a, 7b, 7f) — the exact traits/types being unified.
- `conflux-server::AppState` (Phase 5) — `registry`/`store` fields are
  currently concrete (`Arc<InMemoryRegistry>`/`Arc<InMemoryStore>`); every
  existing test constructs `AppState` via `AppState::new` or
  `AppState::new_with_persistent_accounting[_table]`, which must keep
  working unchanged (backward compatibility, not a rewrite).
- ADR [0003](../adr/0003-no-multi-tenancy.md) — `allow_stub_client`'s
  existing mode-driven fail-fast precedent (spec §7) is the direct model
  for this phase's production-backend requirement.

## Deliverables
- `conflux-registry::AnyRegistry` — `InMemory(InMemoryRegistry) |
  Redis(RedisRegistry)`, implementing `Registry` by delegating each method
  to whichever variant it holds (not `dyn Registry` — native `async fn` in
  traits isn't dyn-compatible without extra work, and an enum sidesteps
  that entirely for a small, closed set of backends).
- `conflux-store::AnyStore` — `InMemory(InMemoryStore) |
  Postgres(PostgresStore) | S3(S3Store)`, same delegation pattern.
- `conflux-server`: `AppState`'s `registry`/`store` fields become
  `Arc<AnyRegistry>`/`Arc<AnyStore>`. `AppState::new` and
  `AppState::new_with_persistent_accounting[_table]` keep their exact
  existing signatures and behavior (both now build `AnyRegistry::InMemory`/
  `AnyStore::InMemory` internally — no caller-visible change).
- `BackendSelection` (`RegistryBackend`, `StoreBackend`,
  `AccountingBackend` enums) and `AppState::connect(config,
  initial_weights, backends) -> Result<Self, AppStateError>` — the new,
  general async constructor `main.rs` uses when any durable backend is
  requested.
- `validate_production_backends(mode, &BackendSelection) ->
  Result<(), BackendSelectionError>`, called *inside*
  `AppState::connect` (not left to `main.rs` to remember) — `mode =
  production` with any backend still resolving to its in-memory default
  fails construction with a message naming exactly which env var is
  missing, mirroring `allow_stub_client`'s existing fail-fast shape.
- `main.rs` reads `CONFLUX_REGISTRY_BACKEND`/`CONFLUX_REDIS_URL`,
  `CONFLUX_STORE_BACKEND`/`CONFLUX_POSTGRES_URL`/`CONFLUX_S3_*`,
  `CONFLUX_ACCOUNTING_PERSISTENCE`/(reuses `CONFLUX_POSTGRES_URL`), builds
  a `BackendSelection`, and calls `AppState::connect`.

## Test plan
- `AnyRegistry`/`AnyStore`: each variant still passes the exact same
  behavioral tests `InMemoryRegistry`/`RedisRegistry`/`InMemoryStore`/
  `PostgresStore`/`S3Store` already have, proving delegation doesn't lose
  or alter behavior.
- `validate_production_backends`: `mode = research` never fails regardless
  of backend selection; `mode = production` fails for each of the three
  backends independently when left at its in-memory/disabled default, and
  succeeds when all three are durable.
- Real integration test: `AppState::connect` against real Redis + real
  Postgres (the same dev containers every other Phase 7 test uses),
  proving the general constructor actually produces a working `AppState`,
  not just that it type-checks.

## Definition of done
- [x] `cargo test -p conflux-registry -p conflux-store -p conflux-server`
      passes, including the real Redis+Postgres `AppState::connect` test.
- [x] `cargo build --workspace` and `cargo clippy --workspace --all-targets`
      stay clean.
- [x] Every existing test in the workspace still passes unmodified —
      `AppState::new`'s signature and behavior didn't change.
- [x] `docs/STATUS.md` updated.

## Outcome

`main.rs` now reads `CONFLUX_REGISTRY_BACKEND`/`CONFLUX_REDIS_URL`,
`CONFLUX_STORE_BACKEND`/`CONFLUX_POSTGRES_URL`/`CONFLUX_S3_*`, and
`CONFLUX_ACCOUNTING_PERSISTENCE` (reusing `CONFLUX_POSTGRES_URL`) via
`backend_selection_from_env()`, builds a `BackendSelection`, and calls
`AppState::connect(config, mode, initial_weights, backends)` unconditionally
— unset env vars resolve to `Memory`/`Disabled`, so research-mode behavior
is unchanged from before this phase.

Smoke-tested the binary directly (not just `cargo test`, since `main.rs`
itself isn't covered by the test suite):
- Default (no backend env vars, `mode = research`): starts on in-memory
  backends, same config-log output as before Phase 8a.
- `mode = production`, no backend env vars: fails fast with
  `BackendSelection(ProductionRequiresDurableRegistry)` — exactly the first
  missing durable backend, before attempting any connection.
- `mode = production` with `CONFLUX_REGISTRY_BACKEND=redis`,
  `CONFLUX_STORE_BACKEND=postgres`, `CONFLUX_ACCOUNTING_PERSISTENCE=true`
  (real dev Redis + Postgres containers): connects and runs cleanly.

Full workspace suite stable at 105 passed across repeated runs, `cargo fmt
--all -- --check` and `cargo clippy --workspace --all-targets` both clean.
