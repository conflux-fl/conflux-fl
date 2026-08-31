# Using Conflux

A practical guide to building, running, and testing Conflux. For *why*
things are built the way they are, see [ARCHITECTURE.md](ARCHITECTURE.md).
For the authoritative design, see
[`spec/conflux-spec-v1.md`](spec/conflux-spec-v1.md).

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

## Quick start: running the full pipeline locally

This walks through the same three-process, cross-language setup verified
in Phase 6 — a real `conflux-server`, a real `conflux-node`, and the real
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
./generate_proto.sh          # regenerates fl_transport_pb2*.py — not committed
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
Byzantine client (Phase 11c) — see `python/conflux_client/README.md` for
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
| `CONFLUX_ALLOW_STUB_CLIENT` | `conflux-node` | `true`, `false` | mode's own default (research `true`, production `false`) — see `docs/phases/phase-9b-stub-client-guard.md` |
| `CONFLUX_CLIENT_APP_KIND` | `conflux-node` | `stub`, `real` | `stub` (matches what's actually shipped, ADR 0005) |
| `RUST_LOG` | both | standard `tracing`/`env_logger` filter syntax, e.g. `info`, `conflux_server=debug` | (unset — default level) |

`conflux-node` refuses to start with `CONFLUX_MODE=production` and the
default stub `CONFLUX_CLIENT_APP_KIND` unless `CONFLUX_ALLOW_STUB_CLIENT=true`
is set explicitly (phase-9b's fail-fast guard).

### Durable backends and TLS (`conflux-server`, Phase 8a/9a)

Every variable below is optional — omitted means the in-memory/plaintext
default, which `mode = production` refuses to start with unless every
one of registry/store/accounting-persistence/TLS is explicitly set (see
`docs/phases/phase-8a-backend-selection.md` and `phase-9a-auth-enforcement.md`).

| Variable | Purpose | Values |
|---|---|---|
| `CONFLUX_REGISTRY_BACKEND` | Client registry backend | `redis` (else in-memory) |
| `CONFLUX_REDIS_URL` | Redis connection (registry + node allow-list, Phase 8c) | `redis://host:port` |
| `CONFLUX_STORE_BACKEND` | Checkpoint store backend | `postgres`, `s3` (else in-memory) |
| `CONFLUX_POSTGRES_URL` | Postgres connection (store and/or accounting persistence) | `postgres://user:pass@host:port/db` |
| `CONFLUX_S3_ENDPOINT`, `CONFLUX_S3_BUCKET`, `CONFLUX_S3_ACCESS_KEY`, `CONFLUX_S3_SECRET_KEY` | S3/MinIO checkpoint store | required together when `CONFLUX_STORE_BACKEND=s3` |
| `CONFLUX_ACCOUNTING_PERSISTENCE` | Persist the privacy accountant's round history | `true` (reuses `CONFLUX_POSTGRES_URL`) |
| `CONFLUX_TLS_CERT_PATH`, `CONFLUX_TLS_KEY_PATH`, `CONFLUX_TLS_CLIENT_CA_PATH` | mTLS material for the gRPC server, required in production when the resolved `auth` value is `mtls` (`cross_silo`'s topology default) | PEM file paths |

`require_node_auth` and the node allow-list (Phase 8b/8c) are regular
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
| `CONFLUX_AGGREGATOR` | `aggregator` — `fedavg`, `krum`, `multi_krum`, `trimmed_mean`, `median`, `faba`, `bulyan`, `geometric_median`, `median_of_means`, `divide_and_conquer`, `foolsgold`, `centered_clipping` |
| `CONFLUX_SELECTOR` | `selector` |
| `CONFLUX_PRIVACY_MECHANISM` | `privacy_mechanism` (Phase 11b) |
| `CONFLUX_ROBUST_BYZANTINE_FRACTION` | `robust_byzantine_fraction` — only read by `robust`-family aggregators |
| `CONFLUX_CLIP_RADIUS` | `clip_radius` — Centered Clipping's `τ`. **Must be tuned to your model.** The builtin `1.0` is a placeholder: on a real 50,890-parameter MLP it scored *below undefended `fedavg`* (0.078 vs 0.163, [§5.13](research/temporal-consistency-aggregation.md)). The server warns at startup if you select `centered_clipping` without setting this |
| `CONFLUX_MIN_REPUTATION_SCORE` | `min_reputation_score` — see `docs/E2E_TESTING.md`'s "Real findings" for why you might want this low when testing a `robust` aggregator specifically |
| `CONFLUX_QUORUM` | `quorum` |
| `CONFLUX_MAX_UPDATE_BYTES` | `max_update_bytes` — the largest reassembled update the transport accepts from one client, in bytes (builtin 256 MiB). A trust-boundary bound rather than a tuning knob: gRPC's own limit is per *message*, so without this an unbounded chunk stream is an unbounded server allocation (Tier 5, H1). Over it, the client gets gRPC `resource_exhausted` and the server logs the client id and the limit |
| `CONFLUX_ROUND_TIMEOUT_SECS` | `round_timeout_secs` |
| `CONFLUX_CLIP_NORM` | `clip_norm` |
| `CONFLUX_NOISE_MULTIPLIER` | `noise_multiplier` |
| `CONFLUX_INITIAL_WEIGHTS_DIM` | not an `Overrides` field — the dimension of the server's placeholder initial checkpoint (`vec![0.0f32; N]`), which has to match whatever real model a deployment trains (default `4`) |
| `CONFLUX_ADMIN_TOKEN` | not an `Overrides` field — bearer token required on every HTTP admin route except `/health`. **The server refuses to start if `CONFLUX_HTTP_ADDR` binds beyond loopback without one**, since `/admin/allowlist` decides who may participate |
| `CONFLUX_JWT_PUBLIC_KEY_PATH` | not an `Overrides` field — a PEM public key (RSA → RS256, ECDSA → ES256) that `register()` verifies `auth_token` against when `auth = jwt` (Phase 16). Required in production for the three topologies that default to `jwt`; research warns and skips verification without it |
| `CONFLUX_EXPERIMENT_CONFIG_PATH` | not an `Overrides` field — a TOML file of experiment-level overrides (see below) |
| `CONFLUX_GRPC_ADDR` / `CONFLUX_HTTP_ADDR` | not `Overrides` fields — override the `FlTransport` gRPC and HTTP admin listen addresses (default `127.0.0.1:50051` / `127.0.0.1:8080`). Needed to reach the admin API from a different container — see `docs/WEB_APP_INTEGRATION.md`. Defaults stay loopback-only since the admin API has no auth of its own; bind wider deliberately, not by habit |

Everything else (`require_node_auth`, `target_epsilon`, `seed_mode`,
...) still resolves correctly from topology/mode profiles and builtin
fallbacks, and can be set from an experiment config file.

### Experiment config file (Phase 20)

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
[config] aggregator = centered_clipping  (source: experiment file "./experiment.toml")
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
`docs/phases/phase-20-config-file-parsing.md`'s "out of scope" section.

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
evnx doctor              # gitignore coverage, file permissions, .example sync
evnx diff                # what one file has that the other doesn't
evnx sync                # bring them back into line
evnx validate            # placeholders, weak values, config mistakes
evnx scan .env           # refuse to let a real secret reach a commit
```

`evnx validate` and `evnx scan` both run in CI. `validate` exits non-zero
on errors but not on warnings — the "uses localhost" warnings it reports
against the defaults are deliberate here, since the HTTP admin API's
whole safety model is that it binds to loopback unless you explicitly
give it a token.

Scope `scan` to the env files (`evnx scan .env .env.example`) rather than
the repository root: an unscoped scan walks `python/conflux_client/.venv`
and reports base64 font data inside PIL as an AWS key.

## Durable backends (Redis, Postgres, S3)

`conflux-registry::RedisRegistry`, `conflux-store::PostgresStore`, and
`conflux-store::S3Store` are real, tested, working implementations — but
**as of this writing, neither binary's `main.rs` selects them via an env
var yet** (tracked in [`STATUS.md`](STATUS.md)'s "Next" section). Today
they're usable programmatically (construct one directly instead of
`InMemoryRegistry`/`InMemoryStore` if you're embedding `conflux-server`'s
crates in your own code) and are exercised by each crate's own test suite
against real local infrastructure.

To run those test suites, start the backing services first. The
repository ships a `docker-compose.yml` that brings up all three on the
ports the tests default to:

```bash
docker compose up -d      # redis, postgres, minio
cargo test --workspace
docker compose down -v    # stop and discard the data
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
cargo test -p conflux-registry   # includes RedisRegistry's tests
cargo test -p conflux-store      # includes PostgresStore's and S3Store's tests
```

Tear down when you're done:

```bash
docker rm -f conflux-dev-redis conflux-dev-postgres conflux-dev-minio
```

Each backend is namespaced per test (unique Redis key / Postgres table /
S3 prefix per test) so `cargo test`'s parallel execution doesn't race
against itself on one shared container.

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
cargo fmt --all              # apply formatting
cargo clippy --workspace --all-targets   # lint everything, including tests
```

Before starting any new work, read [`STATUS.md`](STATUS.md) for what's
done and what's next, then the relevant brief under
[`phases/`](phases/) — each phase brief states its scope, what it
deliberately doesn't cover, and its test plan. After finishing work,
update `STATUS.md` (what shipped, what's next, any deviation from spec
and why) — this is how the project stays legible across sessions; see
[ARCHITECTURE.md](ARCHITECTURE.md#how-the-project-was-built) for why that
matters here specifically.
