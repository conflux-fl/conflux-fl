"""Tests for the ClientApp SDK (ADR 0005 question 3).

Run: python3 -m pytest python/conflux_client/tests/ -q
     (or: python3 python/conflux_client/tests/test_app.py)

Deliberately dependency-free beyond grpcio + the generated stubs, so it
runs anywhere the SDK itself does — no PyTorch, no pytest required.
"""

import os
import sys

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))

from app import (  # noqa: E402
    ClientApp,
    TrainResult,
    _chunks,
    decode_weights,
    encode_weights,
    is_placeholder_init,
)


def test_codec_round_trips():
    w = [1.0, -2.5, 3.75]
    assert decode_weights(encode_weights(w)) == w
    assert decode_weights(encode_weights([])) == []


def test_codec_rejects_a_non_multiple_of_four():
    # The one failure the codec can have. A silent truncation here would
    # hand the model a shorter weight vector than it expects.
    try:
        decode_weights(b"\x00\x01\x02")
        raise AssertionError("should have rejected")
    except ValueError as e:
        assert "whole number of f32s" in str(e)


def test_placeholder_detection():
    # All-zero is the server's "no checkpoint yet" signal, and a client
    # that trains from it cannot break symmetry in a ReLU network.
    assert is_placeholder_init([0.0, 0.0, 0.0])
    assert not is_placeholder_init([0.0, 0.1])
    assert not is_placeholder_init([])


def test_control_variate_length_is_checked_client_side():
    # The server cannot check this — it is opaque to model architecture
    # (ADR 0004) — so a mismatch would travel all the way to an
    # aggregator before failing. Catch it where the shape is known.
    try:
        TrainResult(weights=[1.0, 2.0], num_samples=1, control_variate=[1.0])
        raise AssertionError("should have rejected")
    except ValueError as e:
        assert "per-parameter" in str(e)


def test_chunking_splits_vectors_in_lockstep_and_repeats_scalars():
    r = TrainResult(
        weights=[float(i) for i in range(10)],
        num_samples=7,
        local_steps=3,
        local_loss=0.5,
        control_variate=[i * 0.1 for i in range(10)],
    )
    cs = list(_chunks("c1", 4, r, chunk_bytes=16))

    assert len(cs) == 3, "40 bytes at 16 bytes/chunk"
    assert [c.chunk_index for c in cs] == [0, 1, 2]
    assert all(c.total_chunks == 3 for c in cs)

    # Scalars repeat on every chunk, so the server never depends on
    # chunk 0 arriving first.
    assert all(c.num_samples == 7 and c.local_steps == 3 for c in cs)

    # Both vectors reassemble, and at the same offsets — the server
    # concatenates each independently in chunk_index order.
    assert b"".join(c.data for c in cs) == encode_weights(r.weights)
    assert b"".join(c.control_variate for c in cs) == encode_weights(r.control_variate)

    # No chunk may cut an f32 in half; the server concatenates raw bytes
    # and would not notice.
    assert all(len(c.data) % 4 == 0 for c in cs)


def test_absent_optional_fields_are_absent_not_zero():
    """The property the whole `optional` design rests on.

    protobuf reads an unset `optional float` as `0.0`, so a client that
    tests truthiness cannot tell "not running q-FedAvg" from "reported a
    loss of exactly zero". `HasField` is the only correct check, and the
    two really are different on the wire.
    """
    absent = next(_chunks("c", 1, TrainResult(weights=[1.0], num_samples=1), 1 << 20))
    zero = next(
        _chunks("c", 1, TrainResult(weights=[1.0], num_samples=1, local_loss=0.0), 1 << 20)
    )

    assert not absent.HasField("local_loss")
    assert zero.HasField("local_loss")
    assert absent.local_loss == zero.local_loss == 0.0, "both *read* as 0.0"
    assert len(absent.SerializeToString()) < len(zero.SerializeToString()), (
        "an absent optional emits no bytes at all — which is what keeps a "
        "pre-ADR-0012 client byte-compatible"
    )


def test_a_subclass_only_has_to_write_train():
    class Minimal(ClientApp):
        def train(self, weights, round):
            return TrainResult(weights=[w + 1.0 for w in weights], num_samples=1)

    app = Minimal()
    out = app.train([1.0, 2.0], 0)
    assert list(out.weights) == [2.0, 3.0]
    # The optional hooks have working defaults.
    app.on_round_start(0)
    app.on_round_end(0, accepted=True)


if __name__ == "__main__":
    failures = 0
    for name, fn in sorted(globals().items()):
        if name.startswith("test_") and callable(fn):
            try:
                fn()
                print(f"  ok   {name}")
            except Exception as e:  # noqa: BLE001
                failures += 1
                print(f"  FAIL {name}: {e}")
    print(f"\n{'all passed' if not failures else f'{failures} failed'}")
    sys.exit(1 if failures else 0)
