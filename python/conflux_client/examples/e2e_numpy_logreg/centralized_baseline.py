#!/usr/bin/env python3
"""Trains the same logistic regression on the *pooled* (non-federated)
training data, with the same hyperparameters each federated round uses —
the correctness bar the federated run is compared against. If Conflux's
orchestration is working, the federated run's accuracy should land close
to this, not just "some accuracy."
"""

import argparse

import numpy as np

from model import accuracy, train_steps


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--pooled", default="pooled.npz")
    parser.add_argument("--held-out", default="held_out.npz")
    parser.add_argument("--lr", type=float, default=0.5)
    parser.add_argument(
        "--total-steps",
        type=int,
        default=100,
        help="rounds * steps-per-round, to match the federated run's total gradient steps",
    )
    args = parser.parse_args()

    pooled = np.load(args.pooled)
    X, y = pooled["X"], pooled["y"]
    held_out = np.load(args.held_out)
    X_held, y_held = held_out["X"], held_out["y"]

    dim = X.shape[1] + 1
    weights = np.zeros(dim, dtype=np.float32)
    weights = train_steps(weights, X, y, args.lr, args.total_steps)

    print(f"centralized baseline: {args.total_steps} full-batch GD steps on {len(X)} pooled samples")
    print(f"held_out_accuracy={accuracy(weights, X_held, y_held):.4f}")


if __name__ == "__main__":
    main()
