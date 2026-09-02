# Changelog

All notable changes to Conflux FL are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this
project uses [Semantic Versioning](https://semver.org/spec/v2.0.0.html)
with the `0.` major deliberately load-bearing — see
[`docs/API_STABILITY.md`](docs/API_STABILITY.md) for what is and is not
promised before `1.0`.

> **`0.1.0` is the first release.** Conflux FL was built phase by phase
> without a changelog and without tags, so everything below is a single
> entry rather than a reconstruction of what shipped on which day.
> A dated, narrative engineering log of how the project got here —
> including the decisions that were reversed — is kept outside this
> repository. From the next release on, this file is maintained as changes
> land.

## [Unreleased]

### Added

- SCAFFOLD's **reference client** in the MNIST harness
  (`trainer_client.py --scaffold`): local steps follow `g − c_i + c`,
  `c_i` persists across rounds, `Δc_i` goes out on the wire.
  `run_demo.sh` enables it automatically when the aggregator is
  `scaffold`.
- **Per-client fairness metrics** in the MNIST eval client
  (`--shards`): per-round accuracy on every client's own distribution —
  min, std, full list — the axis `qfedavg`'s claim lives on and the
  pooled mean cannot see.
- The trainer announces the first nonzero `c` it receives: a SCAFFOLD
  run where `c` never arrives is indistinguishable from a correct one
  by accuracy alone.

### Fixed

- **`ScaffoldAggregator` discarded the seed round's control variates**,
  permanently breaking the `c = mean(c_i)` invariant the method's
  unbiasedness rests on — clients had already folded the matching
  `c_i⁺` into their own state. Found by the first end-to-end run the
  reference client made possible (held-out loss climbed monotonically),
  isolated on a deterministic quadratic where SCAFFOLD is provably
  exact (a constant bias equal to `mean(c_i)` after round one, to four
  decimals), fixed by folding the seed round's variates, pinned by a
  red-first test. On MNIST the same configuration went from diverging
  to the best result in its comparison.

## [0.1.0]

The first tagged release: a configurable, extensible, Rust-native
federated learning framework with a closed aggregation catalog, both
client SDKs, durable backends, and six tiers of stabilization behind it.

**Twenty-one server-side aggregation methods across five families**
(`averaging`, `robust`, `temporal`, `trusted`, `optimization`), plus
FedProx client-side. Every one is a literal, cited implementation of its
paper (ADR 0008).

### The foundation

The skeleton: every crate in the spec's dependency graph, wired into one
round pipeline that runs end to end across the language boundary.

#### Added

- Cargo workspace (edition 2024) with the crates spec §2 defines, in an
  acyclic dependency graph.
- **`conflux-proto`** — one protobuf schema serving both the
  server↔node network hop and the node↔client local hop (ADR 0004).
- **`conflux-config`** — layered resolution across topology and mode
  profiles, with every resolved parameter logging its source (ADR 0001,
  ADR 0007).
- **`conflux-registry`** — client lifecycle: register, heartbeat, evict.
- **`conflux-store`** — model checkpoint and experiment persistence.
- **`conflux-selector`** — client sampling (`UniformRandomSelector`,
  McMahan et al. 2017).
- **`conflux-net`** — dual-mode (push/pull) gRPC transport.
- **`conflux-buffer`** — quorum/timeout round staging.
- **`conflux-privacy`** — local DP clip-and-noise, epsilon accounting.
- **`conflux-reputation`** — opt-in cosine-similarity contribution
  scoring.
- **`conflux-core`** — the aggregation catalog and the family pattern
  (ADR 0002), with FedAvg as its first member.
- **`conflux-server`** — the full round pipeline and HTTP admin surface.
- **`conflux-node`** — the client-side bridge, with retry and backoff.
- A stub Python `ClientApp`, verified with a real three-process,
  cross-language smoke test.
- The first ADRs, phase briefs, and `docs/spec/conflux-spec-v1.md`.

### Durability, security, and the algorithm catalog

Durable backends, the robust and optimization families, security, and
six tiers of stabilization. This is where Conflux FL went from "the
pipeline runs" to "the pipeline survives contact with adversarial
input".

#### Added

- **Durable backends**: `RedisRegistry`, `PostgresStore`, `S3Store`,
  each tested against a real service rather than a mock. Converting
  `Registry` and `Store` to `async fn` in traits came with them.
- **The `robust` aggregation family** — Krum, Multi-Krum, Trimmed Mean,
  Median, FABA, Bulyan, Geometric Median, Median-of-Means,
  Divide-and-Conquer, FoolsGold, Centered Clipping. Each a literal,
  cited implementation of its paper.
- **The `optimization` family** — FedAvgM, FedAdagrad, FedAdam, FedYogi,
  q-FedAvg. Closed the framework's largest catalog gap.
- **FLANDERS** and **FLTrust**, the `temporal` and `trusted` families.
- **`conflux-trusted-reference`** — an optional sidecar process, so
  FLTrust's server-side training requirement does not put a training
  runtime inside `conflux-server` (ADR 0011).
- **`conflux-attacks`** — cited FL attacks, run against every shipped
  aggregator. Dev/test-only, `publish = false`, with a CI job enforcing
  that `conflux-server` never depends on it (ADR 0010).
- **Security**: mTLS for push mode, JWT verification (RS256/ES256, `sub`
  bound to the registering client), a node allow-list, and an
  authenticated HTTP admin API.
- **Differential privacy**: clip-and-noise, Rényi-DP epsilon accounting
  that survives restart, per-client accounting scope, and a client-side
  privacy transform applied by `conflux-node`.
- **Push mode** in `conflux-node`, `cross_silo`'s own default posture.
- **ADR 0012's optional wire fields** — `local_steps`, `local_loss`,
  `control_variate` — reassembled from chunks and proven
  backward-compatible at byte level.
- **Observability**: every operational decision point emits structured
  `tracing` events — buffer flush reason, reputation rejection,
  cumulative epsilon, node retry and backoff.
- **Config**: the strategy registry wired for all three spec §5 families,
  experiment-file parsing, and provenance logging for every resolved
  parameter.
- **Four end-to-end harnesses** on real models and datasets —
  `e2e_numpy_logreg`, `e2e_pytorch_mnist`, `e2e_pytorch_cifar10`,
  `e2e_pytorch_shakespeare`.
- **Releasability**: Apache-2.0, workspace-inherited metadata, declared
  MSRVs, a compose file, env-file management, and CI.
- `docs/API_STABILITY.md`, and the ADR series.

#### Fixed

- **Non-finite weights crashed or corrupted every aggregator.** One
  client sending `NaN` — four bytes — panicked six aggregators via
  `partial_cmp(...).expect("never NaN")`, taking the server down; the
  rest returned `NaN` into the checkpoint. Now rejected at a single
  chokepoint naming the client and the coordinate.
- **`num_samples` was unbounded**, so a client claiming `u64::MAX`
  samples made FedAvg's output exactly its own submission.
- **Seven `f32` overflow defects** across the catalog, all of the shape
  "accumulate, then normalize", where `inf * 0.0` produces `NaN` from
  finite, validation-passing input. One could permanently corrupt a
  stateful aggregator's stored reference. Fixed with `f64` intermediates
  and by folding `1/n` into each term.
- **The `RoundBuffer` lost-update race**, where a flag lived beside the
  lock rather than inside it.
- **`max_update_bytes` was bypassable** via `control_variate`, which
  relocated the flood one field to the left rather than bounding it.

#### Changed

- `conflux-node` gained a dependency on `conflux-privacy`, a deliberate
  deviation from spec §2 — spec §8's own sequence diagram requires the
  node to apply the mechanism.
- SIMD aggregation was built, benchmarked, and **rejected**: slower than
  the plain loop at every realistic model dimension, because the work is
  memory-bandwidth-bound and LLVM already auto-vectorizes it.

### Client SDKs, the last three methods, and what running them found

The release that closed the aggregation catalog and gave the client half
of the system a real SDK — and, in doing so, found that the wire fields
three of those methods depend on had never reached an aggregator.

#### Added

- **`conflux-client`** — a Rust-native `ClientApp` SDK, the fifteenth
  crate. Same contract as the Python SDK, field for field. Needed no new
  proto field, no server change, and no `conflux-node` change:
  `PullTransport` already *was* the client half of the local hop.
  Demonstrated on a real four-client federation with no Python process
  in the loop.
- **Python `ClientApp` SDK** (`python/conflux_client/app.py`) — connect,
  register, the fetch-until-a-new-round loop, placeholder-init
  detection, f32-aligned chunking, submit-with-retry, and treating a
  round that closed mid-training as ordinary. Replaced four separate
  copies of a `struct.pack`/`unpack` codec with one.
- **FedNova** (Wang, Liu, Liang, Joshi & Poor, 2020) — normalizes each
  client's progress by its local step count, so a client that trained
  longer does not silently get more pull.
- **SCAFFOLD** (Karimireddy, Kale, Mohri, Reddi, Stich & Suresh, 2020) —
  corrects local drift with `(c − c_i)`. The only method whose algorithm
  requires the server to send state *down* to clients.
- **FedProx** (Li, Sahu, Zaheer, Sanjabi, Talwalkar & Smith, 2018/2020) —
  implemented client-side, where its entire algorithm lives. Exposed as
  `--mu` on the MNIST harness. Deliberately not an aggregator name.
- `TaskResponse.control_variate` (`optional bytes`, field 4) — the
  downstream half of the control-variate plumbing, which did not
  previously exist. Backward-compatible: absent means "the configured
  aggregator maintains none".
- `Aggregator::control_variate()`, defaulting to `None`, so the other
  twenty methods are unaffected.
- `ClientApp::on_control_variate` on both SDKs, delivered before
  `train`, because the correction applies during local training.
- `CONFLUX_SCAFFOLD_NUM_CLIENTS` — SCAFFOLD's `N`, the *total* client
  population rather than the round's sample. Cannot be inferred from a
  batch.
- `AggregatorBuildError::ClientSideOnly` — naming a client-side method
  as an aggregator is a category error, not a typo, and now says so.
- CI job **`python-client`**: seven gates from `compileall` up to a real
  server/node/client federation whose pass condition is the *server's*
  round counter advancing. Verified to fail on a deliberately broken
  client, not only to pass on a good one.
- `python/conflux_client/ci_smoke.sh` — the end-to-end gate, runnable
  locally before distributing anything.
- `CHANGELOG.md` (this file).

#### Fixed

- **ADR 0012's optional fields never reached any aggregator.**
  `reencode_passing_deltas` rebuilt each `ClientDelta` ending in
  `..Default::default()`, resetting `local_steps`, `local_loss` and
  `control_variate` to `None` on the last hop before `aggregate`.
  q-FedAvg silently ran as FedAvg; FedNova and SCAFFOLD would have been
  dead on arrival. Found by running `qfedavg` end to end — three
  aggregators produced byte-identical accuracy at every round. Every
  unit test on both sides of that function passed, before and after.
- **The declared MSRV was wrong.** `rust-version = "1.85"` did not build
  eight of the twelve crates that promised it — `tonic`, `jsonwebtoken`
  and `time` each require 1.88. Corrected to **1.88**, verified on a real
  toolchain. `clippy::incompatible_msrv` cannot catch this: it checks
  the std APIs called, not whether dependencies build.
- **The demo health gate could pass against an unrelated process.** All
  four `run_demo.sh` scripts polled a hardcoded `127.0.0.1:8080` and
  accepted any 200. Now checks that the server process is alive, the
  port answers, and the answer is ours; port configurable via
  `CONFLUX_ADMIN_PORT`.
- The unknown-aggregator error hardcoded twelve names while twenty-one
  were registered. It is now generated from the strategy registry.
- Stale generated Python protobuf stubs, which had predated the ADR 0012
  fields entirely. Now regenerated and guarded by CI.

#### Removed

- **Deviation Stability Scoring and the research line it belongs to.**
  `DssAggregator` and `ClientDssDiagnostic` are gone from
  `conflux-core`, along with their tests and the three experiment
  runners in `conflux-attacks/examples/`. They moved to the separate
  `conflux-research` repository, which depends on these crates through
  their public API exactly as any third party would.

  This is a **breaking change** for anyone constructing `DssAggregator`
  directly. It was never in `build_aggregator`'s catalog, so no
  configuration could select it and no deployment is affected.

  The reason is the same one ADR 0008 states: this project ships
  literal, cited implementations of published methods. An unvalidated
  hypothesis has no citation to be faithful to, and while it sat in the
  catalog every document listing the catalog had to explain why one
  entry was different. It rejoins the `temporal` family when it is
  published, with its citation.

#### Changed

- `decode_and_validate` and `MAX_PLAUSIBLE_SAMPLE_COUNT` are now
  **public**. An `Aggregator` implemented outside this crate has to
  decode a batch and reject non-finite weights before touching it, and
  reimplementing that is how a new method acquires the `NaN`-handling
  defects this catalog already fixed. Exporting the chokepoint is
  cheaper than watching it be copied badly — and separating the research
  line is what proved an out-of-tree aggregator needs it.
- Minimum supported Rust version is **1.88** (was a declared-but-untrue
  1.85). `conflux-store` and `conflux-server` remain at 1.94.1 for
  `aws-sdk-s3`.
- `docs/AGGREGATION_LANDSCAPE.md` and ADR 0012 corrected: FedNova does
  **not** fit `AveragingWeighting`. Its update leaves an `x_t` term that
  vanishes only when every local step count is equal — which is exactly
  when FedNova degenerates to FedAvg. It is stateful.

[Unreleased]: https://github.com/conflux-fl/conflux-fl/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/conflux-fl/conflux-fl/releases/tag/v0.1.0
