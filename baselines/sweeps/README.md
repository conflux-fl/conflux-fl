# Sweeps

A baseline asks whether one method reproduces its paper's number. A
sweep asks the comparative question next to it: given the same model,
the same data and the same attack, how do methods differ from each
other — and does a difference that shows up on synthetic vectors
survive contact with a real model and a real dataset?

Every combination is a real federation. Real training, real gRPC, real
aggregation, one process per client. That is the expensive choice and
the entire point, because the cheap version is what a sweep exists to
check.

## Running one

```bash
cargo build -p conflux-server -p conflux-node
cargo run -p conflux-baselines -- sweep \
  --dataset mnist --model mlp \
  --aggregators fedavg krum multi_krum trimmed_mean median \
  --splits iid dirichlet:0.5 dirichlet:0.1 \
  --attacks none poison \
  --clients 5 --rounds 15 \
  --out baselines/sweeps/results_mnist_robustness.jsonl
```

`--attacks poison` gives one client a persistent Byzantine update *and*
turns the reputation pre-filter off. Both, deliberately: with the filter
on, a separate defense may catch the attacker before the aggregator ever
sees it, every method scores the same, and the sweep measures the filter
rather than the method it was pointed at.

Output is appended, not overwritten, so a grid can be extended across
several invocations. Delete the file for a clean run.

## Reading one

```bash
python3 baselines/sweeps/summarize_sweep.py results_mnist_robustness.jsonl
python3 baselines/sweeps/plot_sweep.py results_mnist_robustness.jsonl
```

`summarize_sweep.py` needs nothing beyond the standard library;
`plot_sweep.py` needs matplotlib.

## What a record holds

One JSON object per round per combination:

| field | meaning |
|---|---|
| `dataset`, `aggregator`, `split`, `dirichlet_alpha`, `attack`, `n_clients` | which combination |
| `round`, `held_out_accuracy`, `held_out_loss` | what the evaluator saw that round |
| `centralized_baseline_accuracy` | the same model trained on pooled data for the same total gradient steps |

The centralized figure is the bar, not a competitor: federated training
is not expected to beat it. What matters is the size of the gap, and an
aggregator that collapses under attack shows up as that gap widening
rather than as an absolute number nobody can calibrate. It depends only
on the model, the data and the step budget — none of which a grid varies
— so it is computed once per sweep and shared across every row.
