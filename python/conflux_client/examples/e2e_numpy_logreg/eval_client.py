#!/usr/bin/env python3
"""Eval-only client (docs/E2E_TESTING.md) — registers and calls FetchTask
only, never trains or submits. Scores the current global checkpoint
against a held-out test set every round, so accuracy-over-rounds can be
observed without touching any trainer's data. This needs no Rust-side
changes: any registered client can call FetchTask at any time.
"""

import argparse
import struct
import sys
import time
from pathlib import Path

import grpc
import numpy as np

sys.path.insert(0, str(Path(__file__).resolve().parents[2]))
sys.path.insert(0, str(Path(__file__).resolve().parent))

import fl_transport_pb2 as pb2
import fl_transport_pb2_grpc as pb2_grpc
from model import accuracy, loss


def decode_weights(data: bytes) -> np.ndarray:
    count = len(data) // 4
    return np.array(struct.unpack(f"<{count}f", data), dtype=np.float32)


def run(address: str, held_out_path: str, target_rounds: int, timeout_s: float) -> list[float]:
    held_out = np.load(held_out_path)
    X, y = held_out["X"], held_out["y"]

    channel = grpc.insecure_channel(address)
    stub = pb2_grpc.FlTransportStub(channel)
    stub.Register(pb2.RegisterRequest(client_id="eval-client", auth_token="harness-token"))

    seen_rounds: set[int] = set()
    accuracies: list[float] = []
    deadline = time.time() + timeout_s

    while len(seen_rounds) < target_rounds and time.time() < deadline:
        task = stub.FetchTask(pb2.FetchTaskRequest(client_id="eval-client"))
        if task.round not in seen_rounds:
            weights = decode_weights(task.model_weights)
            acc = accuracy(weights, X, y)
            ce = loss(weights, X, y)
            accuracies.append(acc)
            seen_rounds.add(task.round)
            print(f"round={task.round} held_out_accuracy={acc:.4f} held_out_loss={ce:.4f}")
        time.sleep(0.5)

    return accuracies


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--address", default="127.0.0.1:47100")
    parser.add_argument("--held-out", required=True)
    parser.add_argument("--rounds", type=int, default=20, help="stop after observing this many distinct rounds")
    parser.add_argument("--timeout", type=float, default=120.0, help="give up after this many seconds regardless")
    args = parser.parse_args()

    try:
        run(args.address, args.held_out, args.rounds, args.timeout)
    except grpc.RpcError as e:
        print(f"[eval-client] RPC failed: {e}", file=sys.stderr)
        sys.exit(1)
