# 0003 — No multi-tenancy

## Context
A federated learning server could in principle host multiple concurrent
experiments in one process, multiplexing client connections, round state,
and checkpoints per experiment. This adds significant complexity (isolation,
per-tenant resource limits, cross-tenant scheduling) for a use case that
isn't core to the framework's purpose.

## Decision
Conflux is explicitly single-tenant: one server process runs exactly one
experiment. Running multiple experiments means running multiple processes.
This is an application-layer concern, out of scope for the framework itself
(see spec §1's non-goals).

## Consequences
- `conflux-server`'s `AppState` is single-experiment (see Phase 5 in spec
  §10) — no per-tenant indirection anywhere in the pipeline.
- Orchestrating multiple experiments (e.g. process supervision, port
  allocation) is left to whatever deploys Conflux, not built into it.
- If multi-tenancy is ever needed, it is a new major-version decision, not an
  incremental feature.
