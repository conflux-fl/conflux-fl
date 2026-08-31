# `cflux` — a CLI for Conflux FL

**Status: proposed, and deliberately sequenced last.** Nothing built
yet. This is a scoping document in the same spirit as
`docs/AGGREGATION_LANDSCAPE.md`: analysis and a proposed shape, written
to precede a numbered phase brief once the scope below is confirmed,
not a decision already made.

**Sequencing decision (2026-08-31, project owner):** the CLI comes
*after* `conflux-fl` is stable, not before. The reasoning is that a CLI
is a surface over functionality that already exists — building one over
an unstable surface means rewriting it when the surface moves. Crate
publication is likewise gated on stability, which matters here because
`cargo install cflux` is one of the two distribution paths below.

**Status update (2026-08-31):** stabilization Tiers 1–6 are complete,
so the condition this sequencing waited on has been met. The CLI is
still last in the order, now behind whichever of the deferred feature
gaps (ADR 0012's proto extension in particular) land first — a CLI written against a surface that is about
to gain stateful-aggregator plumbing would be rewritten for the same
reason this paragraph was written to avoid.

**Documentation model (2026-08-31):** `cflux` follows the
[`evnx`](https://evnx.dev) pattern — same project owner, and a proven
shape. See "Documentation as part of the tool" below; it is a design
constraint on the CLI, not a docs-site task to do afterwards.

## Why this exists

Every operation on Conflux FL today is `cargo run -p conflux-server`
with a wall of `CONFLUX_*` env vars, or a raw
`cargo run --release --example run_experiment -p conflux-attacks --
--aggregator ... --attack ...` for research work. That's a fine
surface for someone already deep in the codebase; it's a rough one for
anyone else — installing the framework, running it for the first
time, setting up a research sweep, or taking it to a live deployment
all currently require reading source or long docs pages to assemble
the right command. A CLI's job is a nicer, discoverable surface on
functionality that already exists — not new behavior of its own.

## The one design decision everything else follows from

**Two binaries, not one**: `cflux` (production) and `cflux-dev`
(research/testing). This mirrors a boundary the codebase already
enforces at the dependency-graph level: ADR 0010 keeps
`conflux-attacks` a dev-dependency of nothing `conflux-server` ships,
specifically so attack-simulation code is *structurally* incapable of
reaching a production binary. A single `cflux` binary with both
`server start` and `experiment run` subcommands would quietly undo
that guarantee — the compiled artifact operators run in production
would link attack code, even if no one ever typed the subcommand that
uses it. Splitting the binary is what keeps the CLI's own shape
consistent with a guarantee the rest of the project already treats as
load-bearing.

| | `cflux` | `cflux-dev` |
|---|---|---|
| Depends on | `conflux-server`, `conflux-node`, `conflux-config`, `conflux-registry`, `conflux-store` | all of the above, plus `conflux-attacks` |
| Ships in a production deployment | Yes | Never |
| Who uses it | Operators, deployers | Researchers, contributors |

## Command tree

### `cflux` — production surface

```
cflux init [--topology <t>] [--mode <m>] [--docker]
    Scaffold a new deployment directory: a starter .env (or
    experiment.toml once config-file parsing, phase-20, ships), and
    optionally a docker-compose.yml wiring up Redis/Postgres/MinIO for
    the chosen topology's real backends.

cflux doctor [--topology <t>] [--mode <m>]
    Check everything the server would otherwise fail-fast on, one at a
    time, up front instead of one attempt at a time: durable backend
    reachability (Redis/Postgres/S3), TLS material presence and
    validity, node-auth configuration consistency. Exit code reflects
    pass/fail for scripting (`cflux doctor --json` in CI).

cflux server start [--topology <t>] [--mode <m>] [--config <file>]
    Resolve config and run the round pipeline — the CLI-native
    replacement for `CONFLUX_TOPOLOGY=x CONFLUX_MODE=y cargo run -p
    conflux-server`.

cflux server resolve-config [--topology <t>] [--mode <m>] [--json]
    Dry run: print the fully resolved, source-annotated configuration
    (ADR 0007) without starting anything. The single most useful
    command for validating a deployment before it's live.

cflux server status [--addr <http-addr>]
    Pretty-printed /health + /round/status.

cflux node start [--server <addr>] [--client-id <id>] [--local-addr <addr>]
    Runs the client-side bridge — the CLI-native replacement for
    `cargo run -p conflux-node`.

cflux allowlist add <client-id> (--cert-fingerprint <fp> | --token <t>)
cflux allowlist list [--json]
cflux allowlist remove <client-id>
    Wraps the HTTP admin allow-list endpoints (Phase 8c) instead of
    hand-written curl calls.

cflux checkpoint list [--json]
cflux checkpoint show <round>
    Inspect what's actually stored — dimension, checksum, timestamp —
    without writing a throwaway script against the Store trait.

cflux version
```

### `cflux-dev` — research/testing surface

```
cflux-dev experiment run --aggregator <name> --attack <name>
    [--rounds N] [--seed N] [--num-honest N] [--num-attackers N]
    A friendlier front end for `run_experiment` — same underlying
    binary, without the `cargo run --release --example ... -p
    conflux-attacks --` ceremony.

cflux-dev experiment sweep --config <sweep.toml>
    A single declarative sweep definition replacing a hand-written
    shell script — the same grid `docs/research/scripts/*.sh` already
    sweep, expressed as data instead of bash loops.

cflux-dev aggregator list [--json]
cflux-dev aggregator describe <name>
    Citation, family shape, tunable parameters — reads directly from
    conflux-config's own strategy registry, so this can never drift
    from what's actually shipped the way a hand-maintained doc page
    could.

cflux-dev selector list
cflux-dev privacy list
cflux-dev attack list
    Same idea, for the other three registry-backed families.

cflux-dev version
```

## What `cflux init` and `cflux doctor` actually save

Today, standing up a `cross_silo` production deployment means reading
the config guide, hand-assembling ten-plus env vars, starting the
binary, and finding out about a missing piece of TLS material one
error at a time. `cflux init --topology cross_silo --mode production
--docker` would generate a starter `.env` with every required variable
named (even if empty, so nothing is silently missing) and a
`docker-compose.yml` wiring up Redis + Postgres for that topology's
real backends. `cflux doctor` then checks all of it — reachability,
material presence, auth consistency — and reports a complete list
before the first real start attempt, instead of the current
one-error-at-a-time experience.

## Full comparison with `flwr` (Flower's CLI)

Flower's CLI is a considerably larger surface than what's proposed
above — worth understanding *why*, not just *that*, before treating
the size difference as a gap to close.

| `flwr` command | What it does | `cflux` equivalent | Why the same, or why not |
|---|---|---|---|
| `flwr new` | Scaffold a new Flower App project | `cflux init` | Same idea — go from nothing to a runnable starting point |
| `flwr run` | Submit an app run to a federation | `cflux server start` | Conflux FL has no separate "submit a run to a remote federation" step — a deployment *is* the running server, not a job submitted to one |
| `flwr stop` / `flwr list` / `flwr log` / `flwr pull` | Manage the lifecycle of a submitted run (stop it, list runs, stream logs, download artifacts) | `cflux server status`, `cflux checkpoint list/show` | Partial overlap — Conflux FL doesn't have Flower's multi-run-per-deployment model (ADR 0003: one server process = one experiment), so there's no "list of runs" to manage, only one long-lived process's own state |
| `flwr build` / `flwr install` | Package an app into a FAB (Flower App Bundle) and install one | *(none planned)* | Flower Apps are distributable, versioned artifacts because the same SuperLink can run different apps over time. Conflux FL's one-process-one-experiment model (ADR 0003) has no equivalent unit to package |
| `flwr app publish` / `flwr app review` | Upload an app to Flower Hub, or review one someone else published | *(none planned)* | Flower Hub is Flower Labs' cloud registry/marketplace — a commercial SaaS surface, not something Conflux FL has or needs an equivalent of |
| `flwr login` | Authenticate to a SuperLink / Flower account | *(none planned — see node auth below)* | Conflux FL's node identity model (Phase 8b/8c: cert fingerprint or shared token, soon JWT — Phase 16) is deployment-level, not a personal-account login system |
| `flwr federation create/list/archive`, `flwr federation invite ...` | Manage named remote deployment targets and multi-account access to them | *(none planned)* | This entire subtree exists because Flower Hub hosts federations as a multi-tenant cloud service with per-account invitations. Conflux FL is self-hosted and explicitly not multi-tenant (ADR 0003) — there's no "invite another account to your federation" concept to manage |
| `flwr supernode register/unregister/list` | Manage client identities on a federation | `cflux allowlist add/remove/list` | Same concept, different auth primitive — Flower uses P-384 EC keypairs; Conflux FL uses cert fingerprint / shared token / (soon) JWT |
| `flwr federation simulation-config` | Configure a Ray-backed local simulation of many virtual clients | *(none planned, see below)* | A real gap worth a real answer — see "What Flower has that's worth learning from," below |
| `flwr chat` | Talk to Flower's hosted AI agent | *(none planned)* | A Flower-Labs product feature (their "Flower Agent" positioning), unrelated to running federated learning experiments |
| `flower-superlink` / `flower-supernode` (separate binaries, extensive flags: `--fleet-api-address`, `--isolation`, `--ssl-*`, ...) | Start the actual server/client processes, with production-grade TLS/isolation/auth flags | `cflux server start` / `cflux node start` | Same role. Conflux FL's flag surface is smaller today because fewer of these knobs exist yet (JWT — phase-16, config-file parsing — phase-20) |

### What Flower has that's worth learning from

Being thorough here rather than defensive: two things in Flower's CLI
are genuinely good ideas independent of Flower's cloud/SaaS framing,
worth a real yes/no rather than dismissing the whole comparison as
"they're a bigger company with a hosted product":

1. **`--format json` on every single command.** Every `flwr` command
   listed above accepts it. `cflux`'s design above only puts `--json`
   on a few commands — should be **every** command that prints
   anything, for the same reason Flower has it everywhere: scriptable
   output is what makes a CLI usable from CI or another tool, not just
   a human's terminal.
2. **`flwr federation simulation-config`** — a Ray-backed local
   simulation of many virtual clients from one process, for fast
   iteration without standing up real infrastructure. Conflux FL's
   closest existing thing is the e2e demo scripts
   (`run_demo.sh N_CLIENTS ROUNDS`), which spawn real OS processes per
   client — heavier, but also more faithful (real gRPC, real process
   boundaries, not simulated ones). Worth a real question for later,
   not answered here: is a lighter-weight, single-process,
   many-simulated-clients mode (`cflux-dev experiment simulate
   --num-clients 50`) worth building for fast local iteration, trading
   some of that realism for speed? Flagging as an open question, not
   proposing an answer.

### What this comparison confirms, not just contrasts

The size difference isn't Conflux FL being behind — most of Flower's
CLI surface exists to support things Conflux FL deliberately doesn't
do: a hosted multi-tenant registry (Flower Hub), a personal-account
login system, a multi-run-per-server job-submission model. Every one
of those traces back to a real, already-documented Conflux FL design
choice (ADR 0003's no-multi-tenancy, the self-hosted/no-cloud-service
posture implicit in everything else in this repo) — the comparison
mostly *confirms* those choices were coherent, rather than surfacing
gaps `cflux` should close by imitation.

## Documentation as part of the tool (the `evnx` model)

`evnx` (same project owner, v0.3.8 at time of writing) already solves
the problem `cflux` will have, and the shape is worth copying rather
than re-deriving. Three separable ideas:

### 1. Every command's `--help` ends with a deep link to its own guide

```
$ evnx validate --help
Check .env against .env.example, find issues
...
  -h, --help               Print help

📖  Full guide: https://www.evnx.dev/guides/commands/validate
```

The URL is per-command and predictable: `/guides/commands/<command>`.
The terminal carries the *signature* — flags, types, defaults — and
hands off to the web for the *explanation*. Neither duplicates the
other, which is why neither drifts.

For `cflux` this means `https://<docs-host>/guides/commands/<command>`,
emitted from a single helper so no subcommand can be added without one.
The docs host is a `conflux-web` concern; the *link* is a `cflux`
concern, and it is easier to build in from the first command than to
retrofit onto twenty.

### 2. Each command is a self-contained module

`evnx` has twelve top-level commands, each with its own flags, its own
output shape, and its own guide page. There is no shared "god struct"
of options threaded through everything — only three genuinely global
flags (`-v`, `-q`, `--no-color`) plus `--help`/`--version`.

The Rust shape this implies, and the one `cflux` should adopt:
`src/commands/<name>.rs` per command, each exporting its own `clap`
`Args` struct and a `run(args, ctx) -> Result<Output, CliError>`. The
top-level dispatcher only routes. Two consequences worth naming: a new
command is a new file plus one match arm (the same additive property
ADR 0002's registry gives aggregators), and a command's guide page,
its `Args`, and its tests sit close enough together that updating one
without the others is visibly incomplete.

### 3. Machine-readable output is a first-class mode, not an afterthought

`evnx validate` takes `--format pretty|json|github-actions`. Not a
`--json` bolt-on: three named renderers over one structured result,
and the human-readable one is just the default renderer.

Everything downstream follows from that separation:

| `evnx` feature | Why it exists | `cflux` equivalent |
|---|---|---|
| `--format json` | Parseable by another tool | Every command that prints anything |
| `--format github-actions` | `::error file=...::` inline PR annotations | `cflux doctor` in CI |
| Documented **exit codes** (`0` clean, `1` errors found) | Scripts branch on them | `cflux doctor`, `cflux server resolve-config` |
| `--exit-zero` | Report findings without failing the pipeline | Same, same commands |
| Named issue **types** (`missing_variable`, `weak_secret`) | Stable identifiers to `--ignore`, not prose to grep | `cflux doctor` findings |
| **Severity** (error / warning / style) | Not everything found is fatal | Same |
| `--fix`, with an explicit *auto-fixable* column | Only some findings are safely repairable, and the docs say which | `cflux doctor --fix` — see caveat below |
| `completions` subcommand | Shell completion for bash/zsh/fish | Same, free via `clap_complete` |
| `.evnx.toml` | Per-project defaults so flags aren't retyped | `.cflux.toml`, once phase-21 profile files land |

This table is the actionable part of the comparison. The
`--json`-everywhere point was already raised in the Flower comparison
below as a lesson worth taking; `evnx` shows the same conclusion
reached independently, plus the four things that only become possible
*after* output is structured (stable issue types, severities,
`--exit-zero`, CI-native formats).

**One caveat, specific to `cflux`.** `evnx validate --fix` can
regenerate a weak secret because a `.env` file is a local artifact and
the blast radius is one file `git diff` will show. `cflux doctor --fix`
would be operating on a *deployment* — an unreachable Redis, missing TLS
material, an inconsistent auth configuration. Most of those have no safe
automatic repair, and the ones that look like they do (generating a
missing key, say) are exactly the ones where doing it silently is worst.
The `--fix` row above should ship as **suggestions printed, not applied**
unless a specific finding is argued into auto-fixability one at a time.

### What this changes about the MVP

The MVP below stays the same five commands, but two properties are now
non-negotiable from the first commit rather than retrofitted:
structured output with a `--format` renderer split, and a
`📖 Full guide:` link on every subcommand. Both are cheap at five
commands and expensive at twenty.

## Installation

Two realistic distribution paths, not mutually exclusive:

- **`cargo install cflux`** (once published) — the natural path for
  anyone who already has a Rust toolchain, which is most of this
  framework's current audience.
- **Prebuilt release binaries** (GitHub Releases, one per OS/arch) —
  for operators who want to deploy `cflux`/`cflux server` without
  installing a Rust toolchain on a production host at all.

Neither needs Docker — Docker is only relevant to what `cflux init
--docker` *generates* (a compose file for the durable backends), not
to running `cflux` itself.

## Non-goals

- A hosted registry/marketplace for sharing configs or experiment
  definitions (Flower Hub's role) — no such service exists for
  Conflux FL, and building one is a product decision far outside this
  document's scope.
- A personal-account login/auth system — node identity is a deployment
  concern (allow-list entries), not a user-account concern.
- Managing multiple experiments from one CLI session/server — ADR
  0003 already settled this: one process, one experiment; running two
  means running two processes, each with its own `cflux server start`.
- Replacing `cargo test`/`cargo build` for development — `cflux`/
  `cflux-dev` are for *running* the framework and its experiments, not
  a development-workflow tool.

## Suggested MVP, if this gets prioritized

Not every command above needs to ship at once. Smallest useful first
cut: `cflux server start`, `cflux server resolve-config`, `cflux
doctor`, `cflux-dev experiment run`, `cflux-dev aggregator list`. That
covers "run the framework," "debug a misconfiguration before it bites
you," and "run a research comparison without the `cargo run --example`
ceremony" — the three most common actual needs today — with
`init`/`allowlist`/`checkpoint`/`sweep` as natural fast-follows.
