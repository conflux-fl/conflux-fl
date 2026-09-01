# Baselines

Reference numbers, consolidated. Every figure here is copied from a
`results/*.summary.csv` produced by a script in `scripts/` — nothing is
typed from memory, and nothing is rounded from a different run than the
one named.

Metric throughout is **mean distance-from-true-value over the run**,
lower is better, 8 honest clients + 2 attackers, `dim = 3`, 20 rounds,
5 repeats, unless a row says otherwise. Cross-experiment comparisons are
only valid within a column — the attacks differ in magnitude, so 16.99
against `persistent_sybil` and 171.47 against `scaling` are not the same
kind of number.

## The core comparison (Experiment 2.2 / 2.8)

Colluding Sybil pair, three attack shapes.

| aggregator | `persistent_sybil` | `adaptive_evasion` | `scaling` |
|---|---|---|---|
| `fedavg` | 16.989 | **161.345** | 171.473 |
| `krum` | 0.297 | 0.297 | 0.297 |
| `multi_krum` | **0.173** | **0.173** | **0.173** |
| `trimmed_mean` | 0.273 | 0.273 | 0.273 |
| `median` | 0.257 | 0.257 | — |
| `geometric_median` | 0.243 | 0.240 | — |
| `median_of_means` | 0.233 | 0.233 | — |
| `faba` | 0.173 | 0.173 | — |
| `bulyan` | 0.196 | 0.198 | — |
| `divide_and_conquer` | 0.173 | 0.173 | — |
| `foolsgold` | 1.345 | 1.345 | 1.345 |
| `centered_clipping` (τ=1.0) | 10.683 | 10.683 | 165.173 |

`fedavg` is the undefended control. Note it is *not* uniformly terrible —
against `persistent_sybil` it scores 16.99 while `centered_clipping`
scores 10.68; the catastrophic case is `adaptive_evasion`, where it
diverges by 32× over the run.

## DSS (Experiment 2.4 / 2.8)

Both combines shown, because the difference between them *is* Finding 3.
`dssraw_` is the original combine; `dss_` is the current one.

| aggregator | `persistent_sybil` | `adaptive_evasion` | `scaling` |
|---|---|---|---|
| `dss_fedavg` | 16.989 | **1.178** | 171.473 |
| `dss_krum` | 0.297 | 0.297 | 0.297 |
| `dss_multi_krum` | 0.173 | 0.198 | 0.173 |
| `dss_trimmed_mean` | 0.273 | **0.203** | 0.273 |
| `dssraw_krum` | 16.989 | 1.013 | 171.473 |
| `dssraw_multi_krum` | 16.989 | 1.013 | 171.473 |
| `dssraw_trimmed_mean` | 16.989 | 1.013 | 171.473 |

Two things to read off this table:

- **DSS's one genuine win**: `dss_fedavg` vs `adaptive_evasion`, 161.345
  → 1.178. That is the hypothesis working, on the one attack shape
  (temporally unstable colluders) it targets.
- **Finding 3, and its repair**: every `dssraw_` row collapses to
  `fedavg`'s numbers, because the old combine discarded the base
  method's selection whenever DSS's gate didn't fire. The `dss_` rows now
  match their bare bases. `dss_trimmed_mean` at 0.203 even improves on
  bare `trimmed_mean`'s 0.273.

## Ablation (Experiment 2.5 / 2.9)

Which half of the "unstable AND colluding" gate does the work.

| variant | `persistent_sybil` (identical) | `correlated_sybil` (non-identical, stable) |
|---|---|---|
| `fedavg` | 16.989 | 17.128 |
| `dss_fedavg` (AND) | 16.989 | 17.128 |
| `dssstab_fedavg` (stability only) | 16.989 | 17.128 |
| `dsscoll_fedavg` (collusion only) | **1.083** | **1.094** |
| `foolsgold` | 1.345 | **7.535** |
| `krum` | 0.297 | 0.297 |

The AND-gate is numerically identical to stability-only in both columns,
and stability-only is identical to *no defense*. Collusion-only is ~15×
better. §5.6 saw the first column and couldn't tell whether the collusion
signal was redundant or merely unmeasurable; the second column settles
it.

FoolsGold's row is the other finding here: 1.345 → 7.535 when colluders
stop being byte-identical.

## Solo attacker (Experiment 2.6)

One attacker, no colluding partner. Different design, so do not compare
across to the tables above.

| aggregator | `persistent_sybil` | `adaptive_evasion` |
|---|---|---|
| `fedavg` | 8.504 | 80.682 |
| `krum` | 0.300 | 0.300 |
| `dss_krum` | 0.300 | 0.300 |
| `dss_fedavg` | 8.504 | **36.340** |
| `foolsgold` | 1.791 | 8.104 |

`dss_fedavg` at 36.34 is the **open failure** (task r1): better than
undefended `fedavg`'s 80.68, but nowhere near converged, and unchanged by
the `f64` precision fix. Cause isolated in §5.8.1.

## Centered Clipping's clip radius (Experiment 2.7)

Synthetic, `dim = 3`. Included because §5.13 shows it does not transfer.

| τ | `persistent_sybil` | `adaptive_evasion` | `scaling` |
|---|---|---|---|
| 0.25 | 15.403 | 15.403 | 169.898 |
| 1.0 | 10.683 | 10.683 | 165.173 |
| 4.0 | **3.319** | **3.319** | 146.273 |
| 16.0 | 4.230 | 4.230 | **72.975** |

A real optimum at τ = 4.0 for the first two columns. See below for why
that optimum is not a recommendation.

## Real data (Experiments 3.1 / 3.2)

MNIST, a real 50,890-parameter PyTorch MLP, 5 clients, 6 rounds,
**1 seed per cell**. Metric is held-out accuracy, **higher is better** —
the opposite direction from every table above. Centralized baseline:
0.852.

| aggregator | clean | poisoned |
|---|---|---|
| `fedavg` | 0.884 | 0.163 |
| `krum` | 0.857 | **0.844** |
| `trimmed_mean` | 0.878 | **0.875** |
| `centered_clipping` (τ=1.0) | 0.884 | **0.078** |

Clip-radius sweep on the poisoned cell:

| τ | 1.0 | 5.0 | 20.0 | 100.0 | (`fedavg`) |
|---|---|---|---|---|---|
| accuracy | 0.078 | 0.126 | 0.152 | 0.153 | 0.163 |

Monotonic toward FedAvg, no optimum. The synthetic τ = 4.0 optimum does
not transfer, because τ bounds an L2 norm in parameter space and this
model has 50,890 parameters rather than 3.

## Shakespeare harness reference

Character-level GRU, one client per speaking role (natural non-IID),
held-out drawn from roles no client trains on. Chance is 1/65 ≈ 0.015.

| | accuracy |
|---|---|
| centralized baseline | 0.204 |
| federated, round 1 | 0.017 |
| federated, round 5 | 0.171 |

Convergence-only smoke reference so far — no aggregator comparison has
been run on this harness yet (task r5).

## FLANDERS comparison (Experiment 2.10)

Final-round distance, mean ± 95% CI over 5 seeds. 8 honest + 2
attackers unless the malicious ratio says otherwise. Source:
`results/experiment_2_10_flanders_comparison.summary.csv`.

| aggregator | `adaptive_evasion` | `persistent_sybil` | `correlated_sybil` | `scaling` |
|---|---|---|---|---|
| `fedavg` | 553.045 ± 0.07 | 17.010 ± 0.07 | 17.155 ± 1.16 | 171.473 ± 0.00 |
| `dss_fedavg` | **0.635 ± 0.34** | 17.010 ± 0.07 | 17.155 ± 1.16 | 171.473 ± 0.00 |
| `dsscoll_fedavg` | **0.310 ± 0.03** | **0.255 ± 0.05** | **0.251 ± 0.06** | **0.267 ± 0.04** |
| `flanders_fedavg` | 9.412 ± 7.46 | 24.247 ± 0.05 | 24.511 ± 1.62 | 147.079 ± 117.50 |
| `flanders_krum` | 0.421 ± 0.19 | 0.326 ± 0.15 | 0.358 ± 0.16 | 0.294 ± 0.13 |
| `krum` | 0.299 ± 0.12 | 0.257 ± 0.12 | 0.257 ± 0.12 | 0.257 ± 0.12 |
| `foolsgold` | 1.391 ± 0.12 | 1.391 ± 0.12 | 8.747 ± 2.77 | 1.392 ± 0.12 |

Majority-attacker regime, `adaptive_evasion`:

| malicious | `fedavg` | `dss_fedavg` | `flanders_fedavg` | `krum` |
|---|---|---|---|---|
| 20% | 553.0 | **0.64** | 9.4 | 0.30 |
| 60% | 1659.1 | **0.44** | 1901.7 | 2765.0 |
| 80% | 2212.1 | **7.19** | 2765.0 | 2765.0 |

Two things to read carefully here. `flanders_fedavg` is *worse than
undefended FedAvg* on every Sybil row and at 60% malicious — a stable
colluder is the most forecastable client in the batch, so a
forecast-consistency filter keeps it. And `flanders_krum` holds
throughout, because the paper's own `ϕ` is Krum and the filter is
carried by its base. See §5.14.

## Temporal non-IID fairness (Experiment 2.11)

Minority ÷ majority leave-one-out influence, mean ± 95% CI over 20
seeds, **zero attackers**, 20 rounds. Below 1 means the shifted honest
minority is down-weighted. Source:
`results/experiment_2_11_temporal_fairness.jsonl`.

| aggregator | shift 1.0 | shift 2.0 | shift 3.0 |
|---|---|---|---|
| `fedavg` | 1.835 ± 0.145 | 2.458 ± 0.089 | 2.718 ± 0.055 |
| `dss_fedavg` | 1.835 ± 0.145 | 2.458 ± 0.089 | 2.718 ± 0.055 |
| `dsscoll_fedavg` | 1.680 ± 0.363 | 1.278 ± 0.262 | **1.030 ± 0.177** |
| `krum` | 0.663 ± 0.533 | 0.636 ± 0.511 | 0.636 ± 0.511 |
| `flanders_krum` | 0.752 ± 0.346 | 0.709 ± 0.253 | 0.746 ± 0.256 |
| `foolsgold` | 1.451 ± 0.369 | 2.226 ± 0.482 | 3.180 ± 0.704 |

The one number that decides `r2`: `dsscoll_fedavg` at 1.030 ± 0.177 has
a confidence interval containing 1.0 — parity. Dropping DSS's stability
conjunct does not reopen Claim 2, while `krum` and `flanders_krum`, both
already shipped or published, do down-weight the minority. §5.15.

## Real-model validation (Experiment 3.3, §5.16)

Real MNIST, a 50,890-parameter PyTorch MLP, 3 clients, Dirichlet
`α = 0.5`, 6 rounds. Centralized baseline **0.852**. Held-out accuracy,
higher is better. Source:
`results/experiment_3_3_flanders_real_data.jsonl`.

**Single-seed** — the harness was fixed-seed until 2026-09-01 (task
`r4`). Read the direction, not the third decimal.

| aggregator | no attack | poisoned |
|---|---|---|
| `fedavg` | **0.839** | 0.181 |
| `krum` | 0.669 | **0.655** |
| `flanders` (FLANDERS + Krum) | 0.671 | **0.102** (best round 0.511) |
| `foolsgold` | 0.460 | 0.186 |

The row that matters: `flanders` **is** `krum` plus a pre-filter. With
no attack they are indistinguishable (0.671 vs 0.669). Under attack the
filter takes its base from 0.655 to 0.102 — below undefended FedAvg.
§5.14 found the same direction synthetically but only for the FedAvg
pairing; on a real model the paper's own Krum pairing is the harmful
one.

**DSS does not appear in this table**, and cannot: the real-data harness
drives the production server binary, which builds aggregators from a
catalog that deliberately excludes unvalidated research methods. See
§5.16.

## Real-model, three seeds (Experiment 3.4, §5.16.1)

The one comparison from §5.16 that carries the finding, repeated across
three data partitions. Same setup; final held-out accuracy under a
persistent Byzantine client. Source:
`results/experiment_3_4_flanders_multiseed.jsonl`.

| aggregator | 42 | 1337 | 2024 | mean ± 95% CI |
|---|---|---|---|---|
| `krum` | 0.718 | 0.703 | 0.647 | **0.689 ± 0.042** |
| `flanders` | 0.105 | 0.136 | 0.102 | **0.114 ± 0.021** |

Intervals `[0.647, 0.732]` vs `[0.093, 0.136]` — a 0.575 gap, identical
direction in every seed. `flanders` sits essentially at chance (0.100
for ten-class MNIST).

Two caveats that belong with the numbers: three seeds is not five, and
`CONFLUX_DEMO_SEED` seeds the data partition but **not** the trainers.
The residual run-to-run variance that leaves is measurable — the same
nominal seed 42 gave `krum` 0.655 in §5.16 and 0.718 here, a spread of
0.063 — and the finding's gap is 9.1× larger than it.
