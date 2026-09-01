# conflux_client

Python `ClientApp` SDK (PyTorch-side training). Design deferred — see
the v1 specification §7 and Open Item 3 in §11.

Until the real SDK is designed, `stub_client.py` — fixed dummy weights, no
PyTorch dependency — stands in for end-to-end pipeline testing, permitted
only in research mode (`allow_stub_client`). It connects to `conflux-node`'s
local gRPC server over loopback, the same `.proto` used for the network hop
(ADR 0004).

## Running the stub client

```bash
python3 -m venv .venv
.venv/bin/pip install -r requirements.txt
./generate_proto.sh # regenerates fl_transport_pb2*.py — not committed
.venv/bin/python stub_client.py --address 127.0.0.1:47100
```

`conflux-node` must already be running and have registered with a running
`conflux-server` (see its phase brief for the full
three-process smoke test this was verified against).

## Poison mode (testing the `robust` aggregation family)

`--poison` (default off — every invocation above behaves unchanged)
submits a large-magnitude offset instead of honest training, standing in
for an adversarial `ClientApp`:

```bash
.venv/bin/python stub_client.py --address 127.0.0.1:47100 \
 --client-id attacker-1 --poison --poison-magnitude 1000.0
```

Run alongside one or more honest `stub_client.py` instances against a
`conflux-server` configured with `aggregator = "krum"` (or
`"multi_krum"`/`"trimmed_mean"`/`"median"`) to see the poisoned
submission's influence bounded, over the real network hop — see
[`docs/E2E_TESTING.md`](../../docs/E2E_TESTING.md) for the full harness
this is meant to plug into, and
its phase brief
for the aggregation methods themselves.
