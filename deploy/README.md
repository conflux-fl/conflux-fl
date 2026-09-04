# `deploy/` — running real clients across machines

Thin launch helpers for a multi-host federation. They wrap env plus the
two binaries; they are not a control plane (that would be a CLI or an
orchestrator). Full guide: conflux-web → *Deploying 10–20 real clients*.

- **`run_client.sh`** — start one client (a `conflux-node` bridge + a
  trainer) on this machine. One command per participant.
- **`allowlist.sh`** — admit client ids to the server's node allow-list.

## The recipe

**1. Server** (one host, with real backends and — off a trusted network —
TLS). The topology sets the auth posture: `cross_device` (pull + JWT) or
`cross_silo` (push + mTLS). Enforce the allow-list with
`require_node_auth`.

**2. Admit each client** (only needed when `require_node_auth` is on):

```bash
ADMIN_TOKEN=$ADMIN_TOKEN deploy/allowlist.sh https://fl.example.org:8080 \
  site-1 site-2 site-3 site-4 site-5
```

Each client is admitted with the identity it will present — by default the
shared token `node-auth-token`. Give each a real secret instead:

```bash
IDENTITY_TOKEN=$SITE7_TOKEN ADMIN_TOKEN=$ADMIN_TOKEN \
  deploy/allowlist.sh https://fl.example.org:8080 site-7
```

or, under mTLS, by certificate fingerprint:

```bash
IDENTITY_FINGERPRINT=$SITE7_FP ADMIN_TOKEN=$ADMIN_TOKEN \
  deploy/allowlist.sh https://fl.example.org:8080 site-7
```

**3. On each client machine**, launch the node + a trainer:

```bash
CONFLUX_CLIENT_ID=site-7 \
CONFLUX_SERVER_ADDR=http://fl.example.org:50051 \
CONFLUX_NODE_AUTH_TOKEN=$SITE7_TOKEN \
  deploy/run_client.sh -- \
    python3 python/conflux_client/examples/e2e_pytorch_mnist/trainer_client.py \
      --address 127.0.0.1:47100 --client-id site-7 --shard shard.pt --rounds 30
```

The trainer's `--address` must match `CONFLUX_LOCAL_ADDR` (default
`127.0.0.1:47100`). The trainer can be Python (as above) or a Rust/Burn
`conflux-client` example — the node doesn't care.

## Auth & TLS (read by `conflux-node` from the environment)

| Env | Posture |
|---|---|
| *(none)* | plaintext — trusted network only |
| `CONFLUX_NODE_AUTH_TOKEN` | per-client token / JWT at registration |
| `CONFLUX_TLS_SERVER_CA_PATH` + `CONFLUX_TLS_DOMAIN` | server-authenticated TLS (identity via the token) |
| + `CONFLUX_TLS_CLIENT_CERT_PATH` + `CONFLUX_TLS_CLIENT_KEY_PATH` | mutual TLS (node presents its cert) |

TLS vars are validated as a set: nothing, the CA+domain pair, or all four —
anything else is a startup error.

## Validate on one host first

Before distributing, run the whole federation as processes on one machine
(`run_demo.sh`, or several `run_client.sh` against `127.0.0.1`) to confirm
the round loop and your model at the target client count.
