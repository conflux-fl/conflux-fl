# AGENT.md — the DSS research line

Entry point for any agent picking up the **Deviation Stability Scoring**
research. Read this, then `PROGRESS.json`, then `tasks.json`, and you
should be able to start working without asking anyone anything.

Scoped deliberately: this covers the *research*, not the Conflux FL
framework. `/CLAUDE.md` at the repo root is the framework's entry point
and takes precedence for anything about crates, phases, or shipping code.

## What this project is

DSS is a **research hypothesis**, not a shipped default: an aggregation
wrapper that scores each client on the *temporal* behavior of its
deviation from a reference point, penalizing clients that are both
unstable across rounds **and** suspiciously correlated with another
client. It exists to attack a gap the framework's own catalog can't
close — no stateless method can distinguish a colluding Sybil cluster
from a legitimate majority within a single round, since both can produce
the same single-round batch geometry.

`DssAggregator` is deliberately **not** registered in `build_aggregator`'s
catalog. It is constructed directly by experiment runners. Do not add it
to the catalog; that would make an unvalidated method selectable by
config, against ADR 0008's discipline.

## Where everything lives

This project predates the harness structure, so most of what a
`docs/RESEARCH_PLAN.md` / `EXPERIMENT_LOG.md` / `LITERATURE.md` would
hold already exists inside one long document. Those files were
deliberately **not** created, because duplicating a 1,400-line document
into six thinner ones produces drift, not clarity.

| What you want | Where it is |
|---|---|
| Research plan, hypothesis, claims | `temporal-consistency-aggregation.md` §3, §6, §7 |
| Experiment log (every run, every finding) | same doc, §5.1–§5.13 |
| Literature & novelty positioning | same doc, §6.5 and References |
| Baseline numbers, consolidated | `BASELINES.md` (this directory) |
| Research decisions & their rationale | `/docs/adr/` (framework-level), and each §5 subsection's own "what this does not license" paragraphs |
| Code architecture rules | `/CLAUDE.md`, `/docs/ARCHITECTURE.md` |
| Current state, next actions | `PROGRESS.json`, `tasks.json` (this directory) |

## Repository map for this research

```
docs/research/
├── AGENT.md                                 # this file
├── PROGRESS.json                            # session handoff state
├── tasks.json                               # atomic task breakdown
├── BASELINES.md                             # consolidated reference numbers
├── temporal-consistency-aggregation.md      # the research document itself
├── scripts/                                 # one .sh per experiment + lib.sh + summarizers
├── results/                                 # .jsonl (raw) + .csv + .summary.csv
└── figures/

crates/conflux-core/src/temporal.rs          # DssAggregator, FoolsGold, CenteredClipping
crates/conflux-attacks/                      # the attacks; examples/ holds the runners
```

The experiment runners are `conflux-attacks` **examples**, not a separate
crate — research tooling, not product (ADR 0010).

## How to run an experiment

```bash
# One configuration, one JSON line per round, to stdout:
cargo run --release --example run_experiment -p conflux-attacks -- \
  --aggregator dss_krum --attack persistent_sybil --rounds 20 --seed 1

# A full sweep (builds the runner, writes .jsonl into results/):
./docs/research/scripts/experiment_2_8_finding3_fix.sh

# Summarize (mean ± 95% CI per aggregator × attack):
python3 docs/research/scripts/summarize.py docs/research/results/<file>.jsonl
```

Aggregator-name prefixes the runner understands, beyond the shipped
catalog: `dss_<base>` (DSS wrapping a base), `dssraw_<base>` (DSS with
the pre-Finding-3 combine, for A/B), `dssstab_<base>` / `dsscoll_<base>`
(stability-only / collusion-only ablations).

Real-data runs go through `python/conflux_client/examples/benchmark.py`,
which sweeps (aggregator × split × attack) over an e2e demo directory.

## Constraints that are not negotiable

1. **Report what the numbers show, not what the hypothesis wants.** This
   document's most valuable entries are the ones where a prior
   conclusion was overturned by measurement (§5.8.1, §5.11, §5.12,
   §5.13). Three of them overturned claims made in this same document.
   If a result contradicts the hypothesis, that is the result.
2. **≥5 seeds, and report the confidence interval.** A single-seed number
   is not a finding. Where a run *is* single-seed (§5.13), say so in the
   same breath as the number.
3. **Never edit a results file by hand.** Re-run the script. Every
   `.jsonl` in `results/` must be reproducible from its `.sh`.
4. **Don't rewrite history when a finding is superseded.** Annotate the
   old claim in place with a pointer to what replaced it — see how
   Finding 3's original statements are marked. The record of having been
   wrong is part of the evidence.
5. **DSS stays out of `build_aggregator`.** See above.

## Self-check before reporting a result

- Did the experiment script run to completion, or did you read a partial
  file?
- Is the comparison against the *bare base method*, not just against
  `fedavg`? Finding 3 hid for two experiments because the natural
  comparison wasn't being made.
- Does the confidence interval overlap the thing you're claiming is
  different from?
- Would this number change if the model dimension changed? §5.13 is the
  cautionary case: a τ optimum found at `dim = 3` did not survive
  `dim = 50,890`.

## Known open problems

Tracked in `tasks.json`. The three that matter most:

- DSS-on-`fedavg` still fails under a **solo** attacker, and §5.8.1
  isolated why: the shared deviation reference isn't robust when the
  base isn't. Not an arithmetic bug — a design one.
- The **AND-gate is trading away something real** (§5.12): collusion-only
  catches attacks stability-only misses entirely. Changing it needs the
  non-IID fairness test first, or it reopens Claim 2.
- `clip_radius = 1.0` is a **placeholder default that loses to no
  defense** on a real model (§5.13).
