# Conflux FL Baselines

Runnable reproductions of published federated-learning papers on Conflux
FL — the same idea as Flower's `baselines/`, adapted to a Rust framework
whose methods already live in the [aggregation
catalog](../docs/AGGREGATION_CATALOG.generated.md). A baseline is a
**reproduction recipe**, not a re-implementation: a `baseline.toml` naming a
cataloged method plus the paper's setup and expected results, reproducible
by **two client edges** — Python (PyTorch) and Rust (Burn) — and driven and
verified by the `conflux-baselines` runner.

Design + manifest schema: the [Baselines guide](https://confluxfl.dev/guides/baselines/) and [Add a baseline](https://confluxfl.dev/guides/baselines-add/).

## Reproduced papers

<!-- TODO: generate this table from the manifests, with a golden-file
     test forbidding drift (as docs/AGGREGATION_CATALOG.generated.md has). -->

| Baseline | Paper | Method | Edges | Scenario | Rust result |
|---|---|---|---|---|---|
| [`fedavg-mcmahan-2017`](fedavg-mcmahan-2017/) | McMahan et al. 2017 | `fedavg` | python, rust | clean | 0.95 |
| [`krum-blanchard-2017`](krum-blanchard-2017/) | Blanchard et al. 2017 | `krum` | python, rust | 1 attacker | 0.71 (fedavg collapses to 0.54) |
| [`trimmed-mean-yin-2018`](trimmed-mean-yin-2018/) | Yin et al. 2018 | `trimmed_mean` | python, rust | 1 attacker | 0.98 (fedavg collapses to 0.54) |
| [`bulyan-elmhamdi-2018`](bulyan-elmhamdi-2018/) | El Mhamdi et al. 2018 | `bulyan` | rust | 1 attacker (n≥4f+3 → 8 clients) | 0.91 (fedavg collapses to 0.54) |

The **Rust result** is the Burn edge on a synthetic non-IID problem,
deterministic (seed 0) — what `verify` asserts. The **Python edge**
reproduces the same method on MNIST (numbers in each baseline's README).

## Two client edges, one paper

Every baseline can be reproduced by either edge — the same paper, two
training stacks:

- **Rust (Burn)** — `crates/conflux-client/examples/burn_mlp.rs`. A real
  Burn MLP `ClientApp`, `ndarray` CPU backend, feeding the **real cited**
  `conflux-core` aggregators. Fast, deterministic, needs no Python.
- **Python (PyTorch)** — the `python/conflux_client/examples/e2e_*`
  harnesses. Drives real MNIST/CIFAR training over the full gRPC pipeline.

## Use it

```bash
cargo run -p conflux-baselines -- list                                 # every baseline + its edges
cargo run -p conflux-baselines -- run krum-blanchard-2017 --client rust # reproduce via Burn
cargo run -p conflux-baselines -- run krum-blanchard-2017 --client python
cargo run -p conflux-baselines -- verify --ci                          # assert every Rust edge (CI-friendly)
cargo run -p conflux-baselines -- run <name> --client rust --plan      # validate + plan, no run
```

`verify` runs the **Rust edge** of every baseline — fast, deterministic,
and needs no Python — so it is what a CI gate would run to keep the
reproductions green.

## Add a baseline

1. Its method must already be in the catalog (`cargo run -p conflux-core
   --example catalog`). If not, add the method first — a baseline only
   reproduces a *shipped, cited* method — the catalog is where a method
   is justified against its paper, so a baseline never re-implements one.
2. Copy an existing `<author-year-method>/`, edit `baseline.toml`
   (`[paper]`, `[method]`, `[experiment]`, optional `[scenario]`, and one
   or both `[clients.python]` / `[clients.rust]` edges).
3. `... run <name> --client rust --plan` to validate the manifest and see
   the plan; then run it for real, record the number into `[expected]`.

## Layout

- `<baseline>/baseline.toml` — the manifest (source of truth).
- `<baseline>/README.md` — paper, expected results, how to run.
- `_harness/` — shared Python training library (planned; today the Python
  edge points at an existing `e2e_*` example).
