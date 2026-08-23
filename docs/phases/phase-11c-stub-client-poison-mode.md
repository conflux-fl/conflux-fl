# Phase 11c — A poison mode for the Python stub client

## Scope

`python/conflux_client/stub_client.py` only ever submits one honest
"trained" delta (`+1.0` to every weight). Phase 11a's `robust`
aggregation family exists specifically to resist adversarial submissions
— proving that cross-language, over the real network hop, needs a real
adversarial Python client, not just Rust-side unit tests feeding
synthetic `ClientDelta`s directly to an `Aggregator`. This phase adds an
opt-in poison mode to the existing stub, the smallest change that makes
Phase 11a's poison-resistance property observable end-to-end, and gives
`docs/E2E_TESTING.md`'s planned harness a ready-made adversarial client.

**Not in scope**: this is not ADR 0005's deferred real `ClientApp` SDK,
same as the stub itself isn't. A poison flag on a fixed-offset stub is a
test fixture, not a step toward the SDK design.

## Deliverables
- `stub_client.py`: new `--poison` flag (default off — zero behavior
  change for every existing invocation, including `docs/USAGE.md`'s
  quick-start and the Phase 6 three-process smoke test) and
  `--poison-magnitude` (default a large constant, e.g. `1000.0`). When
  set, the client submits `weights + poison_magnitude` instead of
  `weights + DUMMY_TRAINING_OFFSET` — a large-magnitude attack, the same
  shape Phase 11a's own poison tests use Rust-side, so a real end-to-end
  run and the unit tests are testing the same threat model.
- `stub_client.py` logs which mode it ran in unconditionally (`"trained"`
  vs `"POISONED"`) — this is a test fixture standing in for an
  adversary; it should never be ambiguous from the output which one ran.
- `python/conflux_client/README.md` updated with the new flag and a
  pointer to `docs/E2E_TESTING.md`'s poison-test section.

## Test plan
No new Rust tests (this phase touches only the Python stub). Manual
verification: `--poison` unset behaves identically to before this phase
(the existing Phase 6 three-process walkthrough, unmodified); `--poison`
set against a `conflux-server` configured with `aggregator = "krum"` (or
`"multi_krum"`/`"trimmed_mean"`/`"median"`) and at least one honest
client shows the poisoned submission's influence bounded — the same
property Phase 11a's Rust-side poison tests already prove, observed here
over the real network hop instead.

## Definition of done
- [x] `--poison` defaults to off; every existing stub-client invocation
      documented in `docs/USAGE.md` behaves unchanged.
- [x] `python/conflux_client/README.md` documents the new flag.
- [x] `docs/STATUS.md` updated.

## Outcome

Implemented exactly as specced: `--poison`/`--poison-magnitude` added to
`stub_client.py`, default off, logs `POISONED` unambiguously when active.
`python/conflux_client/README.md` updated with a usage example and
pointers to `docs/E2E_TESTING.md`/`phase-11a-robust-aggregation.md`.

**Manually verified live**, not just read for plausibility: a throwaway
example binary (`AppState::new` with `aggregator = "krum"`,
`robust_byzantine_fraction = 0.34`, deleted after use — not a shipped
artifact) served real gRPC on `127.0.0.1:50051`. Three real
`stub_client.py` processes — two honest, one `--poison
--poison-magnitude 1000.0` — registered and submitted concurrently. The
first attempt raced Phase 10a's `RoundClosed` fix live (round 1 had
already closed with quorum 0 before all three finished registering,
correctly rejecting all three submissions rather than losing any of
them silently) — a genuine, unplanned confirmation that fix works
outside its own test suite. A retry landed all three in the same round:
server log shows `quorum=3`, `num_submitted=3`, `num_passed=3`; a fresh
`FetchTask` afterward read the checkpoint back as **exactly `(1.0,
1.0)`** — the two honest clients' value — with the attacker's `1000.0`
submission completely excluded. Real, cross-language, cross-process,
over-the-network proof that Phase 11a's `krum` aggregator, Phase 11c's
poison client, and Phase 10a's race fix all work together correctly, not
just in isolation.

No Rust changes in this phase; `docs/STATUS.md` updated.
