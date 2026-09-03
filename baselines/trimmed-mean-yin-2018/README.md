# Trimmed Mean — Yin et al. 2018

Reproduces **Coordinate-wise Trimmed Mean** from *Byzantine-Robust
Distributed Learning: Towards Optimal Statistical Rates* (Yin, Chen,
Ramchandran, Bartlett — ICML 2018).

## Method

Conflux FL's `trimmed_mean` aggregator (a
`CoordinateWiseAggregator<TrimmedMeanStatistic>`), a shipped, cited catalog
entry. Per coordinate, it discards the most extreme values (sized by the
assumed Byzantine fraction) and averages the rest — so a poisoned client's
large offsets are trimmed away coordinate by coordinate.

## The reproduction is a *defense*

With one poisoned client present, FedAvg collapses; Trimmed Mean holds —
and, unlike Krum, it keeps averaging the surviving clients, so it retains
the multi-client signal on non-IID data.

## Both client edges

| Edge | Data | Result |
|---|---|---|
| **rust** (Burn) | synthetic non-IID + 1 poisoned client | fedavg 0.54 (collapse) → **trimmed_mean 0.98** (defends strongly); deterministic, seed 0 |
| **python** (PyTorch) | MNIST + 1 poisoned client | expected ≈ 0.86 — **provisional, confirm with a measured run** |

## Run it

```bash
# Rust (Burn) edge — fast, deterministic, no Python:
cargo run -p conflux-baselines -- run trimmed-mean-yin-2018 --client rust

# Python (MNIST) edge — needs the venv:
cargo run -p conflux-baselines -- run trimmed-mean-yin-2018 --client python
```

The Python expected number is provisional; run the Python edge once and
update `[clients.python].expected` in `baseline.toml` with the measured
value before treating it as verified.
