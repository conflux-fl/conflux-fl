#!/usr/bin/env python3
"""Real-training test-harness client for the Shakespeare harness.

Byte-identical in structure to the MNIST harness's trainer_client.py —
which is the point: `model.py` is the only file that knows what the task
is, so a new dataset costs a model and a partitioner, not a new client.
The one addition here is the vocabulary handshake below, which a
character-level model needs and an image model doesn't.
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
from model import is_placeholder_init, new_model, set_vocab_size, train_steps, unflatten


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


def encode_weights(weights) -> bytes:
    return struct.pack(f"<{len(weights)}f", *[float(w) for w in weights])


def run(
    address: str,
    client_id: str,
    shard_path: str,
    rounds: int,
    lr: float,
    steps: int,
    poison: bool = False,
    poison_magnitude: float = 20.0,
) -> None:
    load_vocab_size(shard_path)
    shard = torch.load(shard_path, weights_only=False)
    X, y = shard["X"], shard["y"]
    print(f"[{client_id}] loaded {shard_path}: {len(X)} samples")
    if poison:
        print(f"[{client_id}] POISONED — every round submits offset weights instead of training")

    model = new_model()

    channel = grpc.insecure_channel(address)
    stub = pb2_grpc.FlTransportStub(channel)
    stub.Register(pb2.RegisterRequest(client_id=client_id, auth_token="harness-token"))
    print(f"[{client_id}] registered")

    last_round = None
    completed = 0
    while completed < rounds:
        while True:
            task = stub.FetchTask(pb2.FetchTaskRequest(client_id=client_id))
            if task.round != last_round:
                break
            time.sleep(0.2)

        weights = decode_weights(task.model_weights)
        if not is_placeholder_init(weights):
            unflatten(model, weights)
        # else: Conflux's generic zero placeholder — keep this client's
        # own real init instead (every client agrees, since new_model()
        # is deterministic). See model.py's is_placeholder_init.
        if poison:
            trained = [w + poison_magnitude for w in weights]
        else:
            trained = train_steps(model, X, y, lr, steps)

        def chunks():
            yield pb2.DeltaChunk(
                client_id=client_id,
                round=task.round,
                chunk_index=0,
                total_chunks=1,
                data=encode_weights(trained),
                num_samples=len(y),
            )

        try:
            ack = stub.SubmitDelta(chunks())
        except grpc.RpcError as e:
            print(f"[{client_id}] round {task.round} submission rejected ({e.code()}); retrying")
            last_round = task.round
            continue

        last_round = task.round
        completed += 1
        print(f"[{client_id}] round {task.round}: submitted, accepted={ack.accepted}")

    print(f"[{client_id}] done — completed {completed} rounds")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--address", default="127.0.0.1:47100")
    parser.add_argument("--client-id", required=True)
    parser.add_argument("--shard", required=True)
    parser.add_argument("--rounds", type=int, default=20)
    parser.add_argument("--lr", type=float, default=0.1)
    parser.add_argument("--steps", type=int, default=10)
    parser.add_argument("--poison", action="store_true")
    parser.add_argument("--poison-magnitude", type=float, default=20.0)
    args = parser.parse_args()

    try:
        run(
            args.address,
            args.client_id,
            args.shard,
            args.rounds,
            args.lr,
            args.steps,
            args.poison,
            args.poison_magnitude,
        )
    except grpc.RpcError as e:
        print(f"[{args.client_id}] RPC failed: {e}", file=sys.stderr)
        sys.exit(1)
