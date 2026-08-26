# Phase 20 (draft) — Config-file parsing

**Status: scoping draft, not started.**

## Scope

Spec §11 Open Item 2: "Config file format & merge details beyond
precedence order... exact TOML schema for `inherits` merge semantics on
nested tables isn't fully specified." This phase closes the *narrower*
and immediately actionable half of that item: **experiment-level file
overrides** — reading a TOML file from disk into an `Overrides` struct
and feeding it to `resolve()`'s existing `file` parameter. The *broader*
half — topology/mode **profiles** themselves defined via TOML with
`inherits`-based extension (spec §4.1's `inherits = "research"` example)
— is real, separate, larger work, scoped below as an explicit follow-on,
not this phase's deliverable.

## Why the experiment-level half is smaller than it looks

`conflux-config::resolve()` (`crates/conflux-config/src/lib.rs`) already
accepts `file: Option<(&str, &Overrides)>` as its outermost override
tier, and every one of its ~20 parameters already reads through
`file_overrides.and_then(|o| o.some_field.clone())` in the existing
layering chain — this was built in from Phase 1 onward, evidently
anticipating this exact phase, but nothing today ever *constructs* that
`Some((path, overrides))` argument: `main.rs` always passes `None`. The
missing piece is purely "read a TOML file into an `Overrides` value,"
not any change to the resolution/layering logic itself, which is already
correct and already tested (`crates/conflux-config/src/lib.rs`'s own
`ConfigSource::ExperimentFile` test, using a hand-constructed `Overrides`
today in place of one parsed from a real file).

## Inputs

- `conflux-config::Overrides` — today `#[derive(Debug, Default, Clone)]`,
  no `serde::Deserialize`. Every field is already `Option<T>` for exactly
  the reason TOML deserialization needs (a field absent from the file
  should resolve to `None`, not error) — the struct shape is already
  correct for this use, it just doesn't derive the trait yet.
- `resolve()`'s existing `file` parameter and `ConfigSource::
  ExperimentFile(String)` provenance label — unchanged by this phase;
  this phase only supplies a real value for what's already plumbed
  through to it.
- ADR 0007 (explainable config resolution) — a config value's *source*
  is already surfaced in `to_log_lines()`; this phase adds one more real
  way a value can come from `ExperimentFile`, no new explainability work
  needed beyond making sure the file *path itself* (not just the literal
  string `"experiment.toml"` today's tests use) ends up in that label.

## Deliverables

- `conflux-config` gains `serde` (with `derive`) and `toml` as
  dependencies; `Overrides` gains `#[derive(serde::Deserialize)]`
  (`#[serde(default)]` on the struct, or per-field, so a partial TOML
  file — most experiments will only override a handful of fields —
  deserializes correctly with every other field defaulting to `None`).
  The existing custom types used by `Overrides`'s fields
  (`ConnectionMode`, `AuthMode`, `SeedMode`) each need `Deserialize` too
  — check whether they already derive it for CLI-arg parsing; if so,
  reuse the same derive, don't add a second parsing path.
- New `conflux-config::load_experiment_file(path: &Path) ->
  Result<Overrides, ConfigFileError>` — reads the file, parses TOML,
  returns a typed error (`thiserror`-derived, per this codebase's
  standing rule) distinguishing "file not found" from "TOML syntax
  error" from "a field's value doesn't match its expected type" — each
  needs a distinct, actionable message; a config typo shouldn't produce
  a bare `toml::de::Error` debug-formatted at the user, per ADR 0007's
  explainability principle applied to *failure* messages, not just
  successful resolution.
- `main.rs` (`conflux-server`'s binary): reads an optional
  `CONFLUX_EXPERIMENT_CONFIG_PATH` env var; if set, calls
  `load_experiment_file`, passes `Some((path, &overrides))` to
  `resolve()`; if unset, passes `None` exactly as today — zero behavior
  change for every deployment not yet using a config file.
- A documented, minimal TOML schema (this phase's own scope, not the
  broader profile-`inherits` question): flat top-level keys matching
  `Overrides`' field names exactly (`aggregator = "krum"`,
  `robust_byzantine_fraction = 0.2`, ...) — no nested tables, no
  `inherits` key. This is deliberately the simplest schema that closes
  the "config file parsing" gap without also solving the harder,
  separately-scoped profile-inheritance question below.

## Explicitly out of scope: profile-file `inherits`

Spec §4.1's `inherits = "research"` example describes a **topology or
mode profile** extending a base profile — today, `Mode::Research::
defaults()`/`Mode::Production::defaults()` and their topology
equivalents are hardcoded Rust (Phase 1), not data. Making *profiles
themselves* TOML-defined, with `inherits`-based extension between them,
is a materially larger change (profiles become data loaded at startup,
looked up by name, merged via `inherits` chains — nested-table merge
semantics spec §11 Open Item 2 explicitly flags as still unspecified)
than parsing one flat experiment-override file into the existing
`Overrides` struct. Recommended as its own future phase
(`phase-21-profile-file-parsing.md`, not written here) once this phase's
narrower version is real and in use — solving the harder problem before
the easier one ships first would leave the actually-common case (one
experiment's worth of overrides) blocked on a much bigger design.

## Test plan

- `load_experiment_file`: a real TOML file with a handful of fields set
  deserializes to an `Overrides` with exactly those fields `Some(...)`
  and everything else `None`; a missing file, a syntactically invalid
  TOML file, and a file with a wrong-typed field (e.g.
  `robust_byzantine_fraction = "not a number"`) each produce a distinct,
  named `ConfigFileError` variant — not all collapsed into one generic
  "parse failed."
- `resolve()` integration: a parsed-from-a-real-file `Overrides` produces
  identical resolution results to the existing hand-constructed
  `Overrides` used in today's `ConfigSource::ExperimentFile` test — proof
  the new file-reading path feeds the same, already-correct layering
  logic, not a parallel one.
- Precedence: a field set in the file *and* via an env var — env var
  wins (matches the existing, already-tested precedence order; this
  phase must not change it, only supply a real file-sourced value into
  the tier that's always existed).
- `main.rs` smoke test: `CONFLUX_EXPERIMENT_CONFIG_PATH` unset behaves
  identically to before this phase (regression check); set to a real
  file, the resolved config's `to_log_lines()` output shows the expected
  values sourced as `ExperimentFile` with the real path in the label.

## Definition of done

- [ ] `cargo test -p conflux-config` passes, including the new
      `load_experiment_file` unit tests and the `resolve()` parity test.
- [ ] `cargo build --workspace` and `cargo clippy --workspace --all-targets`
      stay clean.
- [ ] `docs/STATUS.md`'s config-file-parsing deviation bullet updated;
      spec §11 Open Item 2 annotated as "experiment-file half closed,
      profile-file half tracked separately."
