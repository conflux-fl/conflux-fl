# Bulyan — El Mhamdi et al. 2018

Reproduces **Bulyan** from *The Hidden Vulnerability of Distributed
Learning in Byzantium* (El Mhamdi, Guerraoui, Rouault — ICML 2018).

This is the **worked example** in [Add a baseline, step by
step](https://conflux-fl docs/guides/baselines-add) — a *manifest-only*
reproduction: no new client code, because the Burn `burn_mlp` example
already drives any cataloged aggregator by name.

## Method

Conflux FL's `bulyan` aggregator (a `FilteredAggregator<BulyanFilter,
CoordinateWiseAggregator<TrimmedMeanStatistic>>`), a shipped, cited catalog
entry. Bulyan runs a Krum-style selection and then a trimmed mean over the
selected updates.

## Precondition: n ≥ 4f+3

Bulyan needs at least `4f+3` participants for `f` Byzantine clients. With
one attacker (f=1) that is ≥ 7, so this baseline uses **8 clients**. This
is why it ships a **Rust edge only**: `run_demo.sh` pins
`byzantine_fraction = 0.3`, which at 8 clients would demand n ≥ 11 and fail
fast — a good illustration of a method enforcing its own preconditions.

## Result (Rust / Burn edge)

| Setting | Held-out accuracy |
|---|---|
| synthetic non-IID, 8 clients, 1 poisoned, 8 rounds | **0.91** (deterministic, seed 0) — vs FedAvg's 0.54 collapse |

## Run it

```bash
cargo run -p conflux-baselines -- run bulyan-elmhamdi-2018 --client rust
cargo run -p conflux-baselines -- run bulyan-elmhamdi-2018 --client rust --plan
```
