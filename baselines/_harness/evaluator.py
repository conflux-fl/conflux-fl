#!/usr/bin/env python3
"""One manifest-driven evaluator, for every recipe.

Registers as an ordinary client and fetches the global checkpoint each
round without ever submitting one, so the number it prints is the
server's model as clients actually see it — not a local reconstruction
of what it should be.

Prints `held_out_accuracy=<value>` per round, which is the token
`conflux-baselines` reads to decide whether a paper reproduced.

Fetching and scoring run on separate threads, and that split is the
whole reason this sees every round. Fetching is one cheap RPC; scoring
is a pass over the held-out set plus one per client shard, which takes
about as long as a round. Doing both in one loop meant the server moved
on while the evaluator was busy, and it observed every *other* round —
a convergence curve with half its points missing, and no way to recover
them, because `FetchTask` only ever returns the current round.
"""

from __future__ import annotations

import argparse
import queue
import statistics
import struct
import threading
import time

import torch

from . import _sdk, models, torch_model

_sdk.install()

import grpc  # noqa: E402
import fl_transport_pb2 as pb2  # noqa: E402
import fl_transport_pb2_grpc as pb2_grpc  # noqa: E402


def decode_weights(data: bytes) -> list[float]:
    return list(struct.unpack(f"<{len(data) // 4}f", data))


# How often to ask the server what round it is on. Fast enough that a
# round cannot open and close unseen; the call returns the current model
# and does nothing else, so the cost is bandwidth on loopback.
POLL_INTERVAL_S = 0.2

# How long the round number may stand still before this decides training
# has finished. Time rather than a poll count, so changing the interval
# above cannot silently change when the evaluator gives up.
STALL_TIMEOUT_S = 20.0


def _fetch_rounds(stub, out, target_rounds: int, deadline: float) -> None:
    """Captures each new round's weights as fast as they appear.

    Runs on its own thread and does no scoring, so the loop stays short
    enough that nothing slips past it. Ends with a `None`, which is what
    tells the consumer no further rounds are coming.
    """
    seen: set[int] = set()
    last_change = time.time()
    try:
        while len(seen) < target_rounds and time.time() < deadline:
            task = stub.FetchTask(pb2.FetchTaskRequest(client_id="evaluator"))
            if task.round in seen:
                if time.time() - last_change > STALL_TIMEOUT_S:
                    break
                time.sleep(POLL_INTERVAL_S)
                continue
            last_change = time.time()
            seen.add(task.round)
            out.put((task.round, task.model_weights))
    except Exception as e:  # noqa: BLE001 — re-raised on the main thread
        out.put(e)
    finally:
        # In a `finally`, because a consumer blocked on an empty queue has
        # no other way to learn this thread is done. Without it a dead
        # fetcher becomes a hang rather than an error, which is strictly
        # worse than the crash it replaced.
        out.put(None)


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

    # The fetcher runs ahead and buffers; this loop drains it. Scoring
    # may lag the server by a round or two on a fast federation, and
    # that is the trade — a complete curve reported slightly late beats
    # half a curve reported live. The queue is unbounded because the
    # weights of one run's rounds are a few megabytes at most.
    pending: queue.Queue = queue.Queue()
    deadline = time.time() + timeout_s
    fetcher = threading.Thread(
        target=_fetch_rounds,
        args=(stub, pending, target_rounds, deadline),
        daemon=True,
    )
    fetcher.start()

    while True:
        item = pending.get()
        if item is None:
            break
        if isinstance(item, Exception):
            # Raised here rather than swallowed on the fetcher thread, so
            # a transport failure still ends the process non-zero and the
            # runner reports it instead of reporting no metric.
            raise item
        round_number, raw_weights = item

        weights = decode_weights(raw_weights)
        # The same placeholder substitution every trainer makes: scoring
        # the server's all-zero checkpoint would report an accuracy about
        # the placeholder rather than about the model, and round one's
        # real number would read as "0.10, broken" instead of
        # "untrained, but a real initialization".
        if not torch_model.is_placeholder_init(weights):
            torch_model.unflatten(model, weights)
        acc, loss = torch_model.evaluate(model, X, y)

        line = f"round={round_number} held_out_accuracy={acc:.4f} held_out_loss={loss:.4f}"
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
