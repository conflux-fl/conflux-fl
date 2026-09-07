# Bulyan — El Mhamdi et al. 2018

Reproduces **Bulyan** from *The Hidden Vulnerability of Distributed
Learning in Byzantium* (El Mhamdi, Guerraoui, Rouault — ICML 2018).

This is the **worked example** in [Add a baseline, step by
step](https://confluxfl.dev/guides/baselines-add/) — a *manifest-only*
reproduction: no new client code, because the Burn `burn_mlp` example
already drives any cataloged aggregator by name.

## Method

Conflux FL's `bulyan` aggregator (a `FilteredAggregator<BulyanFilter,
CoordinateWiseAggregator<TrimmedMeanStatistic>>`), a shipped, cited catalog
entry. Bulyan runs a Krum-style selection and then a trimmed mean over the
selected updates.

## Precondition: n ≥ 4f+3

§4 of the paper is explicit: *"Bulyan(A) requires n ≥ 4f + 3 received
gradients"*. With one attacker (f=1) that is ≥ 7, so this baseline uses
**8 clients**.

That precondition is also why this manifest sets
`[scenario] byzantine_fraction = 0.125` rather than taking the
framework's 0.3 default. At 0.3, `f` is 30% of `n`, and `n ≥ 1.2n + 3`
has no solution — *no* client count would satisfy the precondition.

The implementation would not stop you. `byzantine_count` floors and
clamps and `theta` saturates, so at the default it computes `f = 2` from
8 clients, selects 4 of them, and runs perfectly happily — outside the
regime the paper makes any claim about. A method quietly operating
outside its own precondition is the failure this baseline exists to
notice, so it pins the fraction that keeps it inside.

## Results

| Edge | Setting | Held-out accuracy |
|---|---|---|
| **rust** (Burn) | synthetic non-IID, 8 clients, 1 poisoned, 8 rounds | **0.91 ± 0.05** (deterministic, seed 0) — vs FedAvg's 0.54 collapse |
| **python** (shared harness) | MNIST, 8 clients, 1 poisoned, 15 rounds | **0.91 ± 0.05** — measured 0.918 |

The target is a measurement rather than the paper's own figure: El
Mhamdi et al. report convergence *curves* for MNIST and CIFAR-10, not a
tabulated accuracy, so there is no number to reproduce against — only a
regime to stay inside.

## Run it

```bash
cargo run -p conflux-baselines -- run bulyan-elmhamdi-2018 --client rust
cargo run -p conflux-baselines -- run bulyan-elmhamdi-2018 --client python
cargo run -p conflux-baselines -- run bulyan-elmhamdi-2018 --client rust --plan
```
