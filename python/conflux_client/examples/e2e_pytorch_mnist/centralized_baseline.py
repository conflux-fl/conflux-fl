#!/usr/bin/env python3
"""Trains the same MLP on the pooled (non-federated) MNIST subsample,
with the same total gradient-step budget the federated run uses — the
correctness bar Option B's federated run is compared against."""

import argparse

import torch

from model import evaluate, new_model, train_steps


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--pooled", default="pooled.pt")
    parser.add_argument("--held-out", default="held_out.pt")
    parser.add_argument("--lr", type=float, default=0.1)
    parser.add_argument(
        "--total-steps",
        type=int,
        default=150,
        help="rounds * steps-per-round, to match the federated run's total gradient steps",
    )
    args = parser.parse_args()

    pooled = torch.load(args.pooled)
    X, y = pooled["X"], pooled["y"]
    held_out = torch.load(args.held_out)
    X_held, y_held = held_out["X"], held_out["y"]

    model = new_model()
    train_steps(model, X, y, args.lr, args.total_steps)
    acc, _ = evaluate(model, X_held, y_held)

    print(f"centralized baseline: {args.total_steps} mini-batch SGD steps on {len(X)} pooled samples")
    print(f"held_out_accuracy={acc:.4f}")


if __name__ == "__main__":
    main()
