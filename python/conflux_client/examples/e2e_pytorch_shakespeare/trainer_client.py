#!/usr/bin/env python3
"""Real-training Shakespeare next-character client (the Shakespeare harness).

Rewritten onto the `ClientApp` SDK — the hand-rolled loop and its own
f32-codec copy are gone; what remains is the GRU-on-a-shard part, plus
the one thing genuinely unique to this harness: pinning the vocabulary
size before any model exists. It now reports `local_steps` and
`local_loss`, so this harness can drive FedNova and q-FedAvg, which the
pre-migration copy silently could not.

`--trainer-seed` reseeds torch's global RNG *after* the deterministic
model init (which every client must share) — see the CIFAR-10 trainer
for the reasoning; it is the same flag with the same contract.
"""

import argparse
import sys
from pathlib import Path

import torch

sys.path.insert(0, str(Path(__file__).resolve().parents[2]))
sys.path.insert(0, str(Path(__file__).resolve().parent))

from app import ClientApp, TrainResult, is_placeholder_init, main  # noqa: E402
from model import new_model, set_vocab_size, train_steps, unflatten  # noqa: E402


def load_vocab_size(shard_path: str) -> int:
    """Reads `vocab.pt` from beside the shards and pins the model's
    output dimension to it.

    Explicit rather than automatic: every process in this harness must
    build the *same* architecture, and a silently-defaulted vocabulary
    would produce models that differ only in their final layer's width —
    which shows up as a confusing length mismatch at the aggregator
    rather than an obvious error here.
    """
    vocab_path = Path(shard_path).resolve().parent / "vocab.pt"
    vocab = torch.load(vocab_path, weights_only=False)["vocab"]
    set_vocab_size(len(vocab))
    return len(vocab)


class ShakespeareClient(ClientApp):
    """A character-level GRU on one speaker-partitioned shard."""

    def __init__(self, shard_path, lr, steps, poison=False, poison_magnitude=20.0):
        vocab_size = load_vocab_size(shard_path)
        shard = torch.load(shard_path)
        self.X, self.y = shard["X"], shard["y"]
        self.lr, self.steps = lr, steps
        self.poison, self.poison_magnitude = poison, poison_magnitude
        self.model = new_model()
        print(
            f"loaded {shard_path}: {len(self.X)} sequences, vocab {vocab_size}",
            flush=True,
        )
        if poison:
            print("POISONED — every round submits offset weights instead of training", flush=True)

    def train(self, weights, round):
        if not is_placeholder_init(weights):
            unflatten(self.model, weights)

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
    parser.add_argument("--lr", type=float, default=0.5)
    parser.add_argument("--steps", type=int, default=20)
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

    app = ShakespeareClient(args.shard, args.lr, args.steps, args.poison, args.poison_magnitude)
    if args.trainer_seed is not None:
        torch.manual_seed(args.trainer_seed)
        print(f"trainer RNG reseeded: {args.trainer_seed}", flush=True)
    sys.argv = [
        sys.argv[0],
        "--address", args.address,
        "--client-id", args.client_id,
        "--rounds", str(args.rounds),
    ]
    main(app)
