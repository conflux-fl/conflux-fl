# Phase 7d — `RdpAccountant` persistence

## Scope
Close the actual correctness gap Phase 7b flagged and deliberately didn't
fix: `RdpAccountant`'s cumulative epsilon lives only in `AppState`'s
in-process `Mutex` and resets to zero rounds recorded on every server
restart. In production, that's a real privacy-guarantee violation — the
accountant would under-report cumulative epsilon after any restart,
potentially reporting "budget not exhausted" when the true cumulative
exposure already exceeded `target_epsilon`.

**Design decision**: replay, not snapshot. Persist every
`(noise_multiplier, sample_rate)` pair `RdpAccountant::record_round`
records as its own row (append-only), and on startup, load every row and
replay it into a fresh `RdpAccountant` via the same `record_round` call a
live round uses. Chosen over serializing the accountant's internal state
because it needs no new (de)serialization of `conflux-privacy`'s
internals, it's naturally idempotent, and it matches the pattern
`PostgresStore` (7b) already established for checkpoints (append rows,
read on startup) rather than introducing a second persistence idiom.

**Scope boundary**: this only makes sense with `PostgresStore` in play —
`InMemoryStore`/`FileStore` have no restart-durability story to extend,
and adding a no-op implementation for them would be misleading (asking
"is accounting persistent?" should have one honest answer per backend).
`AppState`'s primary `store` field (checkpoints) is untouched — this adds
a separate, optional field specifically for accounting persistence, not a
genericization of `AppState` over `Store` implementations.

## Inputs
- `conflux-privacy::{RdpAccountant, PrivacyAccountant}` (Phase 2c) —
  `record_round` is the exact replay mechanism; no changes needed there.
- `conflux-store::PostgresStore` (Phase 7b) — same connection, same
  `conflux-dev-postgres` container, a second table.
- `conflux-server::round::record_round_privacy_cost` (Phase 5) — the one
  call site that needs to also persist, not just update the in-memory
  accountant.

## Deliverables
- `conflux-store::PrivacyRoundLog` trait: `append_round(noise_multiplier,
  sample_rate)`, `load_rounds() -> Vec<(f32, f32)>`. Implemented only by
  `PostgresStore` (new table `conflux_privacy_rounds`, `BIGSERIAL` primary
  key so `load_rounds` naturally replays in recording order).
- `AppState` gains `accountant_log: Option<Arc<PostgresStore>>` — `None`
  by default (matches today's in-memory-only behavior exactly), `Some`
  when constructed via a new `AppState::new_with_persistent_accounting`
  that connects `PostgresStore`, replays any existing rounds into a fresh
  `RdpAccountant` before the server ever answers a round, and keeps the
  handle for future appends.
- `round::record_round_privacy_cost` appends to `accountant_log` (if set)
  immediately after updating the in-memory accountant, so both stay in
  sync every round, not just at startup/shutdown.
- `main.rs` wires this behind `CONFLUX_POSTGRES_URL` — set it, get
  persistent accounting; unset, get today's Phase 5 behavior unchanged.

## Test plan
- Real integration test against live Postgres: build one `AppState` with
  persistent accounting, record several rounds (via the real
  `record_round_privacy_cost` path, not calling the accountant directly),
  then construct a **second, independent** `AppState` against the same
  Postgres table — simulating a restart — and assert its accountant's
  `current_epsilon` already reflects the first instance's recorded rounds,
  not zero.
- `load_rounds` on an empty table returns an empty `Vec`, not an error
  (a first-ever startup has nothing to replay).

## Definition of done
- [x] `cargo test -p conflux-store -p conflux-server` passes against a
      real Postgres, including the "simulated restart" test.
- [x] `cargo build --workspace` and `cargo clippy --workspace --all-targets`
      stay clean.
- [x] `docs/STATUS.md` updated — this phase should let STATUS.md finally
      mark the Phase 7b-flagged gap as closed, not just tracked.

## Implementation note
`AppState::new_with_persistent_accounting` delegates to a
`..._table` variant that also takes an explicit table name — added after
a first draft of the "simulated restart" test tried to bypass the public
constructor (reaching into a hand-rolled parallel replay instead of the
real code path) because the constructor only accepted a `postgres_url`
and always used the default table, making it untestable in isolation.
Mirrors `PostgresStore::connect`/`connect_with_table`'s split exactly, for
the same reason: `cargo test`'s parallel execution needs per-test
isolation, not a shared default table.
