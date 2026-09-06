#!/usr/bin/env python3
"""One manifest-driven evaluator, for every recipe.

Registers as an ordinary client and fetches the global checkpoint each
round without ever submitting one, so the number it prints is the
server's model as clients actually see it — not a local reconstruction
of what it should be.

Prints `held_out_accuracy=<value>` per round, which is the token
`conflux-baselines` reads to decide whether a paper reproduced.
"""

from __future__ import annotations

import argparse
import statistics
import struct
import time

import torch

from . import _sdk, models, torch_model

_sdk.install()

import grpc  # noqa: E402
import fl_transport_pb2 as pb2  # noqa: E402
import fl_transport_pb2_grpc as pb2_grpc  # noqa: E402


def decode_weights(data: bytes) -> list[float]:
    return list(struct.unpack(f"<{len(data) // 4}f", data))


def run(
    model_name: str,
    address: str,
    held_out_path: str,
    target_rounds: int,
    timeout_s: float,
    shard_paths: list[str] | None = None,
) -> None:
    held = torch.load(held_out_path)
    X, y = held["X"], held["y"]
    model = models.build(model_name)

    # The fairness axis. Global held-out accuracy is a *mean* over one
    # pooled distribution, and a mean cannot see who it is failing:
    # q-FedAvg's entire claim is about the per-client accuracy
    # *distribution*, possibly at some cost to the mean, so without this
    # the method's headline number is literally unmeasurable. Each shard
    # stands in for its client's local distribution; both arms of any
    # comparison are measured identically, which is what makes the
    # min/std comparable even though shards are training data.
    shards = []
    for path in shard_paths or []:
        s = torch.load(path)
        shards.append((s["X"], s["y"]))

    channel = grpc.insecure_channel(address)
    stub = pb2_grpc.FlTransportStub(channel)
    stub.Register(pb2.RegisterRequest(client_id="evaluator", auth_token="client-token"))

    seen: set[int] = set()
    deadline = time.time() + timeout_s
    # An evaluator polls, so it sees *some* of the rounds that happen,
    # never all of them — waiting to observe `target_rounds` distinct
    # ones would outlast the training it is watching. What it can see is
    # the round number going still, which is what training finishing
    # looks like from here.
    stalled = 0
    stall_limit = 40  # polls, at half a second each
    while len(seen) < target_rounds and time.time() < deadline and stalled < stall_limit:
        task = stub.FetchTask(pb2.FetchTaskRequest(client_id="evaluator"))
        if task.round in seen:
            stalled += 1
            time.sleep(0.5)
            continue
        stalled = 0
        seen.add(task.round)

        weights = decode_weights(task.model_weights)
        # The same placeholder substitution every trainer makes: scoring
        # the server's all-zero checkpoint would report an accuracy about
        # the placeholder rather than about the model, and round one's
        # real number would read as "0.10, broken" instead of
        # "untrained, but a real initialization".
        if not torch_model.is_placeholder_init(weights):
            torch_model.unflatten(model, weights)
        acc, loss = torch_model.evaluate(model, X, y)

        line = f"round={task.round} held_out_accuracy={acc:.4f} held_out_loss={loss:.4f}"
        if shards:
            per_client = [torch_model.evaluate(model, sx, sy)[0] for sx, sy in shards]
            line += (
                f" client_acc_min={min(per_client):.4f}"
                f" client_acc_std={statistics.pstdev(per_client):.4f}"
                f" client_accs={','.join(f'{a:.4f}' for a in per_client)}"
            )
        print(line, flush=True)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--model", required=True)
    parser.add_argument("--address", default="127.0.0.1:47100")
    parser.add_argument("--held-out", required=True)
    parser.add_argument("--rounds", type=int, default=5)
    parser.add_argument("--timeout", type=float, default=600.0)
    parser.add_argument("--shards", nargs="*", default=None)
    args = parser.parse_args()
    run(
        args.model,
        args.address,
        args.held_out,
        args.rounds,
        args.timeout,
        args.shards,
    )
