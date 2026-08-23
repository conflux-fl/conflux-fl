# 0009 — Project name: Conflux

## Context
The project needed a name that survives contact with crates.io namespace
collisions and captures the core metaphor: many independent, heterogeneous
client contributions converging into one stronger global model.

## Decision
The project is named **Conflux**. Candidates considered and rejected:
- **Alloy** — collides with `alloy-rs`, the dominant Rust Ethereum library.
- **Crucible** — name already taken on crates.io.

"Conflux" captures the aggregation-family metaphor the whole design in spec
§5 is built around: independent contributions flowing together into one
result. `conflux` crates.io/namespace availability should be verified before
the first `cargo new` (tracked as Open Item 1, spec §11 — not yet
exhaustively checked as of the spec's writing).

## Consequences
- All crates are named `conflux-*` (see spec §2's workspace layout).
- If `conflux` turns out to be unavailable on crates.io when publishing is
  needed, this ADR should be revisited rather than silently renaming crates.

## Update (2026-08-22) — renamed from Confluo to Conflux
The project shipped and was developed under the name **Confluo** through
Phase 12, the E2E test harnesses, and the initial `docs/
WEB_APP_INTEGRATION.md`/`docs/AGGREGATION_LANDSCAPE.md` work. It was then
renamed to **Conflux** at the project owner's explicit request. This
document's Context/Decision/Consequences above have been mechanically
updated to read "Conflux" throughout, in place of a second, separate ADR,
per this ADR's own closing line above ("revisited rather than silently
renaming"): the candidates-considered narrative (Alloy, Crucible) and the
crates.io-availability caveat are unchanged in substance, only the chosen
name is different now. Every `confluo-*` crate directory, the
`confluo_client` Python package, `CONFLUO_*` env vars, and all doc
cross-references were renamed in the same pass — see `docs/STATUS.md`'s
"Done" entry for this date for the full scope of what changed.
