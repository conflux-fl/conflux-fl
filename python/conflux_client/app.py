"""The Conflux `ClientApp` SDK.

Everything a federated client does *except* training is identical across
deployments: connect to `conflux-node`'s local loopback hop, register,
wait for a round that isn't the one you already did, recognize the
server's placeholder initialization, chunk the result, submit it, and
survive a round closing while you were still working.

Before this module, all four end-to-end harnesses and the stub client
reimplemented that loop — including four separate copies of a
`struct.pack`/`unpack` codec. Subclassing [`ClientApp`] leaves you with
one method to write:

    class MyApp(ClientApp):
        def train(self, weights, round):
            ...
            return TrainResult(weights=new_weights, num_samples=len(y))

    if __name__ == "__main__":
        run(MyApp(), address="127.0.0.1:47100", client_id="node-1", rounds=10)

## What this deliberately does not solve

Three questions are separable here, and this module answers only the
third:

1. **How does a client learn what model to train?** Not here. You import
   your own model, exactly as the existing harnesses do. A natural
   extension is for the experiment config to carry a Python import
   path; nothing in this module presumes that, and nothing blocks it.
2. **How does a participant *get* this code?** Not here, and explicitly
   outside this repository's boundary. `cross_silo` deployments install
   it out of band, which is already how every existing harness works.
   `crowdsource` and `edge` need a distribution story that is a product
   decision; the Rust-native client (`crates/conflux-client`) is a
   serious alternative answer to exactly that question.
3. **What does the SDK wrap?** This module. It is pure technical design,
   answerable from this codebase alone, and it is what unblocks
   `cross_silo` without waiting on (1) or (2).

## Why the optional fields are here

`TrainResult` carries `local_steps`, `local_loss` and `control_variate`
alongside the weights. Those are the optional per-method wire fields;
without a client that sends them, FedNova, SCAFFOLD and q-FedAvg fall
back to plain averaging. This is the place that sends them. A subclass that returns a `local_loss`
makes `qfedavg` do something other than fall back to FedAvg.
"""

from __future__ import annotations

import struct
import sys
import time
from abc import ABC, abstractmethod
from dataclasses import dataclass
from typing import Iterator, Sequence

import grpc

import fl_transport_pb2 as pb2
import fl_transport_pb2_grpc as pb2_grpc

# One chunk per this many *bytes* of payload. `conflux-net` bounds a whole
# stream at `max_update_bytes` (256 MiB by default), not a single message,
# but gRPC's own per-message ceiling is 4 MiB — so a model past ~1M
# parameters has to be split or the send fails outright. 1 MiB leaves
# generous headroom under that.
DEFAULT_CHUNK_BYTES = 1 << 20


def decode_weights(data: bytes) -> list[float]:
    """Little-endian packed `f32` -> floats.

    The wire convention every `ClientDelta.weights` / `DeltaChunk.data` /
    `TaskResponse.model_weights` buffer uses. Defined once here rather
    than copied into each client, which is what used to happen.
    """
    if len(data) % 4 != 0:
        raise ValueError(
            f"{len(data)} bytes is not a whole number of f32s — this buffer "
            "did not come from Conflux's codec"
        )
    return list(struct.unpack(f"<{len(data) // 4}f", data))


def encode_weights(weights: Sequence[float]) -> bytes:
    """Floats -> little-endian packed `f32`. Inverse of `decode_weights`."""
    return struct.pack(f"<{len(weights)}f", *[float(w) for w in weights])


def is_placeholder_init(weights: Sequence[float]) -> bool:
    """True for the server's generic all-zero starting checkpoint.

    `conflux-server` is opaque to model architecture, so the
    only initialization it can offer a model it knows nothing about is
    zeros. That is harmless for a model with no hidden layers and a
    **textbook symmetry-breaking failure** for anything with ReLU hidden
    units: every unit computes an identical zero output with an identical
    zero gradient, so none ever differentiates from another and the
    network cannot learn from that start, however long you train it.

    A real client recognizes this and substitutes its own
    architecture-aware initialization. Make that initialization
    deterministic, so every client agrees on the same starting point.

    This lived in each harness's `model.py`; it is a property of the
    *protocol*, not of any one model, so it belongs here.
    """
    return len(weights) > 0 and all(w == 0.0 for w in weights)


@dataclass
class TrainResult:
    """What one round of local training produced."""

    #: The trained weights, flat, same length as the ones handed in.
    weights: Sequence[float]
    #: How many local examples this client trained on. FedAvg weights by
    #: it. Self-reported and unauthenticated — inflating it buys
    #: proportional influence, which is an assumption of the published
    #: method rather than a guarantee of the transport.
    num_samples: int
    #: How many local optimization steps were taken. **FedNova** needs
    #: this; leaving it `None` means "not running FedNova".
    local_steps: int | None = None
    #: This client's loss at the round's *starting* weights, before
    #: training. **q-FedAvg** needs this. Note the direction of the
    #: incentive: q-FedAvg weights *up* whoever reports a high loss.
    local_loss: float | None = None
    #: A control variate, same length as `weights`. **SCAFFOLD** needs
    #: this.
    control_variate: Sequence[float] | None = None

    def __post_init__(self) -> None:
        if self.num_samples < 0:
            raise ValueError(f"num_samples must not be negative, got {self.num_samples}")
        if self.control_variate is not None and len(self.control_variate) != len(self.weights):
            raise ValueError(
                f"control_variate has {len(self.control_variate)} entries but weights "
                f"have {len(self.weights)} — SCAFFOLD's correction is per-parameter, and "
                "the server cannot check this for you (it is opaque to model architecture)"
            )


class ClientApp(ABC):
    """Subclass this and implement `train`."""

    @abstractmethod
    def train(self, weights: list[float], round: int) -> TrainResult:
        """Train on local data, starting from `weights`.

        `weights` is flat and architecture-free — unflatten it into your
        own model. If [`is_placeholder_init`] is true for it, the server
        had no checkpoint yet and you should use your own initialization
        instead of these zeros.

        Return the *trained weights*, not a delta. Conflux transmits full
        weight vectors; the server computes whatever difference a given
        aggregator needs.
        """

    def on_round_start(self, round: int) -> None:
        """Called before `train`. Override for logging or setup."""

    def on_control_variate(self, c: list[float]) -> None:
        """The server's global control variate `c`, when the configured
        aggregator maintains one.

        **SCAFFOLD only.** Called immediately before `train` on rounds
        where the server sent one, so an implementation can hold it and
        apply the `(c - c_i)` correction during local training. Never
        called otherwise — which is every aggregator but `scaffold` — so
        this stays a no-op for clients that do not implement it.

        `c` has the same length as the round's weights.
        """

    def on_round_end(self, round: int, accepted: bool) -> None:
        """Called after submission. `accepted` is the server's own answer
        — `False` usually means the round closed on quorum or timeout
        while this client was still training, which is ordinary and not
        an error."""


def _chunks(
    client_id: str,
    round: int,
    result: TrainResult,
    chunk_bytes: int,
) -> Iterator[pb2.DeltaChunk]:
    """Splits one result into `DeltaChunk`s.

    `data` and `control_variate` are split in lockstep at the same
    offsets, because the server concatenates each in `chunk_index` order
    and expects them to correspond. The scalars (`num_samples`,
    `local_steps`, `local_loss`) are repeated on every chunk — the
    server reads them from whichever chunk arrives first, so repeating
    them costs a few bytes and removes any dependence on chunk 0
    arriving first.
    """
    payload = encode_weights(result.weights)
    variate = encode_weights(result.control_variate) if result.control_variate else None

    # Align the split to f32 boundaries so a chunk never cuts a float in
    # half — the server concatenates raw bytes and would not notice.
    step = max(4, (chunk_bytes // 4) * 4)
    total = max(1, (len(payload) + step - 1) // step)

    for index in range(total):
        lo, hi = index * step, min((index + 1) * step, len(payload))
        yield pb2.DeltaChunk(
            client_id=client_id,
            round=round,
            chunk_index=index,
            total_chunks=total,
            data=payload[lo:hi],
            num_samples=result.num_samples,
            local_steps=result.local_steps,
            local_loss=result.local_loss,
            control_variate=variate[lo:hi] if variate is not None else None,
        )


def run(
    app: ClientApp,
    address: str = "127.0.0.1:47100",
    client_id: str = "client-1",
    rounds: int = 1,
    auth_token: str = "client-token",
    chunk_bytes: int = DEFAULT_CHUNK_BYTES,
    poll_interval: float = 0.2,
    verbose: bool = True,
) -> int:
    """Runs `app` for `rounds` rounds. Returns how many were accepted.

    `address` is **`conflux-node`'s local loopback listener**, not the
    server's. That hop is plaintext and localhost-only by design: the
    node has already authenticated upstream on this client's
    behalf, so `auth_token` here is not a credential and is ignored.
    """

    def log(message: str) -> None:
        if verbose:
            print(f"[{client_id}] {message}", flush=True)

    channel = grpc.insecure_channel(address)
    stub = pb2_grpc.FlTransportStub(channel)
    stub.Register(pb2.RegisterRequest(client_id=client_id, auth_token=auth_token))
    log(f"registered with {address}")

    last_round = None
    completed = 0
    while completed < rounds:
        # Wait for a round we have not already done. The node answers
        # immediately with whatever is current, so without this check a
        # fast client submits the same round repeatedly.
        while True:
            task = stub.FetchTask(pb2.FetchTaskRequest(client_id=client_id))
            if task.round != last_round:
                break
            time.sleep(poll_interval)

        weights = decode_weights(task.model_weights)
        app.on_round_start(task.round)

        # SCAFFOLD's `c`, when the server's aggregator maintains one.
        # `HasField` rather than truthiness: an unset `optional bytes`
        # reads as empty, and "no control variate" is a different fact
        # from "a control variate that happens to be empty" — the same
        # distinction `local_loss` needs on the way up.
        if task.HasField("control_variate"):
            app.on_control_variate(decode_weights(task.control_variate))
        result = app.train(weights, task.round)

        if len(result.weights) != len(weights):
            raise ValueError(
                f"train() returned {len(result.weights)} weights for a "
                f"{len(weights)}-weight model. Every client in a round must agree "
                "on the model's shape; the server rejects a batch that does not."
            )

        try:
            ack = stub.SubmitDelta(_chunks(client_id, task.round, result, chunk_bytes))
            accepted = ack.accepted
        except grpc.RpcError as e:
            # A round closing on quorum or timeout mid-training is
            # ordinary, not a failure. Move on to the next one rather
            # than retrying into a round that is over.
            log(f"round {task.round} rejected ({e.code().name}); continuing")
            last_round = task.round
            app.on_round_end(task.round, accepted=False)
            continue

        last_round = task.round
        completed += 1
        app.on_round_end(task.round, accepted=accepted)
        log(f"round {task.round}: submitted, accepted={accepted}")

    log(f"done — completed {completed} rounds")
    return completed


def main(app: ClientApp, description: str | None = None) -> None:
    """Standard argument parsing, so a client is a few lines end to end.

    Kept separate from `run` so a caller embedding a `ClientApp` in
    something larger is not forced to accept an argument parser too.
    """
    import argparse

    parser = argparse.ArgumentParser(description=description or app.__class__.__doc__)
    parser.add_argument("--address", default="127.0.0.1:47100")
    parser.add_argument("--client-id", default="client-1")
    parser.add_argument("--rounds", type=int, default=1)
    parser.add_argument("--quiet", action="store_true")
    args = parser.parse_args()

    try:
        run(
            app,
            address=args.address,
            client_id=args.client_id,
            rounds=args.rounds,
            verbose=not args.quiet,
        )
    except grpc.RpcError as e:
        print(f"[{args.client_id}] RPC failed: {e}", file=sys.stderr)
        sys.exit(1)
