#!/usr/bin/env python3
"""Real-training CIFAR-10 client (the PyTorch CIFAR-10 harness, Option B').

Rewritten onto the `ClientApp` SDK — the hand-rolled loop and its own
f32-codec copy are gone; what remains is the CNN-on-a-shard part. It
now reports `local_steps` and `local_loss`, so this harness can drive
FedNova and q-FedAvg, which the pre-migration copy silently could not.

`--trainer-seed` reseeds torch's global RNG *after* the deterministic
model init (which every client must share), so mini-batch sampling
varies across sweep seeds instead of replaying one trajectory — the
model init stays common, the SGD noise becomes real.
"""

import argparse
import sys
from pathlib import Path

import torch

sys.path.insert(0, str(Path(__file__).resolve().parents[2]))
sys.path.insert(0, str(Path(__file__).resolve().parent))

from app import ClientApp, TrainResult, is_placeholder_init, main  # noqa: E402
from model import new_model, train_steps, unflatten  # noqa: E402


class Cifar10Client(ClientApp):
    """A real small CNN on a real CIFAR-10 shard."""

    def __init__(self, shard_path, lr, steps, poison=False, poison_magnitude=20.0):
        shard = torch.load(shard_path)
        self.X, self.y = shard["X"], shard["y"]
        self.lr, self.steps = lr, steps
        self.poison, self.poison_magnitude = poison, poison_magnitude
        self.model = new_model()
        print(f"loaded {shard_path}: {len(self.X)} samples", flush=True)
        if poison:
            print("POISONED — every round submits offset weights instead of training", flush=True)

    def train(self, weights, round):
        if not is_placeholder_init(weights):
            unflatten(self.model, weights)
        # else: the server's generic zero placeholder — keep this
        # client's own deterministic init, which every client shares.

        if self.poison:
            return TrainResult(
                weights=[w + self.poison_magnitude for w in weights],
                num_samples=len(self.y),
            )

        with torch.no_grad():
            loss_before = torch.nn.functional.cross_entropy(
                self.model(self.X), self.y
            ).item()

        trained = train_steps(self.model, self.X, self.y, self.lr, self.steps)
        return TrainResult(
            weights=trained,
            num_samples=len(self.y),
            local_steps=self.steps,  # FedNova
            local_loss=loss_before,  # q-FedAvg
        )


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--address", default="127.0.0.1:47100")
    parser.add_argument("--client-id", default="trainer-1")
    parser.add_argument("--shard", required=True)
    parser.add_argument("--rounds", type=int, default=5)
    parser.add_argument("--lr", type=float, default=0.01)
    parser.add_argument("--steps", type=int, default=10)
    parser.add_argument("--poison", action="store_true")
    parser.add_argument("--poison-magnitude", type=float, default=20.0)
    parser.add_argument(
        "--trainer-seed",
        type=int,
        default=None,
        help="Reseed torch's RNG after model init, so batch sampling varies "
        "across sweep seeds. Unset keeps the legacy fully-deterministic run.",
    )
    args = parser.parse_args()

    app = Cifar10Client(args.shard, args.lr, args.steps, args.poison, args.poison_magnitude)
    if args.trainer_seed is not None:
        # After new_model()'s manual_seed(0): init stays shared, the
        # sampling trajectory becomes this run's own.
        torch.manual_seed(args.trainer_seed)
        print(f"trainer RNG reseeded: {args.trainer_seed}", flush=True)
    sys.argv = [
        sys.argv[0],
        "--address", args.address,
        "--client-id", args.client_id,
        "--rounds", str(args.rounds),
    ]
    main(app)
