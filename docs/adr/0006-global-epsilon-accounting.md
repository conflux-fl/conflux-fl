# 0006 — `Global` epsilon accounting for v1

## Context
Differential privacy accounting can be scoped globally (one epsilon for the
whole experiment) or per-client (bounding each individual's exposure across
rounds). `PerClient` scope requires per-client round history to be available
in `conflux-registry`/`ExperimentStore` before every selection decision —
infrastructure that doesn't exist yet and isn't needed to prove the core
pipeline.

## Decision
Ship `AccountingScope::Global` only in the initial build. `PerClient` is
deferred to Phase 8 (spec §6, §10), chosen deliberately to unblock a faster
first working prototype. Selecting `PerClient` before it's implemented must
fail fast at startup rather than silently behaving like `Global`, consistent
with the explainability principle in [[0007-explainable-config-resolution]].

## Consequences
- `RdpAccountant` in Phase 2 only needs to track one running epsilon per
  experiment, not per client.
- `PerClient` accounting is blocked on per-client round history landing in
  `conflux-registry`/`ExperimentStore`, which is itself Phase 7/8 work.
- Any config that requests `PerClient` scope before Phase 8 must error at
  startup, not degrade silently.
