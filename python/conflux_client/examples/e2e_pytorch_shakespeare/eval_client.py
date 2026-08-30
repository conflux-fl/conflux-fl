#!/usr/bin/env python3
"""Eval-only client (docs/E2E_TESTING.md, Option B) — same role as
Option A's eval_client.py, scoring the current global checkpoint
against held-out Shakespeare from speaking roles no client trained on.
"""

import argparse
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
from model import evaluate, is_placeholder_init, new_model, set_vocab_size, unflatten


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



def decode_weights(data: bytes) -> list[float]:
    count = len(data) // 4
    return list(struct.unpack(f"<{count}f", data))


def run(address: str, held_out_path: str, target_rounds: int, timeout_s: float) -> None:
    load_vocab_size(held_out_path)
    held_out = torch.load(held_out_path, weights_only=False)
    X, y = held_out["X"], held_out["y"]
    model = new_model()

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
            print(f"round={task.round} held_out_accuracy={acc:.4f} held_out_loss={ce:.4f}")
        time.sleep(0.5)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--address", default="127.0.0.1:47100")
    parser.add_argument("--held-out", required=True)
    parser.add_argument("--rounds", type=int, default=20)
    parser.add_argument("--timeout", type=float, default=180.0)
    args = parser.parse_args()

    try:
        run(args.address, args.held_out, args.rounds, args.timeout)
    except grpc.RpcError as e:
        print(f"[eval-client] RPC failed: {e}", file=sys.stderr)
        sys.exit(1)
