# Phase 10b — Wiring the strategy registry into real selection

## Scope
`conflux-config`'s `inventory`-based strategy registry (ADR 0002, spec §5)
has existed since Phase 1 but nothing ever submitted a real entry to it,
and `AppState::assemble` has always hardcoded `FedAvg::default()` /
`UniformRandomSelector { seed }` rather than reading
`config.aggregator.value` / `config.selector.value`. This phase wires the
path end-to-end for the one member each family currently ships — proving
the mechanism works, not waiting for a second `robust`/averaging member
to justify it.

**Not in scope**: `privacy_mechanism` (`GaussianClippingPrivacy` — Phase
2c) follows the same pattern but is left for a follow-up, to keep this
phase's diff reviewable; the `robust` aggregation family (Phase 8, spec
§10) still has zero members and nothing here changes that.

## Inputs
- `conflux-config::registry` (`StrategyEntry`, `StrategyKind`, `lookup`) —
  the existing, previously-unused mechanism.
- `conflux-core::Aggregator` / `conflux-selector::ClientSelector` — both
  already plain, object-safe, `Send + Sync` traits; `FedAvg`/
  `UniformRandomSelector` need no internal restructuring to be boxed as
  `Box<dyn Aggregator>` / `Box<dyn ClientSelector>`.
- ADR 0002: *"New methods register into `conflux-config`'s compile-time
  strategy registry via `inventory::submit!`, selected by config
  (`aggregator = "fedavg"`) without any change to `conflux-server`."* —
  this phase is that ADR's deferred implementation, not a new design.

## Deliverables
- `conflux-core`/`conflux-selector` each gain a `conflux-config`
  dependency (consistent with "conflux-proto and conflux-config sit
  beneath everything," spec §2 — this isn't a new graph edge the spec
  forbids, just one that was deferred) and `inventory` directly.
- `conflux-core`: `inventory::submit! { StrategyEntry { kind:
  StrategyKind::Aggregator, name: "fedavg" } }` plus
  `build_aggregator(name: &str) -> Result<Box<dyn Aggregator>,
  AggregatorBuildError>` (currently one match arm: `"fedavg"`).
- `conflux-selector`: same shape — `inventory::submit!` for
  `"uniform_random"`, `build_selector(name: &str, seed: SelectionSeed) ->
  Result<Box<dyn ClientSelector>, SelectorBuildError>`.
- `conflux-server::app_state.rs`: `selector`/`aggregator` fields become
  `Box<dyn ClientSelector>` / `Box<dyn Aggregator>`; `assemble` calls
  `conflux_core::build_aggregator(&config.aggregator.value)` /
  `conflux_selector::build_selector(&config.selector.value, seed)`.
  **`AppState::new`'s signature and behavior stay exactly unchanged**
  (Phase 8a's precedent) — `assemble` stays infallible and `.expect()`s
  on an unknown name, matching how `main.rs` already treats config
  resolution itself as a startup-invariant, not a runtime `Result` to
  propagate. Every existing call site resolves `aggregator`/`selector`
  through the builtin fallback (`"fedavg"`/`"uniform_random"`), so this
  can never actually panic for any test or default deployment today —
  only an explicit override naming something unregistered would.

## Test plan
- `conflux-core`/`conflux-selector`: `build_*` succeeds for the one real
  name each family ships, fails for an unknown name; a test asserting
  every name `build_*` accepts is also found by
  `conflux_config::lookup` (and vice versa) — catches the two staying
  out of sync as family membership grows.
- `conflux-server`: `AppState::new` with the default config still
  produces a working `FedAvg`/`UniformRandomSelector` behind the trait
  object — the existing `end_to_end_single_round_pull_mode` test (and
  every other pre-Phase-10 test) passes completely unmodified, proving
  zero behavior change for the default path.
- Real test: an explicit `Overrides { aggregator: Some("fedavg".into()),
  .. }` and `Overrides { selector: Some("uniform_random".into()), .. }`
  resolve and construct successfully end-to-end (proves the
  config-value → registry-lookup → construction path is live, not just
  "the default happens to still work").

## Definition of done
- [x] `cargo test -p conflux-core -p conflux-selector -p conflux-server`
      passes.
- [x] `cargo build --workspace` and `cargo clippy --workspace --all-targets`
      stay clean.
- [x] Every pre-Phase-10 test still passes unmodified.
- [x] `docs/STATUS.md` updated.

## Outcome

Implemented exactly as specced. `conflux-core`/`conflux-selector` each
gained `conflux-config` + `inventory` dependencies, one
`inventory::submit!` (`"fedavg"` / `"uniform_random"`), and one
`build_*` factory function. `AppState`'s `selector`/`aggregator` fields
are now `Box<dyn ClientSelector>` / `Box<dyn Aggregator>`; `assemble`
constructs them via the two `build_*` calls and `.expect()`s on an
unknown name — `AppState::new`'s signature is byte-for-byte unchanged, so
every pre-Phase-10 test needed zero modification (confirmed: all passed
unmodified). `round.rs` lost two now-unused trait imports (`Aggregator`/
`ClientSelector`) — calling a method on a `dyn Trait` value doesn't
require the trait itself in scope, unlike a generic `T: Trait` call.

Tests: `build_aggregator`/`build_selector` succeed for their one real
name, fail for an unknown one, and a "registry-sync" test in each crate
catches `inventory::submit!` and the `build_*` match arms drifting apart.
`crates/conflux-server/tests/strategy_registry.rs`: an explicit
`Overrides{aggregator: Some("fedavg"), selector: Some("uniform_random")}`
resolves through the registry and completes a real round end-to-end
(FedAvg of one update equals that update's weights, proving a real
working `Aggregator` was constructed, not a stub); a `catch_unwind` test
confirms an unregistered name panics loudly at `AppState::new` rather
than silently falling back to anything.

155 tests passing workspace-wide (was 145 at the end of Phase 10a),
stable across 3 repeated runs; `cargo fmt --check` and
`cargo clippy --workspace --all-targets` both clean with zero warnings.
