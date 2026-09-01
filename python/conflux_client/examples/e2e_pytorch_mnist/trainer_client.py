#!/usr/bin/env python3
"""Real-training test-harness client (docs/E2E_TESTING.md, Option B).

Rewritten onto the `ClientApp` SDK (ADR 0005 question 3). Everything this
file used to carry — its own f32 codec, register, the round-polling loop,
chunking, submit-with-retry — now lives in `conflux_client.app`. What is
left is the part that is actually about MNIST.

It also now reports `local_steps` and `local_loss`, which no client could
before: those wire fields existed and nothing populated them, which is
why FedNova and q-FedAvg were shipped-but-inert.
"""

import argparse
import sys
from pathlib import Path

import torch

sys.path.insert(0, str(Path(__file__).resolve().parents[2]))
sys.path.insert(0, str(Path(__file__).resolve().parent))

from app import ClientApp, TrainResult, is_placeholder_init, main  # noqa: E402
from model import new_model, train_steps, unflatten  # noqa: E402


class MnistClient(ClientApp):
    """A real PyTorch MLP on a real MNIST shard."""

    def __init__(self, shard_path, lr, steps, poison=False, poison_magnitude=20.0, mu=0.0):
        shard = torch.load(shard_path)
        self.X, self.y = shard["X"], shard["y"]
        self.lr, self.steps = lr, steps
        self.poison, self.poison_magnitude = poison, poison_magnitude
        # FedProx's proximal coefficient. 0.0 is plain FedAvg local
        # training, which is what the paper's own mu = 0 reduces to.
        self.mu = mu
        self.model = new_model()
        print(f"loaded {shard_path}: {len(self.X)} samples", flush=True)
        if mu > 0:
            print(f"FedProx: proximal term active, mu={mu}", flush=True)
        if poison:
            print("POISONED — every round submits offset weights instead of training", flush=True)

    def train(self, weights, round):
        if not is_placeholder_init(weights):
            unflatten(self.model, weights)
        # else: the server's generic all-zero placeholder. Keep this
        # client's own architecture-aware init — every client agrees,
        # because new_model() is deterministic.

        if self.poison:
            return TrainResult(
                weights=[w + self.poison_magnitude for w in weights],
                num_samples=len(self.y),
            )

        # The loss *before* training, at the round's starting weights —
        # which is what q-FedAvg's F_k(w^t) means. Computed under
        # no_grad so it costs a forward pass and nothing else.
        with torch.no_grad():
            loss_before = torch.nn.functional.cross_entropy(
                self.model(self.X), self.y
            ).item()

        trained = train_steps(self.model, self.X, self.y, self.lr, self.steps, mu=self.mu)
        return TrainResult(
            weights=trained,
            num_samples=len(self.y),
            local_steps=self.steps,   # FedNova
            local_loss=loss_before,   # q-FedAvg
        )


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--address", default="127.0.0.1:47100")
    parser.add_argument("--client-id", default="trainer-1")
    parser.add_argument("--shard", required=True)
    parser.add_argument("--rounds", type=int, default=5)
    parser.add_argument("--lr", type=float, default=0.1)
    parser.add_argument("--steps", type=int, default=30)
    parser.add_argument("--poison", action="store_true")
    parser.add_argument("--poison-magnitude", type=float, default=20.0)
    parser.add_argument(
        "--mu",
        type=float,
        default=0.0,
        help="FedProx proximal coefficient. 0.0 (default) is plain FedAvg local "
        "training; the paper sweeps {0.001, 0.01, 0.1, 1.0}. Entirely client-side — "
        "the server neither knows nor needs to.",
    )
    args = parser.parse_args()

    app = MnistClient(
        args.shard, args.lr, args.steps, args.poison, args.poison_magnitude, args.mu
    )
    sys.argv = [
        sys.argv[0],
        "--address", args.address,
        "--client-id", args.client_id,
        "--rounds", str(args.rounds),
    ]
    main(app)
