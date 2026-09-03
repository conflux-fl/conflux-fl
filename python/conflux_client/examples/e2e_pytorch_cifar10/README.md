# End-to-end: PyTorch + real CIFAR-10

The same harness shape as the MNIST demo (`../e2e_pytorch_mnist`), with a
small real CNN on real CIFAR-10 images instead of an MLP on digits. Only
`model.py` and `partition_data.py` are dataset-specific; the trainer, eval,
and centralized-baseline clients are shared in shape with the MNIST demo.

```bash
# from python/conflux_client with the venv active (see the repo README):
.venv/bin/pip install -r examples/e2e_pytorch_cifar10/requirements.txt
cd examples/e2e_pytorch_cifar10
./run_demo.sh fedavg 5 15                                   # IID
./run_demo.sh fedavg 5 15 --dirichlet --dirichlet-alpha 0.1 # non-IID
./run_demo.sh krum 5 15 --poison --no-reputation            # a robust aggregator vs one attacker
```

`run_demo.sh [AGGREGATOR] [N_CLIENTS] [ROUNDS]` builds the Rust binaries,
downloads and partitions a 2,000-image CIFAR-10 subsample (first run
fetches ~170 MB, cached after), prints a centralized baseline as the
target, then runs a real server, N nodes, N trainers, and one eval client
that prints `round=N held_out_accuracy=…` each round.

The model is deliberately small so the demo finishes in minutes on a CPU —
it beats random guessing (0.10) clearly and separates aggregation methods,
but is not a competitive CIFAR-10 result. For a walkthrough that
reproduces FedAvg here and explains the gap to the paper's numbers, see
the reproduction tutorial on the documentation site:
https://confluxfl.dev/tutorial-reproduce-fedavg/
