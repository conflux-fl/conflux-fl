# Option A: NumPy logistic regression end-to-end demo

Trains a real (if tiny) model — logistic regression, plain NumPy, no
PyTorch — across several simulated clients through the real Conflux
pipeline (`conflux-server` + `conflux-node`, real gRPC, real rounds), and
compares the result against a centralized baseline trained on the same
data without Conflux. This is the fastest way to prove the whole
orchestration actually works end-to-end, and it's a good first stop if
you've never run this Rust framework before — everything here is
scripted, nothing requires editing Rust code.

See [`docs/E2E_TESTING.md`](../../../../docs/E2E_TESTING.md) for the
full design rationale (why this model/dataset, what "working" means,
Option B for a higher-fidelity PyTorch/MNIST version).

## Prerequisites

- Rust toolchain (the repo already builds — if you haven't yet, `cargo
  build --workspace` from the repo root once, to confirm your setup
  works, before running this demo).
- Python 3.10+ and the shared venv this repo's other Python tooling
  uses:

  ```bash
  cd python/conflux_client
  python3 -m venv .venv
  .venv/bin/pip install -r requirements.txt              # grpc (base)
  .venv/bin/pip install -r examples/e2e_numpy_logreg/requirements.txt  # numpy, scikit-learn
  ./generate_proto.sh                                     # regenerates fl_transport_pb2*.py
  ```

  (`generate_proto.sh` isn't committed output — you need to run it once;
  see the parent `README.md` if you haven't before.)

## Run it

```bash
source .venv/bin/activate   # from python/conflux_client/
cd examples/e2e_numpy_logreg
./run_demo.sh                       # fedavg, 5 clients, 15 rounds — the defaults
./run_demo.sh krum 5 15             # a robust aggregator instead
./run_demo.sh krum 5 15 --poison    # + one persistent Byzantine client
```

The script builds the Rust binaries, generates a synthetic dataset,
partitions it across the clients, starts one `conflux-server`, one
`conflux-node` per client (+ one more for evaluation), the trainer
processes, and an eval process that prints held-out accuracy as rounds
complete. Everything is cleaned up (all background processes killed) when
the script exits — the working directory (with every process's logs) is
printed at the end and left in place for inspection.

## What you should see

A `held_out_accuracy=` line every time the eval client observes a new
round, converging toward (and normally landing within a couple points
of) the centralized baseline printed in step 3. Real run, no attack:

```
=== 3. centralized baseline (target accuracy) ===
held_out_accuracy=0.7375

=== 6. starting 5 trainer clients + 1 eval client ===
round=8  held_out_accuracy=0.7425
round=14 held_out_accuracy=0.7350
round=16 held_out_accuracy=0.7350
```

That's federated training through Conflux landing within half a point of
centralized training on the same data — the actual correctness bar, not
just "the loss went down."

## A real finding: reputation filtering has its own blind spot

Running `--poison` with the *default* config (reputation filtering on,
`min_reputation_score = 0.3`) shows something worth understanding before
you conclude a robust aggregator "isn't working":

```
./run_demo.sh krum 5 15 --poison
# ... accuracy collapses to ~0.39, same as plain fedavg would ...
```

This isn't Krum failing. `conflux-reputation`'s cosine-similarity filter
runs **before** aggregation (spec §8's pipeline order), scoring every
update against the batch's raw mean. In round 1 — when every client
starts from the same (typically zero) initial checkpoint — a single
large-magnitude attacker can skew that mean so far that **every honest
update looks anomalous relative to it**, gets rejected by reputation, and
never reaches Krum at all. Krum then aggregates a batch of one (the
attacker), and every later round trains forward from an already-poisoned
checkpoint. Verified directly in the server log:

```
update rejected by reputation filter client_id=client-1 score=-0.50 threshold=0.3
update rejected by reputation filter client_id=client-2 score=-0.43 threshold=0.3
update rejected by reputation filter client_id=client-3 score=-0.41 threshold=0.3
update rejected by reputation filter client_id=client-0 score=-0.49 threshold=0.3
```
— all four honest clients, only in round 1.

`--no-reputation` isolates the aggregator's own defense from this
interaction (sets `min_reputation_score` low enough that nothing gets
filtered before aggregation runs):

```
./run_demo.sh krum 5 15 --poison --no-reputation
# held_out_accuracy stays 0.72-0.7375 — the attacker is excluded, matching
# the centralized baseline, exactly what Phase 12's unit/application
# tests already proved in isolation — now confirmed live.
```

The full comparison, all measured on this exact dataset/seed:

| Scenario | Held-out accuracy | vs. baseline (0.7375) |
|---|---|---|
| `fedavg`, no attack | 0.735–0.7425 | matches |
| `fedavg`, `--poison` (reputation on, default) | 0.3975 | broken |
| `krum`, `--poison` (reputation on, default) | 0.3975 | broken — same reputation issue, not Krum's fault |
| `fedavg`, `--poison --no-reputation` | 0.3925 | broken — expected, FedAvg has no defense |
| `krum`, `--poison --no-reputation` | 0.72–0.7375 | **defended** — matches baseline |

**Takeaway**: a `robust` aggregator is not, by itself, a complete defense
against a first-round large-magnitude attacker in the current pipeline —
reputation filtering needs to either run after aggregation, use a
robust reference point instead of a raw mean, or be tuned/disabled for
deployments expecting this specific attack shape. This is tracked as a
real, open finding — see `docs/STATUS.md`'s "Next" section and
`docs/E2E_TESTING.md`'s "A real finding" section, not fixed by this demo
itself.

## Options

```
./run_demo.sh [AGGREGATOR] [N_CLIENTS] [ROUNDS] [--poison] [--no-reputation]
```

- `AGGREGATOR`: `fedavg` (default), `krum`, `multi_krum`, `trimmed_mean`,
  `median`.
- `--poison`: the last client persistently submits offset weights
  instead of training, every round (not `stub_client.py`'s single shot —
  a real multi-round Byzantine client).
- `--no-reputation`: see above.

## Files

- `partition_data.py` — generates a synthetic dataset
  (`sklearn.datasets.make_classification`) and splits it (IID by
  default; `--split dirichlet` for realistic non-IID).
- `model.py` — plain NumPy logistic regression; weights are already a
  flat vector, so no flatten/unflatten step is needed (unlike Option B).
- `trainer_client.py` — a real client: loads its own shard only, loops
  fetch/train/submit across rounds.
- `eval_client.py` — fetch-only, scores the current checkpoint against a
  held-out set nothing ever trains on.
- `centralized_baseline.py` — the correctness bar.
- `run_demo.sh` — orchestrates all of the above.

## Troubleshooting

- **"server did not become healthy"**: something's already listening on
  `127.0.0.1:50051`/`8080` — check `pgrep -af conflux-server` and kill
  any leftover process from a previous run that didn't clean up (e.g.
  the script was killed with `SIGKILL`, which skips the `trap cleanup`).
- **Accuracy stuck near 0.5 with no attack**: the learning rate/step
  count might not suit a change you made to `--n-features`; the defaults
  (10 features, `--lr 0.5`, 5 steps/round) are tuned for the demo's own
  synthetic dataset.
- **A trainer logs "submission rejected (FAILED_PRECONDITION)"**: normal
  — a submission raced an already-closed round (Phase 10a's fix
  rejecting it explicitly rather than silently losing it); the client
  retries with the next round automatically.
