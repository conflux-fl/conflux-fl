# Phase 7b — Postgres-backed `Store`: `PostgresStore`

## Scope
A second `conflux-store::Store` implementation backed by Postgres —
durable across restarts, unlike `InMemoryStore`, and without the
one-file-per-round sprawl of `FileStore`. Spec §10 names this
"`ExperimentStore` (Postgres — required for `RdpAccountant` persistence
across restarts)" — the *reason* it's required is a real correctness gap:
`RdpAccountant`'s cumulative epsilon (Phase 2c) lives only in
`AppState`'s in-process `Mutex<RdpAccountant>`; a server restart silently
resets it to zero rounds recorded, which is a genuine privacy-guarantee
violation in production (the accountant would under-report cumulative
epsilon after any restart).

**Scope for this phase**: `PostgresStore` implements `Store` (checkpoint
persistence) against real Postgres. Persisting the accountant's own state
(the actual fix for the epsilon-reset gap) is flagged but **not built
here** — it needs its own schema/design (what "restart-safe accounting"
means: replay `(noise_multiplier, sample_rate)` per round from a table? a
periodically-flushed serialized snapshot?) and deserves a dedicated
follow-up rather than being squeezed into a checkpoint-storage phase. See
`docs/STATUS.md` for the explicit tracking note.

## Inputs
- `conflux-store::{Store, StoreError}` (Phase 2a) — the exact trait this
  must implement.
- A real Postgres for testing (this session: Docker, `postgres:16-alpine`,
  isolated container `conflux-dev-postgres` on `127.0.0.1:15432`, database
  `conflux`, not the default port — doesn't collide with any other
  Postgres instance the host might have, including one already running for
  an unrelated project on this machine).

## Deliverables
- `PostgresStore` implementing `Store`: one row per checkpoint
  (`experiment_id`? — no, ADR 0003 says one process = one experiment, so no
  experiment-scoping column is needed; just `round BIGINT PRIMARY KEY,
  weights BYTEA`), `load_latest_weights` = `ORDER BY round DESC LIMIT 1`,
  `save_checkpoint` = `INSERT ... ON CONFLICT (round) DO UPDATE` (a retried
  round, per Phase 6's documented buffer race, shouldn't fail on a
  duplicate key).
- Schema migration as a plain `.sql` file applied via a `CREATE TABLE IF
  NOT EXISTS` in `PostgresStore::connect` itself (no separate migration
  tool — one table, not worth a framework) rather than requiring external
  setup.
- Connection config via a plain Postgres URL string, matching the
  `RedisRegistry`/`main.rs` precedent (Phase 7a, Phase 5/6) — not
  `conflux-config`-driven.

## Test plan
- Real integration tests against live Postgres (not mocked): save then
  load round-trips actual bytes through the database; loading the highest
  round when multiple checkpoints exist; loading with no checkpoint at all
  returns `StoreError::NoCheckpoint` (same contract as `FileStore`);
  saving the same round twice (the documented buffer-retry race) succeeds
  via upsert rather than erroring.

## Definition of done
- [x] `cargo test -p conflux-store` passes against a real Postgres.
- [x] `cargo build --workspace` and `cargo clippy --workspace --all-targets`
      stay clean.
- [x] `docs/STATUS.md` updated, explicitly noting `RdpAccountant`
      persistence is still not built (this phase covers checkpoints only).

## Deviation discovered while implementing
Same root cause as Phase 7a: `Store`'s trait methods were synchronous
(Phase 2a) — fine for `InMemoryRegistry`/`FileStore`, impossible for
`PostgresStore`'s real network I/O. Converted `Store` to native `async fn`
too, and added `StoreError::Backend(String)`. `FileStore`'s internals stay
plain (blocking) `std::fs` calls under the new async signature — noted
inline as a real cleanup candidate (`spawn_blocking`), not fixed here since
it's out of this phase's scope.

Also had to design table-per-test isolation for the tests (like Phase 7a's
key-per-test fix) after a first draft's "distinct round-number ranges"
approach turned out fragile — see the test module's comments.
