# Krum — Blanchard et al. 2017

Reproduces **Krum**, the Byzantine-tolerant aggregation rule from *Machine
Learning with Adversaries: Byzantine Tolerant Gradient Descent* (Blanchard,
El Mhamdi, Guerraoui, Stainer — NeurIPS 2017).

## Method

Conflux FL's `krum` aggregator (a `FilteredAggregator<KrumFilter, FedAvg>`),
a shipped, cited catalog entry. This baseline configures it; it does not
re-implement it. Krum selects the single update closest to its neighbors,
so a Byzantine client's outlier update is never chosen.

## The reproduction is a *defense*

With one poisoned client present, FedAvg collapses (the attacker pulls the
mean); Krum holds. `[scenario].no_reputation = true` isolates Krum's own
defense from Conflux's separate reputation filter.

## Both client edges

| Edge | Data | Result |
|---|---|---|
| **rust** (Burn) | synthetic non-IID + 1 poisoned client | fedavg 0.54 (collapse) → **krum 0.71** (defends); deterministic, seed 0 |
| **python** (PyTorch) | MNIST + 1 poisoned client | **≈ 0.88** (docs/E2E_TESTING.md: `krum --poison --no-reputation`) |

Krum's modest Rust number vs Trimmed-Mean's is its known non-IID weakness:
it picks *one* client's update, losing the other clients' features. Both
still beat the 0.54 FedAvg collapse — that gap is the reproduction.

## Run it

```bash
# Rust (Burn) edge — fast, deterministic, no Python:
cargo run -p conflux-baselines -- run krum-blanchard-2017 --client rust

# Python (MNIST) edge — needs the venv (see the reproduce-fedavg tutorial):
cargo run -p conflux-baselines -- run krum-blanchard-2017 --client python

# just validate + see the plan:
cargo run -p conflux-baselines -- run krum-blanchard-2017 --client rust --plan
```
