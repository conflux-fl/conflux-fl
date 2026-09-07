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

### Fixed

- **`cflux init` now prints the whole sequence, not just the next step.**
  It scaffolded two profiles and told you how to `config check` them —
  then the obvious next command, `cflux server start`, resolved the
  builtin `cross_device` / `research` defaults instead. Nothing was
  broken: profiles are selected *by name*, and writing one does not
  activate it. But a missing selection is a silent fallback rather than
  an error, so the result was a running server that was not the
  deployment just scaffolded.

  `init` now names that outright — "these are selected by name, not by
  being the only profiles here" — prints the `export` line, and lists
  every step through `server start`.

- **A refused bind now names the flag that moves it.** `cannot bind
  127.0.0.1:8080: Address already in use` said what happened and nothing
  about what to do, and it is the first failure a new deployment hits,
  because 8080 is a popular port. It now points at `--http-addr` and
  `CONFLUX_HTTP_ADDR`.

### Added

- **`curl -fsSL https://confluxfl.dev/install.sh | sh`.** Installing
  `cflux` was four manual steps — pick the right target triple, download
  the archive and its checksum, verify with whichever of `sha256sum` or
  `shasum` your platform has, clear macOS quarantine, then edit your
  `PATH`. It is now one line.

  Piping a script into a shell is a real trust decision, so the script
  tries to earn it. It is POSIX `sh` and short enough to read before
  running (`… | less`). It **verifies the checksum before unpacking**,
  because unpacking is already trusting the bytes, and it removes the
  temp directory on every exit path including that failure, so a
  corrupted archive is never left behind to be found and trusted later.
  It needs no root and writes nothing outside the install directory.

  It refuses rather than guesses: musl, non-x86_64 Linux, a glibc older
  than the build's 2.34 floor, and Windows each get a specific message
  naming the alternative, instead of a binary that will not start.

  The canonical copy lives here and is linted in CI (`sh -n` plus
  shellcheck); `confluxfl.dev/install.sh` serves a copy of it, and each
  release now attaches `install.sh` as an asset — so if the served copy
  ever drifts, the release asset is the authoritative one.

### Added

- **Bulyan reproduces on the Python edge**, and a manifest can now state
  the Byzantine fraction its method should assume
  (`[scenario] byzantine_fraction`). The two are the same change.

  Bulyan's guarantee holds only for `n >= 4f + 3`. The runner pinned the
  fraction at 0.3 for every baseline, which makes `f` 30% of `n` — and
  `n >= 1.2n + 3` has no solution, so no client count could satisfy the
  precondition. The manifest recorded this as "would fail fast", but that
  is not what the implementation does: `byzantine_count` floors and
  clamps and `theta` saturates, so at 8 clients it computes `f = 2`,
  selects 4 of 8, and runs *quietly* outside the regime the paper
  describes. A reproduction running where its paper makes no claim is
  not a reproduction.

  At `byzantine_fraction = 0.125` with 8 clients, `f = 1` and `n >= 7`
  holds, and the Python edge measures **0.918–0.925** across four runs
  against a target of `0.91 ± 0.05`. That range is not seed noise —
  every run is identically seeded. The order client updates arrive in is
  not seeded and cannot be, and Bulyan is the catalog's most
  order-sensitive method: an iterative selection whose tie-break takes
  the first minimum, then a trimmed mean summing floats in that order.
  A federation is a distributed system, and its tolerances have to
  absorb that. Unset, the field keeps the framework-wide 0.3, so no existing
  baseline moves. Bulyan's `[experiment]` also changes from
  `synthetic`/`non-iid` — neither of which the harness registry knows —
  to the `mnist`/`iid` recipe the other baselines use, following the
  existing convention that `[experiment]` describes the Python edge while
  the Rust edge's comment records its synthetic problem.

- **`cflux checkpoint list` and `cflux checkpoint show <round>`.** What a
  durable store actually holds, read directly rather than through the
  server — so it answers while the server is down, which is when the
  question is usually asked.

  `list` names gaps in the sequence, because a run that checkpointed
  rounds 1–40 and then 45 lost five somewhere and nothing else reports
  it. `show` gives a round's parameter count, L2 norm and range, flags
  the all-zero placeholder the server hands out before any client has
  trained, and **exits non-zero when any weight is NaN or infinite** —
  one such value poisons every client that resumes from that checkpoint,
  and the range alone would not reveal it.

  An in-memory store is answered rather than attempted: those
  checkpoints live inside the server's process and no separate process
  can read them, so the command says so and names the variable that
  changes it. That is exit `1` — a negative answer — not exit `2`, which
  a script must be able to tell apart from a misconfiguration.

- **`Store::list_checkpoints` and `Store::load_checkpoint`**, across
  every backend. The enumeration already existed inside each backend's
  `load_latest_weights`, which found the highest round and then loaded
  it; this lifts it out rather than writing it again. `load_latest_weights`
  stays a single query on Postgres rather than becoming
  enumerate-then-fetch — the round loop calls it every round, and a CLI
  convenience should not slow the server's hot path.

- **Multi-seed sweeps, and the per-client metrics that make a fairness
  claim measurable.** `sweep` takes `--seeds`, and every combination runs
  once per seed. The seed reaches two places, and both matter: the data
  preparation draws its subsample and partition from it, and each trainer
  gets one derived from it, so seeds differ in their SGD trajectory and
  not only in their shards. Vary just the partition and a "multi-seed"
  sweep replays one trajectory per shard, which understates the spread it
  exists to measure.

  The evaluator is now given every client's shard, so each round reports
  `client_acc_min` and `client_acc_std` beside the pooled number. A
  pooled mean cannot see who it is failing, and the fairness methods make
  their claim about that distribution rather than about its average.

  `summarize_sweep.py` reports mean and standard deviation across seeds
  with `n_seeds` beside every row, instead of a single number that reads
  like a measurement. Files written before any of this still summarize;
  they report one seed and an empty spread.

  This restores what `run_fairness_comparison.sh` did before the `e2e_*`
  harnesses were retired in `0.3.0` — the one capability that release
  removed.

### Changed

- **The evaluator now sees every round.** It observed roughly every
  *other* one, and the pattern was regular enough to name the cause:
  fetching and scoring shared a loop, one score takes about as long as
  one round, and the server moved on while the evaluator was busy.
  `FetchTask` only ever returns the current round, so a missed round was
  unrecoverable — a convergence curve permanently missing half its
  points.

  Fetching now runs on its own thread and buffers each new round's
  weights; scoring drains that buffer. The cheap part keeps up with the
  server, and the expensive part is allowed to lag. Measured on a
  ten-round run: ten of ten rounds, contiguous, where the same run
  previously recorded four.

  A side effect worth knowing: the evaluator now stops at exactly the
  round it was asked for, instead of overrunning by one while it waited
  to observe enough distinct rounds. Baselines therefore report their
  final round rather than one past it, and the manifests are re-measured
  accordingly — FedAvg **0.927**, Trimmed Mean **0.916**, Krum unchanged
  at **0.890**. All three still reproduce inside their tolerances, and
  the numbers now describe the round the manifest actually asks for.

- **Every client now seeds its own sampling.** A trainer previously drew
  batches from torch's global stream after the shared model init, so five
  clients shared one sequence; each now reseeds from a value derived from
  the run's seed and its own index. This is what makes a multi-seed sweep
  measure anything, and it is the more defensible behavior regardless —
  five clients drawing from one stream is not what five independent
  clients should mean.

  It moves the reproduced numbers slightly, so all three baselines were
  re-measured and their manifests record both figures: FedAvg 0.924 →
  **0.928**, Trimmed Mean 0.918 → **0.923**, Krum unchanged at **0.890**
  (it selects one update per round, so a different draw across the five
  trainers largely washes out). Every baseline still reproduces well
  inside its tolerance.

### Fixed

- **The `x86_64-apple-darwin` release binary could never be built.** Its
  matrix row asked for a `macos-13` runner, which GitHub has retired, so
  the job queued for a runner that no longer exists rather than failing
  — `v0.3.0` shipped three of its four archives. It is now
  cross-compiled from the arm64 runner, which is safe in a way the musl
  cross is not: Apple ships both slices in one SDK, so clang targets
  x86_64 from arm64 with no third-party toolchain. The Intel images that
  replaced `macos-13` are larger runners rather than standard ones.

- **Release binaries can now be built for a tag that already exists.**
  The workflow takes a `workflow_dispatch` input naming a tag, checks
  that tag out and uploads into its existing release. Without it, the
  only ways to recover a failed binary build were to move a tag — a lie
  about what shipped, and blocked by the ruleset — or to leave the
  release incomplete.

## [0.3.0] — 2026-09-07

Evaluating Conflux FL no longer starts with installing a Rust
toolchain, and reproducing a paper no longer starts with copying a
trainer. `cflux` starts the real server and the real node, prebuilt
binaries ship with every release, and one shared harness replaced the
four near-identical ones behind the baselines.

### Added

- **Prebuilt `cflux` binaries on every release.** The release workflow
  now builds `cflux` for Linux, macOS (both architectures) and Windows
  and attaches each with a `.sha256`, so evaluating Conflux FL no longer
  starts with installing a Rust toolchain — the barrier the CLI exists
  to remove. Built natively on each platform rather than cross-compiled:
  `cflux` links a C crypto library, and cross-compiling or statically
  linking that against musl is a standing source of toolchain pain.
  The README documents installing from a release or from the tag.

- **`cflux server start` and `cflux node start`.** Each hands off to the
  same `run_from_env` its binary calls, so the CLI starts the real
  server and the real node rather than a second implementation of
  either. Flags are a convenience over the environment those functions
  already read, which is why a flag and its `CONFLUX_*` counterpart
  cannot mean different things.

- **A shared training harness** (`baselines/_harness/`). Every Python
  edge now runs on one library: `torch_model` for everything that does
  not depend on the architecture (flatten, unflatten, local SGD with
  FedProx and SCAFFOLD, evaluation), plus registries of models (`mlp`,
  `cnn`, `gru`, `logreg`), datasets (`mnist`, `cifar10`, `shakespeare`)
  and partitions (`iid`, `dirichlet`, `shard`). A baseline names a
  *recipe* — the `[experiment]` table it already carried — instead of
  pointing at an example directory, and `conflux-baselines` runs the
  federation itself: server, one node per client, one trainer per node,
  and an evaluator, with cleanup on every failure path rather than a
  shell trap. It refuses to start when a port it needs is already held
  and quotes the failing child's own log, so a clash reports itself
  rather than surfacing a minute later as a transport error.

- **`conflux-baselines sweep`.** A grid of (aggregator, split, attack)
  combinations, each one a real federation, appended to a JSONL file as
  one record per round — the comparative counterpart to a baseline's
  single reproduced number. It replaces a shell-driven sweep that lived
  inside one of the `e2e_*` example directories and parsed accuracy back
  out of that script's stdout. The centralized bar each combination is
  measured against depends only on the model, the data and the step
  budget, so it is now computed once per sweep instead of recomputed
  identically for every combination.

### Changed

- **`conflux-server` and `conflux-node` are now libraries with a shell
  around them.** Each binary's startup moved into
  `run_from_env`, and every failure that was a panic is a `ServeError`
  or `RunError` the caller decides about. The binaries still print a
  message and exit non-zero, exactly as before; `conflux-server`'s
  `main.rs` went from 532 lines to 86.

- **Trimmed Mean's reproduction target now comes from its paper.** It
  was `0.86 ± 0.10`, marked provisional. Tracing where 0.86 came from
  found Yin et al. §7 Table 2 — 86.9% for trimmed mean on logistic
  regression at m=40, β=0.05 — which is not the experiment this baseline
  runs. Ours is 5 clients with 1 attacker on a model with hidden units,
  which is Table 3's regime, where the paper reports 90.7%; we measure
  0.918. The target is now `0.91 ± 0.05`, tight enough to catch a
  regression, and the manifest records both the paper's figure and ours.
  Krum's target is unchanged, but its provenance now cites the shared
  harness (0.890) rather than an example directory that no longer exists
  here.

- **Every per-topology default now records why it is what it is.** The
  numbers themselves are unchanged — on inspection they form a coherent
  ladder — but each now carries its reasoning inline: what shape of
  deployment it is calibrated to, and which of them come from the
  literature (Kairouz et al. 2021's taxonomy; Bonawitz et al. 2019's
  production cross-device system) rather than from engineering
  judgment. The claim that they were placeholders was the stale part.

### Removed

- **The four `e2e_*` example harnesses**, and `benchmark.py` with them.
  They were four near-identical copies of one training loop, and
  `baselines/_harness/` plus `conflux-baselines sweep` now do everything
  they did from a single implementation. They are not deleted but
  archived outside this repository, because several published numbers
  were measured on them and a manifest citing such a measurement should
  be able to point at the code that produced it.

  `python/conflux_client/examples/` is gone as a directory. If you drove
  a trainer from there, `python3 -m _harness.trainer --model <name>` from
  `baselines/` is the replacement, and it takes the same
  `--address/--client-id/--shard/--rounds` arguments. The harness's
  Python dependencies, which used to live in each example's own
  `requirements.txt`, are now `baselines/requirements.txt`.

## [0.2.0] — 2026-09-06

A command line for people who use the framework rather than build it,
paper reproductions that cannot silently drift from the manifests that
define them, and an epsilon that reflects what a deployment actually
spends.

### Added

- **`cflux`, the Conflux FL command line** (`crates/cflux`). The Rust
  toolchain is for a developer *of* the framework; `cflux` is for a user
  of it, and it answers — without starting a server — the questions an
  operator has before a deployment exists:

  | Command | What it answers |
  |---|---|
  | `catalog list` / `catalog describe <name>` | what methods exist; one method's family, paper, parameters, and whether it needs the trusted-reference sidecar |
  | `config resolve` | every resolved parameter and the tier that set it |
  | `config check` | the same, plus range and combination validation |
  | `init` | scaffolds a topology profile, a mode profile, and (with `--docker`) a compose file for the durable backends |
  | `doctor` | every check the server fail-fasts on, at once |
  | `version` | this binary's version and the framework version it embeds |

  Every command takes `--format pretty|json`, ends its `--help` with a
  guide link, and uses stable exit codes: `0` ok, `1` a negative answer
  (an error finding, an unknown name), `2` the command could not run.
  `doctor` also has `--format github-actions`, which emits findings as
  annotations on a pull request rather than lines in a job log.

  `init` writes every key an axis owns, commented at its inherited
  value, so nothing tunable is invisible and scaffolding changes no
  behavior until a line is uncommented. `doctor` calls the server's own
  startup functions rather than an imitation of them — backend selection
  against the mode, the TLS posture, the JWT key requirement, and the
  sidecar's `Describe` handshake — so a check that passes there is the
  check that passes at startup. Backend probes are TCP-level and say so:
  a probe that authenticated would need the CLI to hold a deployment's
  secrets, and one that wrote anything would make a diagnostic tool a
  source of side effects. Suggestions are printed, never applied.

  `cflux` declares Rust **1.94.1** rather than the workspace's 1.88,
  because it links `conflux-server`. CI's isolation job asserts it never
  links `conflux-attacks`.

- **A generated "Reproduced papers" table.** `conflux-baselines table`
  emits the table in `baselines/README.md` from the manifests
  (`--write` fills a fenced region, `--check` exits non-zero when it is
  stale), and a golden-file test fails CI on drift — the guarantee the
  aggregation catalog already had. The catalog gained a **Reproduced
  by** column, and a `baselines` field in its JSON, linking each method
  to the baselines that reproduce it.

- **Shared library surfaces**, so the CLI and the server cannot disagree
  about a deployment. `conflux_config::{overrides_from_env,
  topology_profile_named, mode_profile_named}` and
  `conflux_server::{backend_selection_from_env, tls_material_from_env,
  jwt_key_from_env, tls_paths_present, trusted_reference_addr}` — the
  `CONFLUX_*` and profile reading that lived inside the server binary,
  now fallible library functions both callers share. The binary still
  fails fast on any error, with the same messages.

### Changed

- **Epsilon accounting now credits privacy amplification by
  subsampling.** `RdpAccountant` computed per-round RDP for the
  non-subsampled Gaussian mechanism, charging a deployment that exposes
  a `sample_rate` fraction of its clients as though it had exposed all
  of them. It now uses the Sampled Gaussian Mechanism's closed form at
  integer Rényi orders (Mironov, Talwar & Zhang, 2019), falling back to
  the old bound where that form does not apply — a non-integer order, a
  rate outside `(0, 1)`, or a result that is not finite. Every fallback
  is an upper bound on the truth, so epsilon can still only be
  over-reported, never under-reported. Validated against numerical
  integration of the Rényi divergence definition, which agreed with the
  closed form to within 5e-12 relative across nine configurations.

  **Reported epsilon drops, sometimes by a lot.** At `noise_multiplier =
  1.0` and `sample_rate = 0.1`: 17.7 → 3.9 after 8 rounds, 671 → 28.5
  after 1,000. A budget that was exhausted at round 8 no longer is. No
  configuration changes; a deployment simply stops being charged for
  exposure it never had.

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

[Unreleased]: https://github.com/conflux-fl/conflux-fl/compare/v0.3.0...HEAD
[0.3.0]: https://github.com/conflux-fl/conflux-fl/releases/tag/v0.3.0
[0.2.0]: https://github.com/conflux-fl/conflux-fl/releases/tag/v0.2.0
[0.1.0]: https://github.com/conflux-fl/conflux-fl/releases/tag/v0.1.0
