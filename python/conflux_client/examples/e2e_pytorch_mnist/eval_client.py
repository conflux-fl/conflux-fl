#!/usr/bin/env python3
"""Eval-only client (docs/E2E_TESTING.md, Option B) — same role as
Option A's eval_client.py, scoring the current global checkpoint
against real MNIST test images instead of the synthetic held-out set.
"""

import argparse
import statistics
import struct
import sys
import time
from pathlib import Path

import grpc
import torch

sys.path.insert(0, str(Path(__file__).resolve().parents[2]))
sys.path.insert(0, str(Path(__file__).resolve().parent))

import fl_transport_pb2 as pb2
import fl_transport_pb2_grpc as pb2_grpc
from model import evaluate, is_placeholder_init, new_model, unflatten


def decode_weights(data: bytes) -> list[float]:
    count = len(data) // 4
    return list(struct.unpack(f"<{count}f", data))


def run(
    address: str,
    held_out_path: str,
    target_rounds: int,
    timeout_s: float,
    shard_paths: list[str] | None = None,
) -> None:
    held_out = torch.load(held_out_path)
    X, y = held_out["X"], held_out["y"]
    model = new_model()

    # The fairness axis. Global held-out accuracy is a *mean* over one
    # pooled distribution, and a mean cannot see who it is failing:
    # q-FedAvg's entire claim is about the per-client accuracy
    # *distribution* — flattening it, possibly at some cost to the mean —
    # so without this measurement the method's headline number is
    # literally unmeasurable. Each shard here stands in for its client's
    # local distribution; both arms of any comparison are measured
    # identically, which is what makes the min/std comparable even
    # though shards are training data.
    shards = []
    for sp in shard_paths or []:
        d = torch.load(sp)
        shards.append((Path(sp).stem, d["X"], d["y"]))

    channel = grpc.insecure_channel(address)
    stub = pb2_grpc.FlTransportStub(channel)
    stub.Register(pb2.RegisterRequest(client_id="eval-client", auth_token="harness-token"))

    seen_rounds: set[int] = set()
    deadline = time.time() + timeout_s

    while len(seen_rounds) < target_rounds and time.time() < deadline:
        task = stub.FetchTask(pb2.FetchTaskRequest(client_id="eval-client"))
        if task.round not in seen_rounds:
            weights = decode_weights(task.model_weights)
            # Same placeholder-init substitution as trainer_client.py —
            # otherwise round 1's real accuracy reads as "0.10, broken"
            # instead of what it actually is (an untrained-but-real init).
            if not is_placeholder_init(weights):
                unflatten(model, weights)
            acc, ce = evaluate(model, X, y)
            seen_rounds.add(task.round)
            line = f"round={task.round} held_out_accuracy={acc:.4f} held_out_loss={ce:.4f}"
            if shards:
                per = [evaluate(model, sx, sy)[0] for _, sx, sy in shards]
                line += (
                    f" client_acc_min={min(per):.4f}"
                    f" client_acc_std={statistics.pstdev(per):.4f}"
                    f" client_accs={','.join(f'{a:.4f}' for a in per)}"
                )
            print(line)
        time.sleep(0.5)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--address", default="127.0.0.1:47100")
    parser.add_argument("--held-out", required=True)
    parser.add_argument("--rounds", type=int, default=20)
    parser.add_argument("--timeout", type=float, default=180.0)
    parser.add_argument(
        "--shards",
        default="",
        help="Comma-separated shard .pt paths. When given, each round also reports "
        "the global model's accuracy on every client's own data distribution — "
        "min, std, and the full list — which is the axis q-FedAvg claims to improve.",
    )
    args = parser.parse_args()

    try:
        run(
            args.address,
            args.held_out,
            args.rounds,
            args.timeout,
            [s for s in args.shards.split(",") if s],
        )
    except grpc.RpcError as e:
        print(f"[eval-client] RPC failed: {e}", file=sys.stderr)
        sys.exit(1)
