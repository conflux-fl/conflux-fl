# Phase 14 — `PerClient` epsilon accounting

**Status: shipped (2026-08-26).** Written to accompany
`docs/adr/0006-global-epsilon-accounting.md`'s 2026-08-23 "Update"
section — that update records *why* `PerClient` stays deferred and what
would unblock it; this brief scopes the concrete deliverables.

## Scope

Implement `AccountingScope::PerClient` — bounding each individual
client's total epsilon exposure across every round it participates in,
not just one experiment-wide running total (`AccountingScope::Global`,
the only scope Conflux ships today per ADR 0006). Closes the one
still-open gap ADR 0006 named at the framework's very first design pass.

**Explicitly not in scope**: any change to `RdpAccountant`'s composition
math itself (Mironov 2017 / Wang, Balle & Kasiviswanathan 2019 — already
correctly implemented for a single running total) or to
`GaussianClippingPrivacy`'s clip/noise mechanism. This phase only changes
*which* running total a given client's contribution composes into and
where that per-client total is persisted.

## Inputs

- `docs/adr/0006-global-epsilon-accounting.md`'s 2026-08-23 update — the
  recommendation this brief formalizes (reuse `RdpAccountant`'s existing
  composition logic, key it additionally by `client_id`).
- `conflux-privacy::RdpAccountant` — today's `Global`-only implementation;
  the composition step this phase calls once per (client, round) instead
  of once per round.
- `RedisRegistry` (Phase 7a) and `ExperimentStore`'s Postgres backend
  (Phase 7b/7d) — both real, durable, and already the two backend choices
  every other per-experiment persistent state in this codebase goes
  through (`AnyRegistry`/`AnyStore`, Phase 8a's enum-delegation pattern).
- ADR 0007's explainability principle — cumulative epsilon is already
  logged after every round for `Global` scope; `PerClient` needs the same
  "say so, out loud" treatment, now per-client.

## Open design question this brief scopes rather than resolves upfront

Where does per-client epsilon history live — `conflux-registry` (already
owns per-client lifecycle state: heartbeat, TTL, and now `NodeIdentity`/
`NodeAllowlist` from Phase 8b/8c) or `ExperimentStore` (already owns
`RdpAccountant`'s `Global`-scope persistence across restarts, Phase 7d)?

**Recommendation**: `ExperimentStore`. Epsilon history is fundamentally
accounting state (same category as the existing `Global` accumulator
Phase 7d already persists there), not lifecycle state — co-locating it
with the mechanism that already checkpoints/restores `RdpAccountant`
avoids splitting one privacy accountant's state across two different
stores depending on scope. `conflux-registry` stays scoped to "is this
client currently active," not "how much privacy budget has it consumed."

## Deliverables

- `conflux-privacy::RdpAccountant`: gains a `compose_for_client(&self,
  client_id: &str, ...) -> Result<f64, PrivacyError>` alongside the
  existing experiment-wide `compose(&self, ...)` — same underlying RDP
  math, different running total selected by `client_id`. `Global` scope
  keeps calling `compose`; `PerClient` scope calls `compose_for_client`.
  Internal per-client running totals live in a `HashMap<String, RdpState>`
  (in-memory default) with the same interior-mutability shape
  `temporal.rs`'s `Mutex`-based pattern already established (ADR 0012, if
  adopted, names this as the standing precedent for exactly this kind of
  need) — reused here for privacy accounting rather than aggregation
  history.
- `ExperimentStore` trait: two new methods, `save_client_epsilon(round,
  client_id, cumulative_epsilon)` / `load_client_epsilon_history() ->
  HashMap<String, f64>`, mirroring the existing `Global`-scope
  save/load pair exactly (Phase 7d's own precedent) so restart-recovery
  behaves identically for both scopes.
- `PostgresStore`: new table `client_epsilon_history(client_id TEXT,
  cumulative_epsilon DOUBLE PRECISION, updated_at_round BIGINT)`,
  upserted every round a `PerClient`-scoped accountant composes for a
  given client.
- `conflux-server`'s round pipeline: when `accounting_scope.value ==
  PerClient`, the per-round "check budget, reject if exhausted, log"
  step (already present for `Global`) runs **per client in the batch**
  before that client's update is admitted to aggregation, not once for
  the whole round — a client whose own history exceeds `target_epsilon`
  is excluded from that round's batch (same `budget_exhausted_action`
  config knob ADR 0006's `Global` path already uses, now evaluated
  per-client rather than experiment-wide).
- `AccountingScope::PerClient` selectable from `resolve()` no longer
  fails fast at startup — ADR 0006's fail-fast behavior is removed for
  exactly this scope once this phase ships (any *other* not-yet-real
  scope value would still need its own fail-fast, but there isn't one).

## Test plan

- `RdpAccountant::compose_for_client`: two clients composing
  independently never affect each other's running total (the core
  property distinguishing `PerClient` from `Global`); a client's own
  cumulative epsilon monotonically increases across repeated composition
  calls, matching `Global`'s existing monotonicity test shape.
- `PostgresStore`: real Postgres round-trip — save per-client history
  across several rounds, restart (new `PostgresStore` instance against
  the same database), confirm `load_client_epsilon_history` recovers
  every client's correct cumulative value — mirrors Phase 7d's own
  `Global`-scope restart-recovery test exactly.
- End-to-end: a round with `PerClient` scope and a low `target_epsilon`
  where one client has already exhausted its own budget (seeded via
  `save_client_epsilon`) — confirm that client's update is excluded from
  the round's aggregation batch while every other client's update is
  admitted normally, and the exclusion is logged (ADR 0007).
- Config: `resolve()` accepts `PerClient` without erroring, unlike
  today's fail-fast test in `crates/conflux-config` (that test's
  assertion inverts once this phase ships — flagged here so it isn't
  missed as a "test now failing" surprise during implementation).

## Definition of done

- [x] `cargo test -p conflux-privacy -p conflux-store -p conflux-server`
      passes, including the real-Postgres restart-recovery test.
- [x] `cargo build --workspace` and `cargo clippy --workspace --all-targets`
      stay clean.
- [x] `docs/adr/0006-global-epsilon-accounting.md` gains a second "Update"
      noting this phase shipped, and `docs/STATUS.md` is updated.

## Outcome

Implemented close to this brief's shape, with two deliberate deviations
from its literal wording, both because the brief's method/type names
didn't quite match what actually exists in the codebase (it was written
from ADR 0006's update, not from re-reading the real
`RdpAccountant`/`PrivacyRoundLog` source) — not deviations from its
*intent*:

1. **No `PrivacyError`, no `Result`.** The brief's deliverable described
   `compose_for_client(&self, ...) -> Result<f64, PrivacyError>`. Neither
   `compose` nor `PrivacyError` exist — the real methods are
   `record_round`/`current_epsilon`/`budget_exhausted`, all infallible
   (`&mut self`/`&self`, plain return types, no `Result`). The new
   per-client methods (`record_round_for_client`,
   `current_epsilon_for_client`, `budget_exhausted_for_client`) match
   that existing, infallible shape exactly — an in-memory `HashMap`
   insert has no failure mode to report.
2. **Persisted as raw `(noise_multiplier, sample_rate)` rounds, not a
   precomputed `cumulative_epsilon` number.** The brief's schema
   (`client_epsilon_history(client_id, cumulative_epsilon,
   updated_at_round)`) would persist a value only valid for whatever
   `delta` it was computed with — silently wrong the moment a later run
   resolves a different `delta`. The real `PrivacyRoundLog` trait
   already persists raw rounds and replays them on load for exactly this
   reason (Phase 7d); the per-client table
   (`{table}_client_privacy_rounds`) follows that same, already-correct
   pattern instead of introducing a new, delta-fragile one.

Also not anticipated by the brief: `RdpAccountant` and `PrivacyRoundLog`
now **always** record/persist both `Global` and per-client history,
regardless of which `accounting_scope` is actually configured — so a
deployment that switches scope between restarts finds the other scope's
history already there rather than starting from zero. The brief didn't
specify this either way; it's the more robust choice and costs one
extra `HashMap` insert / one extra `INSERT` per admitted client per
round, not a new I/O round trip.

Everything else shipped as scoped: `AccountingScope::PerClient` resolves
without the ADR 0006 fail-fast (`ConfigError` is now an empty enum,
kept as a type rather than removed, so `resolve()`'s signature doesn't
need to change for what would otherwise be a cosmetic reason); the
per-round budget check moved from a pre-selection experiment-wide gate
(`Global`, unchanged) to a post-decode per-client filter (`PerClient`,
new) — `budget_exhausted_action = Halt` aborts the round the moment any
one client is over budget, `ContinueWithoutGuarantee` excludes just
that client and continues with everyone else, both verified with real
end-to-end tests (a live gRPC server, two real client connections, one
pre-exhausted). 249 → 260 tests passing workspace-wide (11 new); `cargo
fmt` and `cargo clippy --workspace --all-targets` both clean.
