# Using Conflux

A practical guide to building, running, and testing Conflux. For *why*
things are built the way they are, see [ARCHITECTURE.md](ARCHITECTURE.md).
For the authoritative design, see
`spec/conflux-spec-v1.md`.

## Prerequisites

- **Rust** (edition 2024 — a recent stable toolchain; this workspace was
 built and tested against rustc 1.96).
- **Docker**, only if you want the durable backends (`RedisRegistry`,
 `PostgresStore`, `S3Store`) or want to reproduce their test suites — the
 default binaries run entirely in-memory with no external services.
- **Python 3** + a venv, only if you want to run the stub `ClientApp`
 (`python/conflux_client/stub_client.py`).

## Building and testing

```bash
cargo build --workspace
cargo test --workspace
cargo fmt --all -- --check
cargo clippy --workspace --all-targets
```

Run a single crate's tests, or a single test by name:

```bash
cargo test -p conflux-core
cargo test -p conflux-core test_name
```

Some tests require a real backing service (see
[Durable backends](#durable-backends-redis-postgres-s3) below) and will
fail with a connection error if that service isn't running — this is
expected, not a code problem; start the relevant container first.

Those tests find their services through three variables, whose defaults
match `docker-compose.yml`. A developer who runs `docker compose up -d`
needs none of them; CI sets all three, because its service containers
listen on the standard ports rather than the offset ones used here.

| Variable | Default |
|---|---|
| `CONFLUX_TEST_REDIS_URL` | `redis://127.0.0.1:16379` |
| `CONFLUX_TEST_POSTGRES_URL` | `postgres://postgres:conflux@127.0.0.1:15432/conflux` |
| `CONFLUX_TEST_S3_ENDPOINT` | `http://127.0.0.1:19000` |

## Quick start: running the full pipeline locally

This walks through the same three-process, cross-language setup verified
in a real `conflux-server`, a real `conflux-node`, and the real
Python stub client, all talking over actual gRPC.

```mermaid
sequenceDiagram
 participant You
 participant Server as conflux-server
 participant Node as conflux-node
 participant Py as stub_client.py

 You->>Server: cargo run -p conflux-server
 Note over Server: binds :50051 (gRPC) and :8080 (HTTP)<br/>starts the round loop immediately
 You->>Node: cargo run -p conflux-node
 Node->>Server: Register (network hop)
 Node->>Node: bind :47100 (local gRPC)
 You->>Py: python stub_client.py
 Py->>Node: Register (local hop)
 Py->>Node: FetchTask
 Node->>Server: FetchTask (forwarded)
 Server-->>Node: task (round, weights)
 Node-->>Py: task (forwarded)
 Py->>Py: "train" (+1.0 offset, no PyTorch)
 Py->>Node: SubmitDelta
 Node->>Server: SubmitDelta (forwarded)
 Server->>Server: aggregate, checkpoint, advance round
```

### 1. Start the server

```bash
cargo run -p conflux-server
```

By default this resolves config for topology `cross_device` / mode
`research` (see [Configuration](#configuration) to change that), logs
every resolved parameter (ADR 0007), and starts three things concurrently:
a gRPC server on `127.0.0.1:50051`, an HTTP admin server on
`127.0.0.1:8080`, and the round loop. With zero clients registered, the
round loop will print `no submissions yet; retrying shortly` every couple
of seconds — that's expected; it's waiting for a client.

Check it's up:

```bash
curl http://127.0.0.1:8080/health
curl http://127.0.0.1:8080/round/status
```

### 2. Start a node

In a second terminal:

```bash
cargo run -p conflux-node
```

This connects to the server, registers itself, and starts a local gRPC
server on `127.0.0.1:47100` for a Python `ClientApp` to connect to.

### 3. Run the stub Python client

```bash
cd python/conflux_client
python3 -m venv .venv
.venv/bin/pip install -r requirements.txt
./generate_proto.sh # regenerates fl_transport_pb2*.py — not committed
.venv/bin/python stub_client.py --address 127.0.0.1:47100
```

You should see the stub register, fetch the current round's task, "train"
it (a fixed `+1.0` offset — no PyTorch, this is a stand-in per ADR 0005),
and submit the result. Back in the server's terminal, you'll see the round
complete and advance. `curl http://127.0.0.1:8080/round/status` will now
report the next round number.

**Testing a robust aggregator against a real adversarial client**:
`stub_client.py --poison --poison-magnitude 1000.0` submits a
large-magnitude offset instead of honest training, standing in for a
Byzantine client — see `python/conflux_client/README.md` for
a quick worked example.

**Real end-to-end training, not just the pipeline**:
`python/conflux_client/examples/e2e_numpy_logreg/` and
`e2e_pytorch_mnist/` are complete, verified test harnesses — real
gradient descent (NumPy logistic regression, or a real PyTorch MLP on
real MNIST) across several simulated clients through the real Conflux
pipeline, each with its own `README.md` written for someone running this
framework for the first time. `./run_demo.sh krum 5 15 --poison
--no-reputation` shows a `robust` aggregator holding federated accuracy
within a couple points of a centralized baseline despite a persistent
attacker. See [`docs/E2E_TESTING.md`](E2E_TESTING.md) for the full
design rationale and two real findings this surfaced (a reputation/
aggregation pipeline-order gap, and a zero-init issue for ReLU models).

## Configuration

Neither binary has a config-file loader yet (spec §11 Open Item 2 is
still open) — both are controlled by environment variables.

### Axis selection (`conflux-server`) and node identity (`conflux-node`)

| Variable | Binary | Values | Default |
|---|---|---|---|
| `CONFLUX_TOPOLOGY` | `conflux-server` | `cross_silo`, `cross_device`, `crowdsource`, `edge` | `cross_device` |
| `CONFLUX_MODE` | both | `research`, `production` | `research` |
| `CONFLUX_SERVER_ADDR` | `conflux-node` | a `http://host:port` URL | `http://127.0.0.1:50051` |
| `CONFLUX_CLIENT_ID` | `conflux-node` | any string | `node-1` |
| `CONFLUX_LOCAL_ADDR` | `conflux-node` | a `host:port` | `127.0.0.1:47100` |
| `CONFLUX_ALLOW_STUB_CLIENT` | `conflux-node` | `true`, `false` | mode's own default (research `true`, production `false`) — see its phase brief |
| `CONFLUX_CLIENT_APP_KIND` | `conflux-node` | `stub`, `real` | `stub` (matches what's actually shipped, ADR 0005) |
| `CONFLUX_CONNECTION_MODE` | `conflux-node` | `push`, `pull` | `pull`. Picks which upstream transport the node uses — the server serves both RPCs either way, so this is the node's choice rather than something it has to match. What *does* have to match is the local hop: a Python `ClientApp` that calls `fetch_task` against a push-mode node (or `subscribe_tasks` against a pull-mode one) gets a typed error naming this variable, not a hang |
| `CONFLUX_CLIENT_SIDE_PRIVACY_TRANSFORM` | `conflux-node` | `true`, `false` | `false`. Applies spec §8's `clip + noise` on the node, *before* the update leaves the machine — the server-side transform still runs independently, so enabling both clips twice |
| `CONFLUX_SEED_VALUE` | `conflux-node` | an integer | (unset — nondeterministic). Seeds the client-side DP noise so a run reproduces. A malformed value panics rather than being ignored |
| `RUST_LOG` | both | standard `tracing`/`env_logger` filter syntax, e.g. `info`, `conflux_server=debug` | (unset — default level) |

`conflux-node` refuses to start with `CONFLUX_MODE=production` and the
default stub `CONFLUX_CLIENT_APP_KIND` unless `CONFLUX_ALLOW_STUB_CLIENT=true`
is set explicitly (a fail-fast guard).

### Durable backends and TLS (`conflux-server`/9a)

Every variable below is optional — omitted means the in-memory/plaintext
default, which `mode = production` refuses to start with unless every
one of registry/store/accounting-persistence/TLS is explicitly set (see
its phase brief).

| Variable | Purpose | Values |
|---|---|---|
| `CONFLUX_REGISTRY_BACKEND` | Client registry backend | `redis` (else in-memory) |
| `CONFLUX_REDIS_URL` | Redis connection (registry + node allow-list) | `redis://host:port` |
| `CONFLUX_STORE_BACKEND` | Checkpoint store backend | `postgres`, `s3` (else in-memory) |
| `CONFLUX_POSTGRES_URL` | Postgres connection (store and/or accounting persistence) | `postgres://user:pass@host:port/db` |
| `CONFLUX_S3_ENDPOINT`, `CONFLUX_S3_BUCKET`, `CONFLUX_S3_ACCESS_KEY`, `CONFLUX_S3_SECRET_KEY` | S3/MinIO checkpoint store | required together when `CONFLUX_STORE_BACKEND=s3` |
| `CONFLUX_ACCOUNTING_PERSISTENCE` | Persist the privacy accountant's round history | `true` (reuses `CONFLUX_POSTGRES_URL`) |
| `CONFLUX_TLS_CERT_PATH`, `CONFLUX_TLS_KEY_PATH`, `CONFLUX_TLS_CLIENT_CA_PATH` | mTLS material for the gRPC server, required in production when the resolved `auth` value is `mtls` (`cross_silo`'s topology default) | PEM file paths |

`require_node_auth` and the node allow-list are regular
`conflux-config` parameters, not separate env vars here — see the next
section.

### `conflux-config`'s resolved parameters

Everything resolves through `conflux-config`'s six-tier precedence chain
and is logged at startup (ADR 0007). `main.rs` reads a focused subset of
`Overrides` fields from their own env vars — enough to run
`docs/E2E_TESTING.md`'s harness without any code changes. Any field
without an env var below can now be set from an experiment config file
instead (see the next subsection); spec §11 Open Item 2's remaining
half is profile files, not experiment files.

| Variable | Overrides field |
|---|---|
| `CONFLUX_AGGREGATOR` | `aggregator`. Seventeen methods across five families: **averaging** — `fedavg`; **robust** — `krum`, `multi_krum`, `trimmed_mean`, `median`, `faba`, `bulyan`, `geometric_median`, `median_of_means`, `divide_and_conquer`; **temporal** — `foolsgold`, `centered_clipping`, `flanders`; **trusted** — `fltrust`; **optimization** — `fedavgm`, `fedadagrad`, `fedadam`, `fedyogi`, `qfedavg`. Two need more than a name: **`fltrust` requires a running sidecar** (see [Trusted-reference sidecar](#trusted-reference-sidecar-adr-0011); the server refuses to start without one), and `flanders` is a pre-aggregation *filter* paired with Krum per its own paper |
| `CONFLUX_SERVER_LEARNING_RATE` | `server_learning_rate` — the `optimization` family's `η`. Builtin `1.0`, and that is a **placeholder, not a recommendation**: Reddi et al. deliberately publish no universal value because it is the parameter their whole experimental section sweeps per task. Same posture as `clip_radius`. Ignored outside that family |
| `CONFLUX_SERVER_TAU` | `server_tau` — the `optimization` family's adaptivity floor `τ`. Builtin `1e-3`, which unlike `η` *is* the paper's own value, reported as working "almost as well as all other values" across their tasks. Smaller means more adaptive |
| `CONFLUX_FAIRNESS_Q` | `fairness_q` — q-FedAvg's fairness exponent. Builtin `0.0`, which **is exactly FedAvg**: selecting `qfedavg` without choosing a `q` should behave like the method it generalizes rather than silently applying a trade. Larger `q` weights high-loss clients up. Requires clients that report `local_loss`; without it the method falls back to FedAvg |
| `CONFLUX_SERVER_LIPSCHITZ` | `server_lipschitz` — q-FedAvg's `L`. A placeholder like `clip_radius`: the paper estimates it by grid search at `q = 0`, which the server cannot do because it never sees a loss surface |
| `CONFLUX_SERVER_MOMENTUM` | `server_momentum` — FedAvgM's `β`. Builtin `0.9`, a real default rather than a placeholder (it sits inside the paper's own `{0, 0.7, 0.9, 0.97, 0.99, 0.997}` sweep). Worth tuning on genuinely non-IID data, which is where the paper finds momentum matters most. `0.0` recovers plain FedAvg. Read only by `fedavgm` |
| `CONFLUX_SELECTOR` | `selector` |
| `CONFLUX_PRIVACY_MECHANISM` | `privacy_mechanism` |
| `CONFLUX_ROBUST_BYZANTINE_FRACTION` | `robust_byzantine_fraction` — only read by `robust`-family aggregators |
| `CONFLUX_CLIP_RADIUS` | `clip_radius` — Centered Clipping's `τ`. **Must be tuned to your model.** The builtin `1.0` is a placeholder: on a real 50,890-parameter MLP it scored *below undefended `fedavg`* (0.078 vs 0.163, measured). The server warns at startup if you select `centered_clipping` without setting this |
| `CONFLUX_REPUTATION_FILTER_ENABLED` | `reputation_filter_enabled` — the master switch for reputation gating, builtin `false` and owned by neither config axis. `CONFLUX_MIN_REPUTATION_SCORE` sets the threshold but does **not** turn the filter on |
| `CONFLUX_MIN_REPUTATION_SCORE` | `min_reputation_score` — see `docs/E2E_TESTING.md`'s "Real findings" for why you might want this low when testing a `robust` aggregator specifically |
| `CONFLUX_QUORUM` | `quorum` |
| `CONFLUX_MAX_UPDATE_BYTES` | `max_update_bytes` — the largest reassembled update the transport accepts from one client, in bytes (builtin 256 MiB). A trust-boundary bound rather than a tuning knob: gRPC's own limit is per *message*, so without this an unbounded chunk stream is an unbounded server allocation (Tier 5, H1). Over it, the client gets gRPC `resource_exhausted` and the server logs the client id and the limit |
| `CONFLUX_ROUND_TIMEOUT_SECS` | `round_timeout_secs` |
| `CONFLUX_CLIP_NORM` | `clip_norm` |
| `CONFLUX_NOISE_MULTIPLIER` | `noise_multiplier` |
| `CONFLUX_INITIAL_WEIGHTS_DIM` | not an `Overrides` field — the dimension of the server's placeholder initial checkpoint (`vec![0.0f32; N]`), which has to match whatever real model a deployment trains (default `4`) |
| `CONFLUX_ADMIN_TOKEN` | not an `Overrides` field — bearer token required on every HTTP admin route except `/health`. **The server refuses to start if `CONFLUX_HTTP_ADDR` binds beyond loopback without one**, since `/admin/allowlist` decides who may participate |
| `CONFLUX_JWT_PUBLIC_KEY_PATH` | not an `Overrides` field — a PEM public key (RSA → RS256, ECDSA → ES256) that `register()` verifies `auth_token` against when `auth = jwt`. Required in production for the three topologies that default to `jwt`; research warns and skips verification without it |
| `CONFLUX_EXPERIMENT_CONFIG_PATH` | not an `Overrides` field — a TOML file of experiment-level overrides (see below) |
| `CONFLUX_GRPC_ADDR` / `CONFLUX_HTTP_ADDR` | not `Overrides` fields — override the `FlTransport` gRPC and HTTP admin listen addresses (default `127.0.0.1:50051` / `127.0.0.1:8080`). Needed to reach the admin API from a different container — see `docs/WEB_APP_INTEGRATION.md`. Defaults stay loopback-only; binding wider is deliberate, and the server refuses to do it without `CONFLUX_ADMIN_TOKEN` (see the row above) |

Everything else (`require_node_auth`, `target_epsilon`, `seed_mode`,
...) still resolves correctly from topology/mode profiles and builtin
fallbacks, and can be set from an experiment config file.

### Experiment config file

Set `CONFLUX_EXPERIMENT_CONFIG_PATH` to a TOML file and every
`Overrides` field becomes settable, not just the subset with env vars:

```toml
# experiment.toml — flat keys named exactly like Overrides' fields.
# Any subset; anything absent falls through to the mode profile,
# topology profile, or builtin fallback exactly as before.
aggregator = "centered_clipping"
clip_radius = 4.0
round_timeout_secs = 120
auth = "mtls"
accounting_scope = "per_client"
```

```bash
CONFLUX_EXPERIMENT_CONFIG_PATH=./experiment.toml cargo run -p conflux-server
```

Startup then logs each of those values with the file's real path as its
source:

```
[config] aggregator = centered_clipping (source: experiment file "./experiment.toml")
```

Three things worth knowing:

- **Precedence is unchanged.** The file sits in the tier it always has:
 below env vars and CLI, above the mode and topology profiles. An env
 var still wins over the same key in the file.
- **Enum-valued keys use the same spelling the startup log prints** —
 `auth = "mtls"`, `connection_mode = "push"`, `accounting_scope =
 "per_client"`. There is no second vocabulary to learn, and a test
 enforces that there never will be.
- **A typo is an error, not a shrug.** An unrecognized key refuses the
 file and lists the valid ones, rather than being silently ignored and
 leaving you with a run that quietly used defaults. So is a missing
 file, or a value of the wrong type.

Profile files — topology/mode profiles themselves defined in TOML with
`inherits`-based extension (spec §4.1) — are deliberately **not** part
of this; they remain hardcoded Rust. See
its phase brief section.

Example: run the server as a `cross_silo` production deployment with
verbose logging:

```bash
CONFLUX_TOPOLOGY=cross_silo CONFLUX_MODE=production RUST_LOG=debug cargo run -p conflux-server
```

Every resolved parameter (topology defaults, mode defaults, built-in
fallbacks) is printed at startup before the server is "ready" — this is
mandatory, not optional verbosity (ADR 0007). In production mode this
prints as JSON lines; in research mode, human-readable text.

## Environment configuration

Every setting above is an environment variable, and there are enough of
them to be worth managing rather than remembering. `.env.example` is the
tracked template — every variable, grouped, with the non-obvious ones
annotated. Copy it and edit:

```bash
cp .env.example .env
```

`.env` is gitignored; `.env.example` is not, and must never hold a real
value.

Variables that are *commented out* in the template are optional, and
that distinction matters: leaving one unset lets `conflux-config`
resolve it through the topology/mode/builtin chain and log where the
value came from (ADR 0007). Setting it to an empty string is instead an
explicit override to "nothing", which is rarely what anyone wants.

This project uses [evnx](https://evnx.dev) to keep the two files honest:

```bash
evnx doctor # gitignore coverage, file permissions, .example sync
evnx diff # what one file has that the other doesn't
evnx sync # bring them back into line
evnx validate # placeholders, weak values, config mistakes
evnx scan .env # refuse to let a real secret reach a commit
```

`evnx validate` and `evnx scan` both run in CI. `validate` exits non-zero
on errors but not on warnings — the "uses localhost" warnings it reports
against the defaults are deliberate here, since the HTTP admin API's
whole safety model is that it binds to loopback unless you explicitly
give it a token.

Two things about `scan` worth knowing before trusting its output:

- **`evnx scan .env.example` scans nothing.** evnx excludes that filename
 by design, and `--exclude` cannot override it — so the command reports
 `Scanned 0 files` above a green `✓ No secrets detected`, which reads
 as a pass and is not one. Naming the template explicitly buys nothing;
 it is covered only as part of a directory scan.
- **Locally, scan the env files; in CI, scan the checkout.** `evnx scan
 .env` is the useful local command, because an unscoped scan here walks
 `python/conflux_client/.venv` and reports base64 font data inside PIL
 as an AWS key. CI runs `evnx scan .` instead and is right to: `.venv`
 is gitignored, so it does not exist in a fresh checkout, and scanning
 the root is what catches a credential pasted into a tracked file that
 is not an env file at all.

## Durable backends (Redis, Postgres, S3)

`conflux-registry::RedisRegistry`, `conflux-store::PostgresStore`, and
`conflux-store::S3Store` are real, tested, working implementations — but
**as of this writing, neither binary's `main.rs` selects them via an env
var yet**. Today
they're usable programmatically (construct one directly instead of
`InMemoryRegistry`/`InMemoryStore` if you're embedding `conflux-server`'s
crates in your own code) and are exercised by each crate's own test suite
against real local infrastructure.

To run those test suites, start the backing services first. The
repository ships a `docker-compose.yml` that brings up all three on the
ports the tests default to:

```bash
docker compose up -d # redis, postgres, minio
cargo test --workspace
docker compose down -v # stop and discard the data
```

The individual `docker run` commands below are equivalent, and remain
here because they document what compose is doing:

```bash
# Redis — conflux-registry's RedisRegistry tests
docker run -d --name conflux-dev-redis -p 16379:6379 redis:7-alpine

# Postgres — conflux-store's PostgresStore tests (checkpoints + privacy round log)
docker run -d --name conflux-dev-postgres \
 -e POSTGRES_PASSWORD=conflux -e POSTGRES_DB=conflux \
 -p 15432:5432 postgres:16-alpine

# MinIO (S3-compatible) — conflux-store's S3Store tests
docker run -d --name conflux-dev-minio -p 19000:9000 -p 19001:9001 \
 -e MINIO_ROOT_USER=confluxadmin -e MINIO_ROOT_PASSWORD=confluxsecret \
 minio/minio server /data --console-address ":9001"
```

Then:

```bash
cargo test -p conflux-registry # includes RedisRegistry's tests
cargo test -p conflux-store # includes PostgresStore's and S3Store's tests
```

Tear down when you're done:

```bash
docker rm -f conflux-dev-redis conflux-dev-postgres conflux-dev-minio
```

Each backend is namespaced per test (unique Redis key / Postgres table /
S3 prefix per test) so `cargo test`'s parallel execution doesn't race
against itself on one shared container.

## Trusted-reference sidecar (ADR 0011)

Only needed for a `trusted`-family aggregator — `fltrust` today. Every
other deployment can skip this section entirely: no sidecar runs, and no
connection is opened.

FLTrust scores clients against an update the **server** trains on its own
small trusted root dataset, rather than against anything derived from the
client batch. That is what makes it resist a colluding *majority*, which
no batch-derived method can — but it needs a training capability
`conflux-server` deliberately does not have (ADR 0004 keeps PyTorch
client-side and the server opaque to model architecture). ADR 0011's
resolution is a separate, optional process.

```bash
# 1. Start the sidecar with your trusted root dataset. One example per
# line, comma-separated, target last.
cat > root-dataset.csv <<'CSV'
1.0,0.0,2.0
0.0,1.0,3.0
1.0,1.0,5.0
CSV

CONFLUX_TRUSTED_DATASET_PATH=root-dataset.csv CONFLUX_SIDECAR_ADDR=127.0.0.1:50100 cargo run -p conflux-trusted-reference
```

```bash
# 2. Point the server at it.
CONFLUX_AGGREGATOR=fltrust CONFLUX_TRUSTED_REFERENCE_ADDR=http://127.0.0.1:50100 cargo run -p conflux-server
```

The server performs a `Describe` handshake at startup and **refuses to
start** if the sidecar is missing, unreachable, or cannot serve the
configured method. That is deliberate: a server that came up without the
signal its aggregator is defined in terms of would run rounds and write
checkpoints that look healthy while the defense was simply absent.
Conversely, setting `CONFLUX_TRUSTED_REFERENCE_ADDR` with a non-trusted
aggregator logs a warning and opens no connection.

The sidecar's own training effort is tunable, and worth setting
deliberately: FLTrust assumes the server trains comparably to its
clients and never checks it. A reference trained far less than the
clients were is still a valid vector — it just points less far in the
right direction, which quietly weakens every trust score computed
against it.

| Variable | Default | What it does |
|---|---|---|
| `CONFLUX_TRUSTED_LEARNING_RATE` | `0.05` | Gradient-descent step size |
| `CONFLUX_TRUSTED_STEPS` | `200` | Full-batch steps per reference |

**What ships, and its limit.** The sidecar's bundled model is
`LinearLeastSquares` — real gradient descent on your dataset, not a stub,
and honestly a *linear* model. A deployment training anything else
implements the `TrustedModel` trait against a runtime that can run its
architecture (ONNX, libtorch, or a Python process speaking the same gRPC
service) and serves that instead. That extension point is the whole
reason the capability lives out here rather than in the server.

**The root dataset is the trust anchor.** FLTrust is exactly as good as
that file: a reference trained on unrepresentative data points somewhere
honest clients do not, and the method's `ReLU` then zeroes *them*. If the
sidecar is not colocated with the server, put the hop behind TLS —
`TrustedReferenceTransport::connect_with_tls` is the other half.

## mTLS

`conflux-net::tls` provides real mutual-TLS config builders
(`server_tls_config`/`client_tls_config`) and
`PullTransport`/`PushTransport::connect_with_tls`. Like the durable
backends above, this is tested (see `crates/conflux-net/tests/mtls.rs` —
real certificate generation via `rcgen`, a real handshake, and a real
rejection of both an untrusted-CA client and a plaintext client) but not
yet wired into either binary's startup. To use it today, call
`connect_with_tls`/`server_tls_config` directly from your own integration
code, using real certificates for your deployment (the test suite
generates disposable self-signed ones purely for testing).

## Load / concurrency testing

`crates/conflux-server/tests/load.rs` spins up 30 concurrent simulated
clients across 3 rounds against a real running server and reports timing:

```bash
cargo test -p conflux-server --test load -- --nocapture
```

## Development workflow

```bash
cargo fmt --all # apply formatting
cargo clippy --workspace --all-targets # lint everything, including tests
```

Before starting any new work, read [CONTRIBUTING.md](../CONTRIBUTING.md)
— it covers what CI enforces and the four things this project is
opinionated about. See
[ARCHITECTURE.md](ARCHITECTURE.md#how-the-project-was-built) for why that
matters here specifically.
