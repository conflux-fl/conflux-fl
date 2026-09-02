#!/usr/bin/env python3
"""Real-training test-harness client (docs/E2E_TESTING.md, Option A).

Rewritten onto the `ClientApp` SDK: the connect/register/poll/chunk/
submit loop this file used to hand-roll — including its own copy of the
f32 codec — now lives in `conflux_client.app`, where its bugs get fixed
once. What is left is the part that is actually about logistic
regression on a shard.

It also now reports `local_steps` and `local_loss`, so this harness can
drive FedNova and q-FedAvg — before the migration it silently could
not, whatever the server was configured to run.

No `--trainer-seed` here, deliberately: training is full-batch gradient
descent, so there is no sampling stochasticity to seed. The PyTorch
harnesses have the flag; this one documents why it doesn't.
"""

import argparse
import sys
from pathlib import Path

import numpy as np

sys.path.insert(0, str(Path(__file__).resolve().parents[2]))
sys.path.insert(0, str(Path(__file__).resolve().parent))

from app import ClientApp, TrainResult, main  # noqa: E402
from model import loss, train_steps  # noqa: E402


class LogRegClient(ClientApp):
    """Logistic regression on this client's own shard — never the full
    dataset, the same discipline a real federated client has to follow."""

    def __init__(self, shard_path, lr, steps, poison=False, poison_magnitude=1000.0):
        shard = np.load(shard_path)
        self.X, self.y = shard["X"], shard["y"]
        self.lr, self.steps = lr, steps
        self.poison, self.poison_magnitude = poison, poison_magnitude
        print(
            f"loaded {shard_path}: {len(self.X)} samples, class balance {self.y.mean():.2f}",
            flush=True,
        )
        if poison:
            print(
                "POISONED — every round submits offset weights instead of training",
                flush=True,
            )

    def train(self, weights, round):
        w = np.asarray(weights, dtype=np.float32)

        if self.poison:
            # A persistent Byzantine client, every round — what actually
            # stresses a robust aggregator across rounds, unlike a
            # single-shot offset.
            return TrainResult(
                weights=(w + self.poison_magnitude).tolist(),
                num_samples=len(self.y),
            )

        # The loss at the round's *starting* weights — q-FedAvg's
        # F_k(w^t), measured before any local step.
        loss_before = float(loss(w, self.X, self.y))
        trained = train_steps(w, self.X, self.y, self.lr, self.steps)
        return TrainResult(
            weights=trained.tolist(),
            num_samples=len(self.y),
            local_steps=self.steps,  # FedNova
            local_loss=loss_before,  # q-FedAvg
        )


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--address", default="127.0.0.1:47100")
    parser.add_argument("--client-id", required=True)
    parser.add_argument("--shard", required=True)
    parser.add_argument("--rounds", type=int, default=20)
    parser.add_argument("--lr", type=float, default=0.5)
    parser.add_argument("--steps", type=int, default=5)
    parser.add_argument(
        "--poison",
        action="store_true",
        help="submit offset weights every round instead of training — a "
        "persistent Byzantine client",
    )
    parser.add_argument("--poison-magnitude", type=float, default=1000.0)
    args = parser.parse_args()

    app = LogRegClient(args.shard, args.lr, args.steps, args.poison, args.poison_magnitude)
    sys.argv = [
        sys.argv[0],
        "--address", args.address,
        "--client-id", args.client_id,
        "--rounds", str(args.rounds),
    ]
    main(app)
