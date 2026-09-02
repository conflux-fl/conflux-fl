# Conflux Architecture

What Conflux is, how its pieces fit together, and how the project was
actually built, session by session. For step-by-step build/run
instructions, see [USAGE.md](USAGE.md). For the authoritative design
spec, see `spec/conflux-spec-v1.md`; for the
reasoning behind individual decisions, see
Design decisions.

## What Conflux is

Conflux is a configurable, extensible, Rust-native federated learning
framework. Python (PyTorch) stays entirely client-side for model
training; Rust owns networking, orchestration, aggregation, privacy, and
reputation. One codebase supports four deployment topologies —
cross-silo, cross-device, crowdsource, edge — selected by configuration,
not by forking code. The name captures the core metaphor: many
independent, heterogeneous client contributions *conflux*-ing into one
stronger global model (ADR 0009).

## Workspace layout

Fifteen crates, dependency graph is acyclic:

```mermaid
graph TD
 proto["conflux-proto<br/><small>protobuf schema — network + local IPC</small>"]
 config["conflux-config<br/><small>layered config, strategy registry</small>"]
 registry["conflux-registry<br/><small>client lifecycle</small>"]
 store["conflux-store<br/><small>checkpoint + accounting persistence</small>"]
 selector["conflux-selector<br/><small>client sampling</small>"]
 net["conflux-net<br/><small>dual-mode gRPC transport</small>"]
 buffer["conflux-buffer<br/><small>quorum/timeout staging</small>"]
 privacy["conflux-privacy<br/><small>DP clip+noise, epsilon accounting</small>"]
 reputation["conflux-reputation<br/><small>contribution scoring</small>"]
 core["conflux-core<br/><small>aggregation (21 methods)</small>"]
 attacks["conflux-attacks<br/><small>test/dev-only: known FL attacks</small>"]
 server["conflux-server (bin)<br/><small>integrates everything</small>"]
 node["conflux-node (bin)<br/><small>client-side bridge</small>"]
 client["conflux-client<br/><small>Rust ClientApp SDK</small>"]
 trusted["conflux-trusted-reference<br/><small>optional sidecar (never a server dep)</small>"]

 net --> proto
 buffer --> proto
 core --> proto
 core --> config
 selector --> config
 privacy --> config
 attacks --> proto
 attacks -. dev-dependency only .-> core
 node --> net
 node --> proto
 node --> privacy
 client --> net
 client --> proto
 trusted --> proto

 server --> proto
 server --> config
 server --> registry
 server --> store
 server --> selector
 server --> net
 server --> buffer
 server --> privacy
 server --> reputation
 server --> core

 style proto fill:#2b6cb0,color:#fff
 style config fill:#2b6cb0,color:#fff
 style server fill:#b7791f,color:#fff
 style node fill:#b7791f,color:#fff
 style attacks fill:#742a2a,color:#fff,stroke-dasharray: 5 5
```

`conflux-attacks` is deliberately outside
`conflux-server`'s dependency graph entirely — dashed in the diagram to
mark it as test/dev-only, never reachable from a production binary.
`cargo tree -p conflux-server` never lists it.

`conflux-proto` and `conflux-config` sit at the bottom with zero internal
dependencies. `conflux-net` and `conflux-buffer` depend only on
`conflux-proto`. `conflux-core`, `conflux-selector`, and `conflux-privacy`
each additionally depend on `conflux-config` — each
registers its family's members into `conflux-config`'s `inventory`
strategy registry (ADR 0002) so `config.aggregator.value = "fedavg"`/
`config.selector.value = "uniform_random"`/`config.privacy_mechanism.value
= "gaussian_clipping"` actually construct the right implementation,
rather than `conflux-server` hardcoding one — all three spec §5 families,
not some of them. `conflux-registry`, `conflux-store`, and
`conflux-reputation` are leaf crates with **no** internal dependencies at
all — deliberately: they operate on plain `String`/`&[f32]` rather than
importing `conflux-proto`'s network types, so `conflux-server` (the one
crate that touches everything) is responsible for converting at the
integration boundary.
`conflux-node` depends only on `conflux-proto` and `conflux-net` — it's a
thin bridge, not a second copy of the server's logic.

`python/conflux_client/` sits outside the Rust workspace entirely — the
real Python `ClientApp` SDK design is deferred (ADR 0005); today it holds
only a stub client for pipeline testing.

## The round pipeline

One `.proto` schema (the `FlTransport` gRPC service) serves two hops with
the same message types: the network hop between `conflux-server` and
`conflux-node`, and the local loopback hop between `conflux-node` and the
Python `ClientApp` (ADR 0004). `conflux-node` is a *bridge*: every RPC a
local Python client makes is forwarded to the real server, with
retry/backoff, and nothing else.

```mermaid
sequenceDiagram
 participant Server as conflux-server<br/>(AppState)
 participant Store as conflux-store
 participant Reg as conflux-registry
 participant Sel as conflux-selector
 participant Buf as conflux-buffer
 participant Node as conflux-node
 participant Py as Python ClientApp
 participant Priv as conflux-privacy
 participant Rep as conflux-reputation
 participant Core as conflux-core

 Note over Server: run_round() — one call per round
 Server->>Store: load_latest_weights()
 Server->>Reg: active_clients()
 Server->>Sel: select(candidates, n, round)
 Server->>Buf: open RoundBuffer(round, quorum)

 Node->>Server: FetchTask (forwarded from Py)
 Server-->>Node: task (round, weights)
 Node-->>Py: task (forwarded)
 Py->>Py: train (real SDK deferred — stub adds a fixed offset)
 Py->>Node: SubmitDelta
 Node->>Server: SubmitDelta (forwarded, retried on failure)
 Server->>Buf: push(delta)

 Buf-->>Server: flush (quorum reached OR timeout — logged either way)
 loop each delta in the batch
 Server->>Priv: transform (server-side clip + noise)
 Server->>Rep: score against reference direction
 end
 Server->>Core: aggregate(passing deltas)
 Core-->>Server: new global weights
 Server->>Store: save_checkpoint(round, weights)
 Server->>Priv: record_round(noise_multiplier, sample_rate)
 Note over Server,Priv: also persisted to PostgresStore<br/>if accounting_log is Some — survives a restart
 Server->>Server: round += 1
```

`run_round` returns after one round; the caller (`main.rs`, or a test)
decides whether to call it again. On `AggregatorError::EmptyBatch`
(nothing submitted yet — the common case before any client has
registered), `main.rs`'s loop retries after a short delay rather than
treating it as fatal; on `BudgetExhausted` or a store/registry failure, it
stops.

## Configuration: two orthogonal axes

Every parameter resolves through a fixed six-tier precedence chain, and
**every resolved value logs which tier it came from** — this is mandatory
(ADR 0007), not optional verbosity, and it's what makes a misconfigured
deployment debuggable without reading source.

```mermaid
flowchart LR
 A[built-in fallback] --> B[topology profile]
 B --> C[mode profile]
 C --> D[experiment file]
 D --> E[env var]
 E --> F[CLI flag]
 F --> R{{resolved value<br/>+ ConfigSource}}

 style A fill:#4a5568,color:#fff
 style F fill:#2b6cb0,color:#fff
 style R fill:#276749,color:#fff
```

**Topology** (`cross_silo | cross_device | crowdsource | edge`) answers
"what kind of participants and network?" — it owns `connection_mode`,
`auth`, `round_timeout_secs`, `min_reputation_score`, and
`client_registry_ttl`. **Mode** (`research | production`) answers "am I
iterating, or running a live deployment?" — it owns `seed_mode`,
`budget_exhausted_action`, `accounting_scope`, `allow_stub_client`, and
`config_log_format`. The two axes own disjoint parameter sets by design
(ADR 0001), so layering never conflicts, and `inherits` lets a new special
case extend a base profile without a code change.

## The family pattern

Published FL research produces many variants of the same underlying
mechanism. A *family* factors out the shared accumulation/selection logic
once, and captures what varies in a small trait (ADR 0002):

```mermaid
graph TD
 Agg["Aggregator trait<br/><small>aggregate(updates) -> weights</small>"]
 WAA["WeightedAverageAggregator&lt;W&gt;<br/><small>shared accumulation, written once</small>"]
 AW["AveragingWeighting trait<br/><small>weight_for(update, batch) -> f32</small>"]
 SCW["SampleCountWeighting<br/><small>McMahan et al., 2017</small>"]
 FedAvg["FedAvg = WeightedAverageAggregator&lt;SampleCountWeighting&gt;"]
 Future["a future variant<br/><small>~10-line trait impl</small>"]

 Agg -. impl .-> WAA
 WAA -->|generic over| AW
 AW -. impl .-> SCW
 AW -. impl .-> Future
 SCW --> FedAvg

 style FedAvg fill:#276749,color:#fff
 style Future fill:#4a5568,color:#fff,stroke-dasharray: 5 5
```

The `robust` (Byzantine-resilient) aggregation family applies the same
pattern **twice**, not once — Krum/Multi-Krum select a
*subset of whole updates* (`UpdateFilter` + `FilteredAggregator<F, C>`,
reusing `DistanceMatrix`, built); Trimmed Mean/Median combine
*one coordinate at a time across every client*
(`CoordinateWiseRobustStatistic` + `CoordinateWiseAggregator<S>`), which
doesn't fit the first shape at all — there's no "selected whole update"
per client when the combination is coordinate-wise. See
the changelog for the redesign that
produced this split, and `docs/EXTENDING.md` for how to add a new member
to either shape.

## How the project was built

Conflux was built as a sequence of scoped increments, each with a
written brief *before* implementation, and a
the STATUS record update *after* — the durable record that lets a new
session (or a new contributor) pick up without reconstructing context from
chat history. Every increment's Definition of Done required the full
workspace to build, lint clean, and pass its tests before moving on.

```mermaid
timeline
 title Conflux build history
 : Workspace scaffold : conflux-proto .proto + codegen
 : conflux-config (6-tier resolution) : conflux-registry (InMemoryRegistry)
 : conflux-store, conflux-selector : conflux-privacy, conflux-reputation
 : conflux-net dual-mode gRPC transport
 : conflux-buffer quorum/timeout : conflux-core FedAvg + robust scaffold
 : conflux-server — AppState, round pipeline, HTTP admin
 : conflux-node bridge + real Python stub client
 : Redis, Postgres, S3, observability, mTLS, accounting persistence, load test
 : Hybrid backend selection : Node-auth core + enforcement (allow-list, mTLS fingerprint)
 : auth (mTLS) enforcement : Production stub-client guard
 : RoundBuffer race fix : Strategy registry wiring (aggregator, selector)
 : robust aggregation family (Krum, Multi-Krum, Trimmed Mean, Median) : privacy_mechanism registry : stub-client poison mode
 : conflux-attacks — cited known-attack simulation + attack/defense tests
```

Each brief stated scope, explicit non-goals, inputs, deliverables,
and a test plan — and each one actually ran real verification, not
just `cargo build`: real over-the-wire gRPC tests from onward,
real Docker-backed Redis/Postgres/MinIO from, a real three-process
cross-language smoke test in, and a real concurrent-load test in
.

### Real gaps found and fixed along the way

Several genuine bugs surfaced during implementation — not hypothetical
edge cases, but things that actually broke a build or a test run — and
each was fixed rather than worked around, and documented:

| Found in | The gap | The fix |
|---|---|---|
| | `DeltaChunk` (the wire streaming format) never carried `num_samples`, so a reassembled `ClientDelta` had no input for FedAvg's weighting | Added `num_samples` to the `.proto`, repeated on every chunk |
| | `conflux-server`'s round loop treated *any* `run_round` error as fatal, including the ordinary "no submissions yet" case — the server would exit before a client could ever connect | Retry on `EmptyBatch`, keep `BudgetExhausted` fatal |
| | `RedisRegistry` tests shared one Redis key and raced under `cargo test`'s parallel execution; a first "fix" (a per-process counter) still collided across separate `cargo test` invocations against the same persistent Redis | Per-test key incorporating both a counter *and* the process id |
|/7b | `Registry`/`Store` trait methods were synchronous (fine for an in-process `HashMap`, impossible for real network I/O) | Converted both traits to native `async fn` (stable, no `async-trait` needed) |
| | A first draft of the mTLS rejection test asserted `connect()` itself fails — it doesn't, because the TLS handshake completes lazily | Assert on the first RPC failing instead, which is what actually proves rejection |
| | Adding `aws-sdk-s3` (pulls in `rustls`/`aws-lc-rs`) alongside `conflux-net`'s existing `ring`-based TLS caused a runtime crypto-provider panic — but only under `cargo test --workspace`, never `cargo test -p conflux-net` alone, because workspace-wide feature unification links both providers into every crate's test binaries | Aligned `conflux-net` on `tls-aws-lc` to match |

## Architecture decisions

| ADR | Decision |
|---|---|
| 0001 | Topology × mode are orthogonal config axes |
| 0002 | New algorithms extend a shared family base, not a whole trait from scratch |
| 0003 | One server process = one experiment |
| 0004 | One `.proto` schema serves both the network and local-loopback hops |
| 0005 | Real Python SDK is deferred; a stub client stands in for pipeline testing |
| 0006 | `Global` epsilon accounting ships first; `PerClient` is |
| 0007 | Every resolved config parameter logs its source — mandatory |
| 0008 | Shipped baselines are literal, cited implementations of specific papers |
| 0009 | Project name: Conflux |

## Current status

501 tests pass workspace-wide; `cargo fmt --check` and
`cargo clippy --workspace --all-targets` are both clean, the latter under
`-D warnings`. See [CHANGELOG.md](../CHANGELOG.md) for what shipped in
each release. See [`EXTENDING.md`](EXTENDING.md)
for how to add a new aggregator, selector, privacy mechanism, or attack
without touching `conflux-server`.

```mermaid
graph LR
 subgraph "Backends: env-var selectable"
 redis[("Redis<br/>RedisRegistry + RedisNodeAllowlist")]
 postgres[("Postgres<br/>PostgresStore +<br/>PrivacyRoundLog")]
 minio[("MinIO<br/>S3Store")]
 tls[("mTLS material<br/>server_tls_config")]
 end
 subgraph "Algorithms: registry selectable"
 fedavg["aggregator = fedavg / krum / multi_krum /<br/>trimmed_mean / median"]
 uniform["selector = uniform_random"]
 gaussian["privacy_mechanism = gaussian_clipping"]
 end
 mainrs["main.rs / AppState::connect"]
 redis --> mainrs
 postgres --> mainrs
 minio --> mainrs
 tls --> mainrs
 fedavg --> mainrs
 uniform --> mainrs
 gaussian --> mainrs

 style mainrs fill:#2f855a,color:#fff
```

Every backend, the TLS material, and all three spec §5
families (`aggregator`, `selector`, `privacy_mechanism`/11a/
11b) are now selected by env var / resolved config value alone — no code
change required to pick one, and choosing an aggregator name now covers
five real methods, not one. What's still hardcoded or missing: `main.rs`
itself doesn't yet expose `CONFLUX_AGGREGATOR`/`CONFLUX_SELECTOR`/
`CONFLUX_PRIVACY_MECHANISM`/`CONFLUX_ROBUST_BYZANTINE_FRACTION` env vars
the way it does for backend selection (spec §11 Open Item 2 — the
registry-construction path is proven end-to-end via `conflux-server`'s
own tests and `Overrides`, just not yet reachable from the compiled
binary without code), and config-*file* parsing (`Overrides` still comes
from env vars/CLI only, not a TOML file).
