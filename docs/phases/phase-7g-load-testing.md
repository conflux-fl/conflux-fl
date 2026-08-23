# Phase 7g — Load testing

## Scope
Validate `conflux-server` under realistic concurrency: many simulated
clients (`conflux-net::PullTransport`, the same real client used by every
other integration test this session) registering and participating
across several rounds against one real, running `AppState` + gRPC server,
not a single client as every prior integration test used. Reports basic
timing (round duration, per-client RPC latency) and — the actual point —
asserts correctness held under concurrency: every client's submission was
counted, no round silently lost a participant, no deadlock.

**Not a separate load-testing tool/framework**: given `conflux-net`'s
`PullTransport` already exists and is exactly the client every other
Phase 5/6 test drives, spinning up N of them concurrently *is* a load
test — no need for an external tool (e.g. `k6`, `locust`) when the real
client library is right here and already proven correct for the
single-client case.

## Inputs
- `conflux-server`'s full pipeline (Phase 5), already proven correct for
  one client (`end_to_end_single_round_pull_mode`, Phase 5's own test).
- `conflux-net::PullTransport` (Phase 3) — reused as-is, N times
  concurrently, not reimplemented.
- The residual `RoundBuffer` race flagged in Phase 6/7d's known
  deviations — concurrency is exactly the condition that could surface it,
  so this phase is also a real chance to either hit it or gain evidence
  it doesn't manifest at this scale.

## Deliverables
- A `conflux-server` test spinning up `N` (order of tens) concurrent
  simulated clients, each running its own `PullTransport`: register, then
  loop fetch_task → "train" (fixed offset, same dummy transform the Phase
  6 stub Python client uses) → submit_delta across several rounds.
- Basic timing captured and reported (not just pass/fail): total wall time
  for all rounds, per-round quorum-reached latency.

## Test plan
- All `N` clients' submissions are counted in the round they were
  submitted for — no lost updates under concurrent `submit_delta` calls
  hitting the same `RoundBuffer`.
- The server completes all configured rounds without error, without
  hanging, and without any client's RPC ever failing.
- Report whether the known `RoundBuffer` race (Phase 6/7d) was observed —
  document either outcome honestly in `docs/STATUS.md`, not just "test
  passed."

## Definition of done
- [x] `cargo test -p conflux-server --test load -- --nocapture` passes and
      prints its timing summary.
- [x] `cargo build --workspace` and `cargo clippy --workspace --all-targets`
      stay clean.
- [x] `docs/STATUS.md` updated with the actual observed numbers/outcome,
      not a claim.

## Observed results
30 concurrent clients × 3 rounds, run 5 times total (once with output
captured, four more to check for flakiness under concurrency — none
observed). Each round completed in 28–46ms; all 90 client-round
submissions across all 5 runs were counted (`num_submitted`/`num_passed`
always 30/30) — **the known Phase 6/7d `RoundBuffer` race was not observed
at this scale.** That's evidence it doesn't manifest under 30-way
concurrency on localhost, not proof it can't occur — the race window
(between a retried round and `AppState::current_buffer` being replaced)
is a timing condition this test doesn't specifically try to hit; it's
still tracked as a known gap in `docs/STATUS.md`, not closed by this
result.
