#!/usr/bin/env python3
"""Trains the recipe's model on the pooled (non-federated) data.

The number this prints is the bar a federated run is measured against:
the same model, the same total gradient-step budget, but every sample in
one place. Federated training is not supposed to beat it — the question
is how much it gives up, and an aggregator that collapses under attack
shows up as a widening gap rather than as an absolute number nobody can
calibrate.

`prepare` already writes `pooled.pt` for exactly this, so the baseline
costs one more process rather than a second copy of the data.
"""

from __future__ import annotations

import argparse

import torch

from . import models, torch_model


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--model", required=True)
    parser.add_argument("--pooled", default="pooled.pt")
    parser.add_argument("--held-out", default="held_out.pt")
    parser.add_argument("--lr", type=float, default=0.1)
    parser.add_argument(
        "--total-steps",
        type=int,
        default=150,
        help="rounds * steps-per-round, so the comparison is at equal gradient steps",
    )
    args = parser.parse_args()

    pooled = torch.load(args.pooled)
    held_out = torch.load(args.held_out)

    model = models.build(args.model)
    torch_model.train_steps(model, pooled["X"], pooled["y"], args.lr, args.total_steps)
    accuracy, _ = torch_model.evaluate(model, held_out["X"], held_out["y"])

    print(
        f"centralized baseline: {args.total_steps} mini-batch SGD steps "
        f"on {len(pooled['X'])} pooled samples"
    )
    print(f"held_out_accuracy={accuracy:.4f}")


if __name__ == "__main__":
    main()
