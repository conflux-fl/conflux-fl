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
