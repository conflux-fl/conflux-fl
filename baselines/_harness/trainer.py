#!/usr/bin/env python3
"""One manifest-driven trainer, for every recipe.

Before this, each harness carried its own `trainer_client.py` differing
in the model it imported and almost nothing else. What is genuinely
per-recipe is the architecture and the shard, and both are arguments
here — so a new dataset or model needs no new client at all.

The behavior is the shipped MNIST trainer's, unchanged: `local_steps`
and `local_loss` on every submission (which is what lets FedNova and
q-FedAvg do anything other than fall back to plain averaging), and
SCAFFOLD's control variate when the server sends one down.
"""

from __future__ import annotations

import argparse

import torch

from . import _sdk, models, torch_model

_sdk.install()

from app import ClientApp, TrainResult, run  # noqa: E402


class HarnessClient(ClientApp):
    """A `ClientApp` whose only per-recipe knowledge is which model to
    build and which shard to read."""

    def __init__(
        self,
        model_name: str,
        shard_path: str,
        lr: float,
        steps: int,
        poison: bool = False,
        poison_magnitude: float = 20.0,
        mu: float = 0.0,
        scaffold: bool = False,
    ) -> None:
        shard = torch.load(shard_path)
        self.X, self.y = shard["X"], shard["y"]
        self.lr, self.steps = lr, steps
        self.poison, self.poison_magnitude = poison, poison_magnitude
        # FedProx's proximal coefficient. 0.0 is plain FedAvg local
        # training, which is what the paper's own mu = 0 reduces to.
        self.mu = mu
        # SCAFFOLD's client half (Karimireddy et al. 2020, Algorithm 1,
        # option II). Two flat vectors the size of the model:
        #   c_i — THIS client's control variate, persisted across rounds,
        #         initialized to zero (the paper's own initialization).
        #   c   — the SERVER's global control variate, delivered before
        #         each round; zeros until the server has aggregated one.
        self.scaffold = scaffold
        self.c_i: list[float] | None = None
        self.c: list[float] | None = None
        self.model = models.build(model_name)
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
        if not torch_model.is_placeholder_init(weights):
            torch_model.unflatten(self.model, weights)
        # else: the server's generic all-zero placeholder. Keep this
        # client's own architecture-aware init — every client agrees,
        # because the model builders are deterministic.

        if self.poison:
            return TrainResult(
                weights=[w + self.poison_magnitude for w in weights],
                num_samples=len(self.y),
            )

        # The loss *before* training, at the round's starting weights —
        # which is what q-FedAvg's F_k(w^t) means. Computed under
        # no_grad so it costs a forward pass and nothing else.
        with torch.no_grad():
            loss_before = torch.nn.functional.cross_entropy(self.model(self.X), self.y).item()

        if not self.scaffold:
            trained = torch_model.train_steps(
                self.model, self.X, self.y, self.lr, self.steps, mu=self.mu
            )
            return TrainResult(
                weights=trained,
                num_samples=len(self.y),
                local_steps=self.steps,  # FedNova
                local_loss=loss_before,  # q-FedAvg
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
        trained = torch_model.train_steps(
            self.model,
            self.X,
            self.y,
            self.lr,
            self.steps,
            correction=torch_model.unflatten_like(self.model, correction_flat),
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
            local_steps=self.steps,  # FedNova
            local_loss=loss_before,  # q-FedAvg
            control_variate=delta_c,  # SCAFFOLD
        )


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--model", required=True)
    parser.add_argument("--shard", required=True)
    parser.add_argument("--address", default="127.0.0.1:47100")
    parser.add_argument("--client-id", default="trainer-1")
    parser.add_argument("--rounds", type=int, default=5)
    parser.add_argument("--lr", type=float, default=0.1)
    parser.add_argument("--steps", type=int, default=30)
    parser.add_argument("--mu", type=float, default=0.0, help="FedProx proximal term")
    parser.add_argument("--scaffold", action="store_true")
    parser.add_argument("--poison", action="store_true")
    parser.add_argument("--poison-magnitude", type=float, default=20.0)
    parser.add_argument(
        "--trainer-seed",
        type=int,
        default=None,
        help="reseeds SGD sampling after the shared model init, so a "
        "multi-seed sweep varies real mini-batch noise rather than "
        "replaying one trajectory",
    )
    return parser


if __name__ == "__main__":
    args = build_parser().parse_args()
    app = HarnessClient(
        args.model,
        args.shard,
        args.lr,
        args.steps,
        poison=args.poison,
        poison_magnitude=args.poison_magnitude,
        mu=args.mu,
        scaffold=args.scaffold,
    )
    if args.trainer_seed is not None:
        # After the shared init above, so every client still starts from
        # the same weights and only the sampling differs.
        torch.manual_seed(args.trainer_seed)
    # `run` rather than the SDK's `main`, which would re-parse argv:
    # this module owns its own flags.
    run(app, address=args.address, client_id=args.client_id, rounds=args.rounds)
