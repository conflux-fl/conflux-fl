#!/usr/bin/env python3
"""Real-training test-harness client (the PyTorch MNIST harness).

Built on the `ClientApp` SDK: the f32 codec, registration, the
round-polling loop, chunking, and submit-with-retry all live in
`conflux_client.app`. What is here is the part that is actually about
MNIST.

It also reports `local_steps` and `local_loss`, which no client could
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
from model import new_model, train_steps, unflatten, unflatten_like  # noqa: E402


class MnistClient(ClientApp):
    """A real PyTorch MLP on a real MNIST shard."""

    def __init__(
        self, shard_path, lr, steps, poison=False, poison_magnitude=20.0, mu=0.0, scaffold=False
    ):
        shard = torch.load(shard_path)
        self.X, self.y = shard["X"], shard["y"]
        self.lr, self.steps = lr, steps
        self.poison, self.poison_magnitude = poison, poison_magnitude
        # FedProx's proximal coefficient. 0.0 is plain FedAvg local
        # training, which is what the paper's own mu = 0 reduces to.
        self.mu = mu
        # SCAFFOLD's client half (Karimireddy et al. 2020, Algorithm 1,
        # option II). Two pieces of state, both flat vectors the size of
        # the model:
        #   c_i — THIS client's control variate, persisted across rounds,
        #         initialized to zero (the paper's own initialization).
        #   c   — the SERVER's global control variate, delivered before
        #         each round via `on_control_variate`; zeros until the
        #         server has aggregated at least one round of deltas.
        self.scaffold = scaffold
        self.c_i: list[float] | None = None
        self.c: list[float] | None = None
        self.model = new_model()
        print(f"loaded {shard_path}: {len(self.X)} samples", flush=True)
        if mu > 0:
            print(f"FedProx: proximal term active, mu={mu}", flush=True)
        if poison:
            print("POISONED — every round submits offset weights instead of training", flush=True)
        if scaffold:
            print("SCAFFOLD: client-side control variate active", flush=True)

    def on_control_variate(self, c):
        self.c = list(c)
        # Say so, out loud — once. A SCAFFOLD run where c never arrives
        # is indistinguishable from a correct one by accuracy alone (the
        # correction just silently becomes -c_i, which *increases*
        # variance), so the first nonzero delivery is worth a line.
        if not getattr(self, "_c_announced", False) and any(v != 0.0 for v in c):
            norm = sum(v * v for v in c) ** 0.5
            print(f"SCAFFOLD: first nonzero c received (l2 norm {norm:.4f})", flush=True)
            self._c_announced = True

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

        if not self.scaffold:
            trained = train_steps(self.model, self.X, self.y, self.lr, self.steps, mu=self.mu)
            return TrainResult(
                weights=trained,
                num_samples=len(self.y),
                local_steps=self.steps,   # FedNova
                local_loss=loss_before,   # q-FedAvg
            )

        # --- SCAFFOLD ------------------------------------------------
        dim = len(weights)
        if self.c_i is None:
            self.c_i = [0.0] * dim
        c = self.c if self.c is not None else [0.0] * dim

        # Each local step follows the corrected gradient g - c_i + c, so
        # the per-parameter correction handed to the optimizer is
        # (c - c_i). With both at their zero initialization this is
        # exactly plain FedAvg local training, which is the paper's own
        # round-one behavior — not a special case bolted on here.
        correction_flat = [cv - ci for cv, ci in zip(c, self.c_i)]
        trained = train_steps(
            self.model,
            self.X,
            self.y,
            self.lr,
            self.steps,
            correction=unflatten_like(self.model, correction_flat),
        )

        # Option II's control-variate update, computed from what this
        # round actually did rather than from a second gradient pass:
        #   c_i+ = c_i - c + (x - y) / (K * lr)
        # so the *delta* this client reports is
        #   dc_i = c_i+ - c_i = (x - y) / (K * lr) - c.
        # The server folds it in damped by 1/N (its
        # `scaffold_num_clients`); c_i is updated locally to c_i+ so next
        # round's correction uses this round's evidence.
        scale = 1.0 / (self.steps * self.lr)
        delta_c = [(x - yy) * scale - cv for x, yy, cv in zip(weights, trained, c)]
        self.c_i = [ci + d for ci, d in zip(self.c_i, delta_c)]

        return TrainResult(
            weights=trained,
            num_samples=len(self.y),
            local_steps=self.steps,      # FedNova
            local_loss=loss_before,      # q-FedAvg
            control_variate=delta_c,     # SCAFFOLD
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
        "--scaffold",
        action="store_true",
        help="Maintain a SCAFFOLD control variate: correct local steps by (c - c_i) "
        "and report delta-c_i on the wire. Pair with aggregator = \"scaffold\" and "
        "CONFLUX_SCAFFOLD_NUM_CLIENTS on the server.",
    )
    parser.add_argument(
        "--trainer-seed",
        type=int,
        default=None,
        help="Reseed torch's RNG after model init, so batch sampling varies "
        "across sweep seeds. Unset keeps the legacy fully-deterministic run.",
    )
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
        args.shard,
        args.lr,
        args.steps,
        args.poison,
        args.poison_magnitude,
        args.mu,
        scaffold=args.scaffold,
    )
    if args.trainer_seed is not None:
        # After new_model()'s manual_seed(0): the shared init every
        # client needs survives; the sampling trajectory becomes this
        # run's own. This is r4's second half — without it, a
        # multi-seed sweep varies only the partition and replays one
        # SGD trajectory per shard.
        torch.manual_seed(args.trainer_seed)
        print(f"trainer RNG reseeded: {args.trainer_seed}", flush=True)
    sys.argv = [
        sys.argv[0],
        "--address", args.address,
        "--client-id", args.client_id,
        "--rounds", str(args.rounds),
    ]
    main(app)
