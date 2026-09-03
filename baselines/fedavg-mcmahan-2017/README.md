# FedAvg — McMahan et al. 2017

Reproduces **Federated Averaging** from *Communication-Efficient Learning
of Deep Networks from Decentralized Data* (McMahan, Moore, Ramage,
Hampson, Agüera y Arcas — AISTATS 2017,
[arXiv:1602.05629](https://arxiv.org/abs/1602.05629)).

## Method

FedAvg is Conflux FL's `fedavg` aggregator (a
`WeightedAverageAggregator<SampleCountWeighting>`), paired with the
`uniform_random` selector for the paper's C-fraction client sampling.
Both are shipped, cited catalog entries — this baseline *configures*
them, it does not re-implement them.

## Expected results (both edges)

| Edge | Setting | Held-out accuracy | Source |
|---|---|---|---|
| **rust** (Burn) | synthetic non-IID, 5 clients, 8 rounds | **0.95 ± 0.05** | Burn MLP, deterministic (seed 0) — what `verify` asserts |
| **python** `[smoke]` | MNIST, 5 clients, 15 rounds, IID | **0.90 ± 0.06** | `docs/E2E_TESTING.md` (0.905 @ round 15) |
| **python** `[full]` | MNIST, 100 clients, C=0.1, 500 rounds | ~0.97+ (paper trend) | the paper — real hardware |

The `[full]` regime needs real hardware (100 real clients is past a
laptop's ceiling — see the client-simulation cost guide).

## Run it

```bash
# Rust (Burn) edge — fast, deterministic, no Python:
cargo run -p conflux-baselines -- run fedavg-mcmahan-2017 --client rust

# Python (MNIST) edge — needs the venv:
cargo run -p conflux-baselines -- run fedavg-mcmahan-2017 --client python

# the paper-faithful Python run (heavy):
cargo run -p conflux-baselines -- run fedavg-mcmahan-2017 --client python --full

# just validate + see the plan:
cargo run -p conflux-baselines -- run fedavg-mcmahan-2017 --client rust --plan
```

Python setup for that edge is the same as the reproduction tutorial — a
venv under `python/conflux_client` with the `e2e_pytorch_mnist`
requirements.

## Files

- `baseline.toml` — the manifest (method, both client edges, expected).
- `results/` — committed reference results to match (populated on first
  verified run).
