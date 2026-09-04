# Changelog

All notable changes to Conflux FL are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this
project uses [Semantic Versioning](https://semver.org/spec/v2.0.0.html)
with the `0.` major deliberately load-bearing — see
[API stability](https://confluxfl.dev/reference/api-stability/) for what is and is not
promised before `1.0`.

> **`0.1.0` is the first release.** Everything below is a single entry:
> what the release contains, grouped by area, and the defects fixed on
> the way to it. From the next release on, this file is maintained as
> changes land.

## [Unreleased]

Nothing yet — `0.1.0` is the current release.

## [0.1.0] — 2026-09-03

The first release of Conflux FL — a Rust-native federated learning
framework with a closed, cited aggregation catalog, two client SDKs,
durable backends, real authentication, and the tooling to reproduce the
papers it implements. It all shipped as one release; the fuller account,
including every defect found on the way, is under **Details**.

### At a glance

#### Framework crates

| Crate | What it ships in 0.1.0 |
|---|---|
| `conflux-proto` | One protobuf schema for the network hop *and* the local loopback hop; optional wire fields (`local_steps`, `local_loss`, `control_variate`) proven backward-compatible at the byte level. |
| `conflux-config` | Layered resolution (builtin → topology → mode → experiment → env) with per-value provenance; custom profiles with `inherits`; startup **validation** of ranges *and* combinations, each finding attributed to the tier that set the value; a strategy registry carrying `citation`/`family`/`params` for every method. |
| `conflux-core` | **22 aggregation methods in 5 families**, each a literal cited implementation on a shared family pattern; hardened against `NaN`/`inf`/overflow and implausible sample counts; registry-driven catalog generation with a golden-file test. |
| `conflux-selector` · `conflux-buffer` · `conflux-reputation` | Uniform-random client sampling; quorum-or-timeout round staging (a lost-update race fixed); opt-in cosine-similarity contribution scoring. |
| `conflux-privacy` | Clip + Gaussian noise (Abadi et al.) and Rényi-DP epsilon accounting that survives a restart; a client-side transform the node applies before submitting. |
| `conflux-registry` · `conflux-store` | `RedisRegistry`; `PostgresStore` and `S3Store`; the node allow-list — every backend tested against a real service, not a mock. |
| `conflux-net` | Dual-mode (push / pull) gRPC transport; TLS builders for the server, mutual-TLS clients, and server-authenticated clients. |
| `conflux-server` | The round pipeline and an authenticated HTTP admin API; node admission by allow-list, JWT (RS256/ES256), or mTLS fingerprint; the provenance log and validation gate at startup; a per-method sidecar capability gate; structured `tracing` at every decision point; graceful shutdown; bounded submissions. |
| `conflux-node` | The client-side bridge — push or pull with retry/backoff, a per-client token/JWT, a three-way TLS posture (plaintext / server-auth / mutual) resolved from env, and optional local DP. |
| `conflux-trusted-reference` | Optional sidecar for the `trusted` family (FLTrust, Zeno) — a separate process, never a server dependency. |
| `conflux-client` | Rust-native `ClientApp` SDK with no Python in the loop, SCAFFOLD's client half, and an opt-in Burn example. |
| `conflux-attacks` | Cited FL attacks run against every aggregator; dev/test-only and structurally unshippable in the server. |
| `conflux-baselines` | The runner for the paper reproductions in `baselines/`. |

#### Aggregation catalog — 22 methods, 5 families

- **averaging** — FedAvg
- **robust** — Krum, Multi-Krum, Trimmed Mean, Median, FABA, Bulyan, Geometric Median (RFA), Median-of-Means, Divide-and-Conquer, FoolsGold, Centered Clipping
- **temporal** — FLANDERS
- **trusted** — FLTrust, Zeno
- **optimization** — FedAvgM, FedAdagrad, FedAdam, FedYogi, q-FedAvg, FedNova, SCAFFOLD
- plus **FedProx**, implemented client-side, where its whole algorithm lives

#### Baselines

- `baselines/` reproduces published papers as manifests — `baseline.toml`
  names a cataloged method, the paper's setup, and the expected result —
  driven by `conflux-baselines` (`list`, `run <name> --client
  python|rust`, `verify`).
- Four reproductions: **FedAvg, Krum, Trimmed Mean, Bulyan**, each
  runnable through a Python (PyTorch) and/or Rust (Burn) client edge. The
  method is validated against the registry before a run, and `verify`
  asserts every Rust edge against its committed number.

#### Clients

- **Python `ClientApp` SDK**, and four end-to-end harnesses on real
  models and real data — NumPy logistic regression, PyTorch MNIST,
  CIFAR-10, Shakespeare — all reporting the FedNova / q-FedAvg / SCAFFOLD
  fields, with per-client fairness metrics and a multi-seed sweep.
- **Rust `ClientApp` SDK** (`conflux-client`) — the same contract, field
  for field, demonstrated on an all-Rust federation; a Burn MLP example
  behind an opt-in feature.

#### Deployment and operations

- Four topologies from one codebase — `cross_silo`, `cross_device`,
  `crowdsource`, `edge` (with real, justified defaults) — selected by
  configuration.
- `deploy/run_client.sh` (a per-machine node + trainer launcher) and
  `deploy/allowlist.sh` (batch admission by id, token, or certificate
  fingerprint).
- Real backends, three authentication postures, DP accounting that
  survives restarts, and a pipeline that says out loud what it decided.

#### Documentation

- Manuals, guides, tutorials, a crate-by-crate reference, and
  Rust-concept deep dives live at **confluxfl.dev**; this repository
  keeps the code and development files.

#### CI and supply chain

- Jobs: rustfmt, clippy under `-D warnings`, tests against real
  Redis / Postgres / MinIO, MSRV **1.88**, the Python client end to end,
  rustdoc with warnings denied, `cargo deny` (advisories + licenses),
  dependency isolation (the server never depends on the attacks crate or
  the sidecar), and a secrets scan.
- Four real vulnerabilities removed from the tree on `cargo deny`'s first
  run; Dependabot for cargo, pip, and GitHub Actions.

### Details

### The foundation

The skeleton: every crate in the dependency graph, wired into one
round pipeline that runs end to end across the language boundary.

#### Added

- Cargo workspace (edition 2024) with the framework crates in an
  acyclic dependency graph.
- **`conflux-proto`** — one protobuf schema serving both the
  server↔node network hop and the node↔client local hop.
- **`conflux-config`** — layered resolution across topology and mode
  profiles, with every resolved parameter logging its source.
- **`conflux-registry`** — client lifecycle: register, heartbeat, evict.
- **`conflux-store`** — model checkpoint and experiment persistence.
- **`conflux-selector`** — client sampling (`UniformRandomSelector`,
  McMahan et al. 2017).
- **`conflux-net`** — dual-mode (push/pull) gRPC transport.
- **`conflux-buffer`** — quorum/timeout round staging.
- **`conflux-privacy`** — local DP clip-and-noise, epsilon accounting.
- **`conflux-reputation`** — opt-in cosine-similarity contribution
  scoring.
- **`conflux-core`** — the aggregation catalog and the family pattern,
  with FedAvg as its first member.
- **`conflux-server`** — the full round pipeline and HTTP admin surface.
- **`conflux-node`** — the client-side bridge, with retry and backoff.
- A stub Python `ClientApp`, verified with a real three-process,
  cross-language smoke test.

### Durability, security, and the algorithm catalog

Durable backends, the robust and optimization families, security, and
six rounds of hardening. This is where Conflux FL went from "the
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
  runtime inside `conflux-server`.
- **`conflux-attacks`** — cited FL attacks, run against every shipped
  aggregator. Dev/test-only, `publish = false`, with a CI job enforcing
  that `conflux-server` never depends on it.
- **Security**: mTLS for push mode, JWT verification (RS256/ES256, `sub`
  bound to the registering client), a node allow-list, and an
  authenticated HTTP admin API.
- **Differential privacy**: clip-and-noise, Rényi-DP epsilon accounting
  that survives restart, per-client accounting scope, and a client-side
  privacy transform applied by `conflux-node`.
- **Push mode** in `conflux-node`, `cross_silo`'s own default posture.
- **Optional per-method wire fields** — `local_steps`, `local_loss`,
  `control_variate` — reassembled from chunks and proven
  backward-compatible at byte level.
- **Observability**: every operational decision point emits structured
  `tracing` events — buffer flush reason, reputation rejection,
  cumulative epsilon, node retry and backoff.
- **Config**: the strategy registry wired for all three strategy families,
  experiment-file parsing, and provenance logging for every resolved
  parameter.
- **Four end-to-end harnesses** on real models and datasets —
  `e2e_numpy_logreg`, `e2e_pytorch_mnist`, `e2e_pytorch_cifar10`,
  `e2e_pytorch_shakespeare`.
- **Releasability**: Apache-2.0, workspace-inherited metadata, declared
  MSRVs, a compose file, env-file management, and CI.
- The [API stability](https://confluxfl.dev/reference/api-stability/) policy.

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

- `conflux-node` gained a dependency on `conflux-privacy`: the round
  sequence requires the node to apply the client-side mechanism, and it
  cannot without reaching it.
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

- **The optional per-method fields never reached any aggregator.**
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
- Stale generated Python protobuf stubs, which had predated the optional
  per-method fields entirely. Now regenerated and guarded by CI.

#### Removed

- **An uncited aggregator removed from the framework.** An unpublished
  aggregator and its diagnostic type are gone from `conflux-core`, along
  with their tests and the three experiment runners in
  `conflux-attacks/examples/`.

  This is a **breaking change** for anyone constructing that aggregator
  directly. It was never in `build_aggregator`'s catalog, so no
  configuration could select it and no deployment is affected.

  The reason is the project's own rule: it ships literal, cited
  implementations of published methods. An unpublished method has no
  citation to be faithful to, and while it sat in the catalog every
  document listing the catalog had to explain why one entry was
  different. It can rejoin the `temporal` family once it is published,
  with its citation.

#### Changed

- `decode_and_validate` and `MAX_PLAUSIBLE_SAMPLE_COUNT` are now
  **public**. An `Aggregator` implemented outside this crate has to
  decode a batch and reject non-finite weights before touching it, and
  reimplementing that is how a new method acquires the `NaN`-handling
  defects this catalog already fixed. Exporting the chokepoint is
  cheaper than watching it be copied badly; the first aggregator built
  outside this crate is what proved it needs to be public.
- Minimum supported Rust version is **1.88** (was a declared-but-untrue
  1.85). `conflux-store` and `conflux-server` remain at 1.94.1 for
  `aws-sdk-s3`.
- A design correction: FedNova does **not** fit `AveragingWeighting`. Its update leaves an `x_t` term that
  vanishes only when every local step count is equal — which is exactly
  when FedNova degenerates to FedAvg. It is stateful.

### Profiles, validation, baselines, and the Rust training edge

The work after the catalog closed: the last open configuration items,
the twenty-second method, the reproductions, the Burn client, real node
credentials, and the move of the documentation to its own site.

#### Added

- **Baselines** — `baselines/` reproduces published papers as
  manifests (`baseline.toml`: a cataloged method + the paper's setup +
  the expected result) and a `conflux-baselines` runner (`list`,
  `run <name> --client python|rust`, `verify`). Four reproductions ship —
  FedAvg, Krum, Trimmed Mean, Bulyan — each with a Python (PyTorch) and/or
  Rust (Burn) client edge; the method is validated against the strategy
  registry before anything runs, and `verify` asserts every Rust edge
  against its committed number.
- **Rust-native Burn client** — `conflux-client`'s `burn_mlp` example:
  a real Burn MLP `ClientApp` (ndarray CPU backend) that drives the
  *real* catalog aggregators in-process; strictly opt-in behind the
  `burn` feature so the default build never compiles it.
- **Node credentials and TLS** — `conflux-node` now presents a
  per-client `CONFLUX_NODE_AUTH_TOKEN` (token/JWT) at registration and
  resolves a three-way TLS posture from `CONFLUX_TLS_*` — plaintext,
  server-authenticated (`SERVER_CA_PATH` + `DOMAIN`), or mutual (all
  four) — failing loudly on any other subset. `conflux_net::tls` gained
  `client_tls_config_server_auth`.
- **`deploy/`** — `run_client.sh` (a per-machine node + trainer
  launcher) and `allowlist.sh` (batch admission by id, token, or cert
  fingerprint).
- **Documentation moved to confluxfl.dev** — manuals, guides, and
  tutorials now live on the documentation site; this repository keeps
  code and development files. `docs/` retains only the generated
  aggregation catalog, a golden-file test artifact.

- **Registry-driven catalog generation** — a `catalog` example
  (`cargo run -p conflux-core --example catalog`, Markdown or `--format
  json`) emits the aggregation catalog's facts (method, family,
  citation, parameters) straight from the strategy registry, and a
  golden-file test fails CI if the committed
  `docs/AGGREGATION_CATALOG.generated.md` drifts from it. This is what
  the `StrategyEntry` metadata was for: the count and citations that
  went stale in the docs repeatedly can no longer do so silently.

- **Registry metadata**: `StrategyEntry` now carries `citation`,
  `family`, and `params` for every registered aggregator, selector, and
  privacy mechanism, plus a `conflux_config::entries()` reader. A test
  makes the cite-the-paper rule a build-time fact — a method registered
  without a citation naming authors and a year fails CI — and the metadata is the
  no-drift source a generated catalog or a CLI `describe` would read.
- **The three remaining PyTorch/numpy harnesses migrated onto the
  `ClientApp` SDK** (numpy-logreg, CIFAR-10, Shakespeare). Each drops
  its hand-rolled connect/register/poll/chunk/submit loop and its own
  copy of the f32 codec, and — the functional payoff — now reports
  `local_steps` and `local_loss`, so every harness can drive FedNova
  and q-FedAvg instead of silently running FedAvg whatever the server
  was configured for. The numpy `run_demo.sh` now forwards
  `CONFLUX_FAIRNESS_Q` so that capability is reachable.
- **`--trainer-seed`** on the three PyTorch trainers: reseeds torch's
  RNG *after* the shared deterministic model init, so a multi-seed
  sweep varies real SGD sampling instead of replaying one trajectory
  per shard. `run_fairness_comparison.sh` derives a
  per-client seed from `(sweep seed, client index)` and its
  known-limitation note is retired.

- **SCAFFOLD in the Rust client example** (`logreg.rs --scaffold`) —
  the same client half the Python harness ships, field for field:
  corrected local steps `g − c_i + c`, persistent `c_i`, `Δc_i` on the
  wire, and the first-nonzero-`c` announcement. Proven on an all-Rust
  federation (server + 4 nodes + 4 Rust clients, `aggregator =
  scaffold`): `c` delivered, 0.68 local-only → 0.996 federated, one
  round faster to 0.996 than plain FedAvg on the same problem.
- SCAFFOLD's **reference client** in the MNIST harness
  (`trainer_client.py --scaffold`): local steps follow `g − c_i + c`,
  `c_i` persists across rounds, `Δc_i` goes out on the wire.
  `run_demo.sh` enables it automatically when the aggregator is
  `scaffold`.
- **Per-client fairness metrics** in the MNIST eval client
  (`--shards`): per-round accuracy on every client's own distribution —
  min, std, full list — the axis `qfedavg`'s claim lives on and the
  pooled mean cannot see. Both trainers announce the first nonzero `c`
  they receive: a SCAFFOLD run where `c` never arrives is otherwise
  indistinguishable from a correct one by accuracy alone.
- CI job **`docs`**: `cargo doc --no-deps --workspace` with
  `RUSTDOCFLAGS="-D warnings"`. Intra-doc links break silently — the
  code compiles, the docs render, the link is just dead — and this is
  the only gate that notices. Its first local run found five broken
  links across four crates, all fixed.

- `run_fairness_comparison.sh` in the MNIST harness — the multi-seed
  SCAFFOLD / q-FedAvg / FedAvg sweep with per-client fairness metrics,
  one CSV row per (arm, seed), and a mean ± std summary that says out
  loud when a difference is inside the noise.
- **Configuration validation** — `ResolvedConfig::validate()`, run at
  server startup after the provenance log. Range checks (zero
  timeouts/TTLs/quorums/byte ceilings, cosine bounds, non-finite
  numerics, DP's `0 < δ < 1`) and cross-parameter combination checks
  (a Byzantine-majority fraction with a batch-only robust method, a
  negative `clip_radius` under `centered_clipping`, per-method
  positivity for the optimizer knobs, `scaffold_num_clients < quorum`).
  Errors refuse to start; warnings start out loud — including
  "`noise_multiplier` has no effect because `clip_norm = 0`" and a
  quorum below Krum's `n ≥ 2f + 3` / Bulyan's `n ≥ 4f + 3`, with the
  arithmetic filled in. Every finding names the tier that supplied the
  value, in the same phrasing as the startup log, so a bad number in a
  profile file is attributed to that file. All findings are collected
  in one pass. Bounds are deliberately conservative: mathematical facts
  and paper-stated requirements only, never taste.

- **Custom profiles with `inherits`** — topology and mode profiles
  defined in TOML, extending a base and overriding only what differs.
  `CONFLUX_TOPOLOGY=hospital_silo`
  loads `profiles/hospital_silo.toml`; chains may pass through other
  profiles and must end at a builtin. Provenance credits the chain link
  that actually set each value (`topology profile "hospital_silo →
  cross_silo"` for an inherited one). The rules are enforced with
  specific startup errors: wrong-axis keys are told which file they
  belong in (the two axes own disjoint sets), misspelled keys get a
  "did you mean", cycles are printed as the chain, builtin names cannot
  be shadowed, and unknown profile names list what exists.
  `conflux-config` gains `resolve_with_profiles`,
  `load_topology_profile`, `load_mode_profile`, `TopologyProfile`,
  `ModeProfile`, `ProfileError`; `resolve` is unchanged and now wraps
  the new path.

- **Zeno** (Xie, Koyejo & Gupta, 2019) — the twenty-second method, and
  the second member of the `trusted` family. Ranks each candidate by a
  suspicion score (the sidecar's held-out improvement minus
  `ρ·‖update‖²`), drops the `b` lowest, averages the rest unweighted.
  Consumes the sidecar's `ScoreUpdates` RPC, which had shipped with the
  sidecar and gone unused; the server calls it after the buffer flushes,
  because Zeno's scores — unlike FLTrust's reference — can only exist
  once the batch does. Scores are consumed on use, so a round that was
  never scored fails loudly instead of ranking this batch with the
  previous batch's numbers. Startup now gates each sidecar capability by
  what the configured method actually consumes. `CONFLUX_ZENO_RHO`
  configures `ρ` (builtin `0.0005`, the paper's own value). Proven over
  the real gRPC hop: a poisoned client is dropped and the honest mean
  survives.
- CI job **`deny`**: `cargo deny check` — RustSec advisories, yanked
  crates, a license allow-list, and registry provenance — plus a
  `deny.toml` documenting every allowance.
- Dependabot for cargo, pip, and GitHub Actions; a CI badge in the
  README.

#### Fixed

- A typo'd `CONFLUX_TOPOLOGY` (e.g. `cros_silo`) used to fall back to
  `cross_device` **silently** — a correctly-logged, wrong deployment.
  It is now a startup error listing the builtins and every profile the
  profile directory actually contains. Same for `CONFLUX_MODE`.
- **Four real vulnerabilities and a yanked crate**, found by `cargo
  deny`'s first local run: `aws-sdk-s3`'s default `rustls` feature was
  dragging the *legacy* rustls-0.21/h2-0.3 stack (RUSTSEC-2026-0098,
  -0099, -0104, -0258) into the tree alongside the modern TLS stack the
  SDK actually uses. Disabling that one default feature removes the
  vulnerable stack entirely; `chacha20` was yanked and is bumped.
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

#### Changed

- **The `edge` topology has real defaults** instead of mirroring
  `cross_device`: `auth = mtls` (an edge fleet is operator-provisioned,
  so it can carry a client certificate from day one — and its devices
  are the most physically exposed, so the stronger identity is the one
  it most needs), `round_timeout_secs = 900` (MCU/SBC-class hardware,
  not phone NPUs), `min_reputation_score = 0.0` (a closed,
  operator-owned population — gating defaults track how open the
  population is), `client_registry_ttl = 3600` (stable membership,
  unstable links). Each field's justification is in the source.
  **Behavior change** for `edge` deployments relying on the old
  mirrored values; a profile with `inherits = "edge"` overriding them
  restores any of the old numbers.
- **Zeno++ declined, with a reason**: it is fully *asynchronous* SGD —
  an execution model, not an aggregation rule — and cannot be expressed
  through `Aggregator::aggregate(batch)` without inventing a batched
  variant the paper never defined. It becomes the first candidate if
  an async pipeline mode ever exists.

#### Fixed — CI and supply chain

- CI: `cargo deny` gained scoped ignores for two *unmaintained* advisories
  riding in only via the optional `burn` tree (`paste`, `bincode`) and an
  allowance for MPL-2.0 (`option-ext`, same path); Rust 1.98's new
  `clippy::chunks_exact_to_as_chunks` lint is satisfied with `as_chunks`
  on the f32 decode paths; the MinIO service container moved to
  `bitnamilegacy/minio` after Bitnami emptied `bitnami/minio`; evnx is
  installed via `gh release download` and gated on high/medium-confidence
  findings; the Redis/Postgres integration tests now read the
  `CONFLUX_TEST_*` URLs instead of hardcoding the dev container ports.

[Unreleased]: https://github.com/conflux-fl/conflux-fl/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/conflux-fl/conflux-fl/releases/tag/v0.1.0
