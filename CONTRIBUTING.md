# Contributing to Conflux FL

Thanks for looking. This document is short on ceremony and specific
about the few things this codebase genuinely cares about.

## Before you start

```bash
cargo build --workspace
cargo test --workspace
```

That is the whole setup. The durable-backend tests (Redis, Postgres,
S3) skip themselves when no service is reachable, so a plain checkout
runs green.

## What gets merged

Every change has to pass what CI enforces:

```bash
cargo fmt --all
cargo clippy --workspace --all-targets   # CI denies warnings
cargo test --workspace
```

Plus, for anything touching the Python client:

```bash
cd python/conflux_client && ./ci_smoke.sh 2
```

## The four things this project is opinionated about

**1. Every fallible public function returns a `thiserror`-derived enum,
never a `String`.** An error a caller cannot match on is a log line
wearing a return type.

**2. Comments say *why*, not *what*.** The code already says what it
does. What is expensive to reconstruct six months later is why the
obvious alternative was rejected. A comment that names the rejected
alternative and the reason is worth more than one that narrates the
code.

**3. A new aggregation method must be a literal implementation of a
published paper**, cited in its doc comment (the framework ships literal
implementations, not variants). This is a catalog
researchers compare against, so "our improved variant of Krum" is a
different project — build it against the public API from your own
repository, which
[https://confluxfl.dev/guides/extending/](https://confluxfl.dev/guides/extending/) explains how to do. It joins the
catalog when it is published.

**4. A demonstration that cannot fail is not evidence.** Before
believing a benchmark or a demo, check what result would have falsified
it. This project shipped a client demo whose data was split so evenly
that every client scored 1.000 *before* federating — the federated
number looked excellent and meant nothing.

## Adding an aggregation method

[https://confluxfl.dev/guides/extending/](https://confluxfl.dev/guides/extending/) has the steps. The short version:
implement the small trait your method's family varies (not the whole
`Aggregator`), register it with `inventory::submit!`, and add it to
`build_aggregator`. A new averaging variant is usually a ten-line trait
impl.

Two obligations that are easy to miss:

- **Decode through `decode_and_validate`.** It is the single chokepoint
  that rejects non-finite weights. Skipping it is how a method acquires
  the `NaN` defects this catalog has already fixed — there have been
  eight.
- **If your method keeps state across rounds**, it needs cross-round
  tests. `tests/stateful_adversarial_input.rs` is the pattern. Four
  defects were found by writing it, and none was visible to a
  single-round test.

- **If your method's paper states a numeric result**, it should come
  with a reproduction under `baselines/<method>-<author>-<year>/`. Four
  exist to copy from. What makes one count:
  - **The expected value is measured here, not copied from the paper.**
    `krum-blanchard-2017` records `expected = 0.88` with the measurement
    beside it — *"0.890 held-out @ round 15 — unmoved by independent
    client seeding, since krum selects one update per round"*. A number
    lifted from a table proves the table was read, not that this
    implementation reproduces it.
  - **The scenario isolates your method's own claim.** Krum's manifest
    sets `no_reputation = true` so the reproduction tests Krum's defense
    rather than Conflux's separate reputation filter. If both run, a pass
    says nothing about either.
  - **The tolerance is justified**, not padded until it passes.

  If a reproduction is genuinely impractical — the paper reports no
  number, or it needs a dataset we cannot ship — say so in the pull
  request. That is a reasonable answer; silence is not.

## Working on `conflux-attacks`

The attack crate is **not published to crates.io**, deliberately rather
than by omission. Attack code that could run against a production
aggregator has to be *structurally* incapable of shipping in the
production binary: CI asserts `conflux-server`'s dependency tree never
contains it at any depth, and `publish = false` closes the other route. A
crates.io release would be an installable copy of the exact thing that
edge exists to prevent.

The consequence is worth stating plainly, because it lands on the people
most likely to want the crate: security researchers are the one audience
who must clone rather than `cargo add`.

```bash
git clone https://github.com/conflux-fl/conflux-fl
cargo test -p conflux-attacks --test attack_vs_defense
```

That runs every shipped attack against every shipped aggregator. New
attacks carry the same citation discipline as the defenses — a published
attack, implemented against its paper — and join that matrix.

## Numeric code

Two rules, each learned from a real defect:

- **Accumulate in `f64`, narrow at the end.** Two finite `f32`s can be
  `2·f32::MAX` apart, which overflows to infinity, and `inf * 0.0` is
  `NaN`. A correct clipping step once corrupted a server permanently
  this way, from a single update that passed every validation check.
- **Normalize before accumulating, not after.** `f32::MAX * 10` is
  already infinity by the time you divide.

## Commits and pull requests

Explain *why* in the message, not just what changed — the same standard
as comments. If the change fixes a defect, say how it was found; "found
by running it end to end" is more useful to the next person than the
diff.

Small, focused pull requests get reviewed faster. If a change touches
`conflux-proto`, say so prominently: it is the most stable layer, and a
schema change reaches every deployed client and both client SDKs at
once.

## Reporting bugs and vulnerabilities

Ordinary bugs: open an issue. Security vulnerabilities: **do not** open
a public issue — see [SECURITY.md](SECURITY.md).

## Development workflow

```bash
cargo fmt --all # apply formatting
cargo clippy --workspace --all-targets # lint everything, including tests
```

Before starting any new work, read the sections above — they cover what
CI enforces and the four things this project is opinionated about. The
[Architecture guide](https://confluxfl.dev/guides/architecture/) explains
how the pieces fit and why.

## Load / concurrency testing

`crates/conflux-server/tests/load.rs` spins up 30 concurrent simulated
clients across 3 rounds against a real running server and reports timing:

```bash
cargo test -p conflux-server --test load -- --nocapture
```


