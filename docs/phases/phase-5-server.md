# Phase 5 — `conflux-server`

## Scope
Wire every library crate built in Phases 0–4 into one real
`RoundDispatcher` implementation and a working single-round pipeline:
`AppState` (single-experiment, ADR 0003 — no multi-tenancy), the
`RoundDispatcher` impl that answers `conflux-net`'s RPCs for real, one
round of the Step 0–5 pipeline from spec §8, and a minimal HTTP admin
surface (`/health`, `/round/status`, `/clients/register`).

**Does not build**: CLI/experiment-file parsing into `conflux-config`'s
`Overrides` (spec §11 Open Item 2 — still unresolved; this phase picks
topology/mode via a simple env-var-or-default, not a real config-file
loader), mTLS/JWT auth enforcement (deferred since Phase 3), the
`allow_stub_client` production startup guard (spec §7's stub-vs-real
`ClientApp` distinction doesn't exist at the network layer until Phase 6 —
building a guard against a distinction that doesn't exist yet would be
hollow; tracked as a Phase 6 follow-up), an outer loop that runs rounds
forever until convergence/budget-exhaustion (this phase's `run_round` does
one round; looping it is a thin `main.rs` concern), and wiring
`conflux-config`'s `inventory` strategy registry into real algorithm
selection by name (every family still ships exactly one member — FedAvg,
GaussianClippingPrivacy, UniformRandomSelector, CosineScorer — so
`AppState` selects each concretely; the registry stays a name-existence
check, per Phase 1, until a second member of some family actually exists
to select between).

## Inputs (what must already exist)
- Every library crate from Phases 1–4: `conflux-config::resolve` +
  `ResolvedConfig`, `conflux-registry::{Registry, InMemoryRegistry}`,
  `conflux-store::{Store, InMemoryStore}`,
  `conflux-selector::{ClientSelector, UniformRandomSelector}`,
  `conflux-net::{RoundDispatcher, FlTransportService, DispatchError}`,
  `conflux-buffer::{RoundBuffer, FlushReason}`,
  `conflux-privacy::{GaussianClippingPrivacy, RdpAccountant,
  PrivacyAccountant}`, `conflux-reputation::{CosineScorer,
  filter_by_threshold}`, `conflux-core::{FedAvg, Aggregator}`.
- Spec §8's five-step pipeline and sequence diagram — the authoritative
  shape of one round.
- ADR [0003](../adr/0003-no-multi-tenancy.md) — one `AppState`, no
  per-tenant indirection.
- ADR [0007](../adr/0007-explainable-config-resolution.md) — resolved
  config must be logged before the server is "ready".

## Deliverables
- `AppState`: holds `ResolvedConfig`, `Arc<InMemoryRegistry>`,
  `Arc<InMemoryStore>`, `UniformRandomSelector`, `FedAvg`,
  `GaussianClippingPrivacy`, `Mutex<RdpAccountant>`, `CosineScorer`,
  an `AtomicU64` round counter, the in-flight round's
  `Mutex<Option<Arc<RoundBuffer>>>`, and a `broadcast::Sender<TaskResponse>`
  for push-mode subscribers.
- `RoundDispatcher` impl for `AppState`: `register`/`heartbeat` delegate to
  the registry (a second `register` for an already-registered client is
  treated as idempotent — `accepted: true` — rather than an RPC error, to
  avoid punishing a client's retry); `fetch_task` reads the current round +
  latest checkpoint; `submit_delta` reassembles a client's `DeltaChunk`s
  (ordered by `chunk_index`) into one `ClientDelta` and pushes it into the
  in-flight round's buffer; `subscribe_tasks` wraps a fresh broadcast
  receiver into the stream type `conflux-net` expects.
- `run_round(state: &Arc<AppState>) -> Result<RoundSummary, ServerError>`:
  checks the privacy budget (`budget_exhausted_action` — `Halt` errors out,
  `ContinueWithoutGuarantee` logs loudly and proceeds) → loads weights →
  reads active clients → selects via `UniformRandomSelector` → opens this
  round's `RoundBuffer` (quorum: the resolved `quorum` override if set,
  else every selected client — spec §9 gives no universal default) →
  broadcasts the task for push-mode subscribers (pull-mode clients just see
  the new round on their next `fetch_task`) → awaits the buffer flush →
  re-applies `GaussianClippingPrivacy::transform` server-side per delta →
  scores + filters via `CosineScorer`/`filter_by_threshold` → aggregates
  via `FedAvg` → checkpoints via the store → records the round with the
  accountant → advances the round counter.
- A minimal HTTP surface (axum, already pulled in transitively by tonic's
  `router` feature) on a separate port from the gRPC one: `GET /health`,
  `GET /round/status` (current round number, last flush reason), `POST
  /clients/register` (JSON, delegates to the same registry the gRPC
  `Register` RPC uses).
- `ServerError` (thiserror) wrapping the failure modes from every
  downstream crate plus `BudgetExhausted`.
- A shared little-endian `f32` codec moves from being duplicated in
  `conflux-core`'s private `weights.rs` into `conflux-proto` as a small
  public `encode_weights`/`decode_weights` pair — both `conflux-core` and
  `conflux-server` now call the one implementation instead of each having
  their own copy. (`conflux-store`'s identical-looking codec for
  *checkpoint files* is left alone — different artifact, different call
  site, and `conflux-store` isn't in `conflux-proto`'s dependent set per
  spec §2's graph, so pulling it in would be a bigger architectural change
  than this phase's actual need.)

## Test plan
- A real end-to-end integration test: build `AppState` with in-memory
  backends in pull mode, serve the gRPC `FlTransportService` on a bound
  port, drive it with a real `conflux-net::PullTransport` (standing in for
  what `conflux-node` will be in Phase 6) — register, fetch the round's
  task, submit a delta — then call `run_round` and assert a checkpoint
  landed in the store with the expected aggregated weights.
- `/health` and `/round/status` exercised as real HTTP request/response
  round trips through the actual axum router (via `tower::ServiceExt::oneshot`,
  no need for a second bound port in tests).
- `run_round` with `budget_exhausted_action = Halt` and an already-exhausted
  accountant returns `ServerError::BudgetExhausted` without touching the
  store or registry.
- `submit_delta` reassembles out-of-order `DeltaChunk`s (chunk 1 arriving
  before chunk 0) correctly by sorting on `chunk_index` before
  concatenating.

## Definition of done
- [x] `cargo test -p conflux-server` passes, including the end-to-end test.
- [x] `cargo build --workspace` and `cargo clippy --workspace --all-targets`
      stay clean.
- [x] `docs/STATUS.md` updated, including what's explicitly deferred above.
