# 0008 — Cited baseline implementations

## Context
Federated learning has an active research literature with specific,
attributable methods behind even the "obvious" defaults (client sampling,
DP noise mechanism). Shipping an implementation without a citation risks
silently drifting from the published method it's supposed to represent, and
makes it harder for a reader to verify correctness against the source.

## Decision
Each shipped family member is the literal implementation of a cited paper,
docstring-cited in the code:
- `UniformRandomSelector` — McMahan, Moore, Ramage, Hampson & y Arcas (2017),
  *Communication-Efficient Learning of Deep Networks from Decentralized
  Data*, AISTATS — the client-sampling strategy from the original FedAvg
  algorithm.
- `GaussianClippingPrivacy` — Abadi et al. (2016), *Deep Learning with
  Differential Privacy*, ACM CCS; Geyer, Klein & Nabi (2017), *Differentially
  Private Federated Learning: A Client Level Perspective*.
- `RdpAccountant` — Mironov (2017), *Rényi Differential Privacy*, IEEE CSF;
  Wang, Balle & Kasiviswanathan (2019), *Subsampled Rényi Differential
  Privacy and Analytical Moments Accountant*, AISTATS.

See spec §5–§6 for the full reference list and defaults derived from each
paper (e.g. `clip_norm = 1.0`, `target_epsilon = 8.0`).

## Consequences
- Each family (see [[0002-family-pattern]]) ships exactly one cited member
  in v1 — breadth of coverage is a Phase 8 concern, not a v1 goal.
- Defaults are taken from the cited papers, not chosen arbitrarily —
  changing a default requires re-justifying against the literature or
  documenting the deviation in `docs/STATUS.md`'s "Known deviations" section.
- New family members added later (Phase 8 onward) must carry the same
  citation discipline.

## Update (Phase 11a) — the `robust` family's members

- `KrumFilter`/`MultiKrumFilter` — Blanchard, El Mhamdi, Guerraoui &
  Stainer (2017), *Machine Learning with Adversaries: Byzantine Tolerant
  Gradient Descent*, NeurIPS 2017.
- `TrimmedMeanStatistic`/`MedianStatistic` — Yin, Chen, Ramchandran &
  Bartlett (2018), *Byzantine-Robust Distributed Learning: Towards
  Optimal Statistical Rates*, ICML 2018, PMLR 80.

Same discipline applied to the adversary side in Phase 12
(`conflux-attacks`): every attack implementation is cited to the paper
that describes it, the same way every defense is — see that phase's
brief for the full list.
