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
obvious alternative was rejected. Comments cite architecture decisions
as `ADR NNNN`; the citation exists so a comment can name a decision
without restating it.

**3. A new aggregation method must be a literal implementation of a
published paper**, cited in its doc comment (ADR 0008 — the framework
ships literal implementations, not variants). This is a catalog
researchers compare against, so "our improved variant of Krum" is a
different project — build it against the public API from your own
repository, which
[docs/EXTENDING.md](docs/EXTENDING.md) explains how to do. It joins the
catalog when it is published.

**4. A demonstration that cannot fail is not evidence.** Before
believing a benchmark or a demo, check what result would have falsified
it. This project shipped a client demo whose data was split so evenly
that every client scored 1.000 *before* federating — the federated
number looked excellent and meant nothing.

## Adding an aggregation method

[docs/EXTENDING.md](docs/EXTENDING.md) has the steps. The short version:
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
