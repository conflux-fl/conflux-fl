#!/usr/bin/env python3
"""Trains the same character-level GRU on the pooled (non-federated)
Shakespeare text, with the same total gradient-step budget the federated
run uses — the correctness bar the federated run is compared against."""

import argparse
from pathlib import Path

import torch

from model import evaluate, new_model, set_vocab_size, train_steps


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

    # Same vocabulary handshake every other process in this harness
    # performs — see trainer_client.py's `load_vocab_size`.
    vocab_path = Path(args.pooled).resolve().parent / "vocab.pt"
    set_vocab_size(len(torch.load(vocab_path, weights_only=False)["vocab"]))

    pooled = torch.load(args.pooled, weights_only=False)
    X, y = pooled["X"], pooled["y"]
    held_out = torch.load(args.held_out, weights_only=False)
    X_held, y_held = held_out["X"], held_out["y"]

    model = new_model()
    train_steps(model, X, y, args.lr, args.total_steps)
    acc, _ = evaluate(model, X_held, y_held)

    print(f"centralized baseline: {args.total_steps} mini-batch SGD steps on {len(X)} pooled samples")
    print(f"held_out_accuracy={acc:.4f}")


if __name__ == "__main__":
    main()
