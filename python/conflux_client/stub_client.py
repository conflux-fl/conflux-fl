#!/usr/bin/env python3
"""Stub Python ClientApp — fixed dummy weights, no PyTorch dependency.

Stands in for a real training client so the local gRPC handoff between
conflux-node and a Python ClientApp can be exercised end-to-end, across the language boundary, not just within
Rust. Permitted only in research mode (`allow_stub_client`).

Connects to conflux-node's local gRPC server (loopback, no TLS),
registers, fetches one task, "trains" by adding a fixed offset to every
weight, and submits the result back.

Requires the generated stubs from generate_proto.sh (not committed — see
that script's comment).

`--poison` turns this into a Byzantine test client instead — submits a
large-magnitude offset instead of the honest one, standing in for an
adversarial ClientApp so the `robust` aggregation family can be
exercised end-to-end, over the real network hop, not just against
synthetic `ClientDelta`s in Rust unit tests. This is a test fixture, not
the `ClientApp` SDK (`conflux_client.app`).
"""

import argparse
import struct
import sys

import grpc

import fl_transport_pb2 as pb2
import fl_transport_pb2_grpc as pb2_grpc

DUMMY_TRAINING_OFFSET = 1.0
DUMMY_NUM_SAMPLES = 100


def decode_weights(data: bytes) -> list[float]:
    count = len(data) // 4
    return list(struct.unpack(f"<{count}f", data))


def encode_weights(weights: list[float]) -> bytes:
    return struct.pack(f"<{len(weights)}f", *weights)


def run(
    address: str,
    client_id: str,
    poison: bool = False,
    poison_magnitude: float = 1000.0,
) -> None:
    channel = grpc.insecure_channel(address)
    stub = pb2_grpc.FlTransportStub(channel)

    register_response = stub.Register(
        pb2.RegisterRequest(client_id=client_id, auth_token="stub-token")
    )
    print(f"[stub_client] register: accepted={register_response.accepted}")

    task = stub.FetchTask(pb2.FetchTaskRequest(client_id=client_id))
    print(f"[stub_client] fetched task_id={task.task_id} round={task.round}")

    weights = decode_weights(task.model_weights)
    if poison:
        trained = [w + poison_magnitude for w in weights]
        print(
            f"[stub_client] POISONED (magnitude={poison_magnitude}, no PyTorch): "
            f"{weights} -> {trained}"
        )
    else:
        trained = [w + DUMMY_TRAINING_OFFSET for w in weights]
        print(
            f"[stub_client] trained (fixed offset, no PyTorch): "
            f"{weights} -> {trained}"
        )

    def chunks():
        yield pb2.DeltaChunk(
            client_id=client_id,
            round=task.round,
            chunk_index=0,
            total_chunks=1,
            data=encode_weights(trained),
            num_samples=DUMMY_NUM_SAMPLES,
        )

    ack = stub.SubmitDelta(chunks())
    print(
        f"[stub_client] submit_delta: accepted={ack.accepted} "
        f"message={ack.message!r}"
    )


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--address",
        default="127.0.0.1:47100",
        help="conflux-node's local gRPC address",
    )
    parser.add_argument("--client-id", default="stub-client-py")
    parser.add_argument(
        "--poison",
        action="store_true",
        help=(
            "submit adversarial weights instead of honest training, for "
            "testing the robust aggregation family end to end"
        ),
    )
    parser.add_argument(
        "--poison-magnitude",
        type=float,
        default=1000.0,
        help="offset added to every weight when --poison is set",
    )
    args = parser.parse_args()

    try:
        run(args.address, args.client_id, args.poison, args.poison_magnitude)
    except grpc.RpcError as e:
        print(f"[stub_client] RPC failed: {e}", file=sys.stderr)
        sys.exit(1)
