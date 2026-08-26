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

## Update (2026-08-23) — the Phase 7/8 blocker is now cleared; scoped as Phase 14

This ADR originally deferred `PerClient` to "Phase 8," using the spec's
old generic placeholder numbering — `docs/STATUS.md`'s own note already
flags that label as stale (the real Phase 8 shipped node auth, not
accounting). The actual blocker this ADR names — **per-client round
history landing in `conflux-registry`/`ExperimentStore`** — is still the
real gate, and it's still not built. What's changed since this ADR was
written: `RedisRegistry` (Phase 7a) and `ExperimentStore`'s Postgres
backend (Phase 7b/7d) both now exist and are real, tested, durable
backends — the infrastructure `PerClient` accounting would need to
*persist into* is no longer hypothetical, only the per-client history
schema itself is missing.

**Recommendation** (scoped in full as `docs/phases/
phase-14-perclient-accounting.md`, not decided here — this ADR records
*that* per-client accounting stays real future work and *why* it's
gated, not the concrete schema): add a `client_epsilon_history` table
(Postgres) / hash-per-client-id (Redis) recording cumulative epsilon
spent per `client_id`, written by the same call site `RdpAccountant`
already updates for `Global` scope today, keyed additionally by
`client_id`. `AccountingScope::PerClient` then means: before admitting a
client's update into a round, look up that client's own cumulative
epsilon (not the experiment-wide one) and apply the RDP composition
there. This reuses `RdpAccountant`'s existing composition math
unchanged — the only new work is *which* running total a given update
composes into, and where that per-client total is persisted — consistent
with this ADR's original framing that `PerClient` is an accounting
*scope* change, not a new privacy mechanism.

Still an open product question this update doesn't resolve: whether
`ExperimentStore`'s existing schema is the right home for per-client
epsilon history, or whether it belongs in `conflux-registry` instead
(registry already owns per-client lifecycle state — heartbeat, TTL —
which argues for co-locating epsilon history there rather than splitting
"client state" across two stores). `docs/phases/
phase-14-perclient-accounting.md` scopes both options; this ADR doesn't
pick one, since that's an implementation-phase decision, not an
architecture-boundary one.

## Update (2026-08-26) — `PerClient` shipped

`docs/phases/phase-14-perclient-accounting.md` is implemented — the
`ExperimentStore` question above resolved in favor of `PostgresStore`
(this update's own recommendation), with one real correction to that
recommendation once actual code was in front of it: raw
`(noise_multiplier, sample_rate)` rounds are persisted per client, not
a precomputed `cumulative_epsilon` number — a precomputed value is only
valid for whatever `delta` it was computed with, and would go silently
stale the moment a later run resolves a different one. This matches
what `PrivacyRoundLog` (Phase 7d) already does for `Global` scope; the
per-client table follows the same, already-correct pattern rather than
introducing a new delta-fragile one. `AccountingScope::PerClient` no
longer fails fast at resolve time — see the phase brief's own "Outcome"
section for the full account, including the other place its literal
wording (written from this update's prose, not from the real
`RdpAccountant`/`PrivacyRoundLog` source) needed correcting once
implemented.
