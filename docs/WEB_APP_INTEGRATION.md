# Integrating Conflux into your own web application

Your product's backend and frontend can be in any stack — FastAPI, Django,
Node, Rust/Axum, whatever — with a React (or any) frontend. This doc
answers: **what's the actual interface to Conflux, and does it matter what
language your app is written in?**

Short version: **no, it doesn't matter.** Conflux's integration surface is
plain HTTP/JSON and gRPC, both language-agnostic. The one thing that *does*
change with your stack is which of two integration patterns is available to
you — and for a non-Rust backend, only one of them is.

Read this after [`docs/E2E_TESTING.md`](E2E_TESTING.md) if you haven't run
Conflux at all yet. [`docs/FLOWER_COMPARISON.md`](FLOWER_COMPARISON.md)
cross-checks Conflux's internal design against Flower's, architecture to
architecture — a different comparison than this one, which is about
plugging FL into an app you're already building.

## The interface, independent of language

Conflux exposes two protocols, and your web app's backend only needs one of
them:

| Protocol | Port (default) | Who speaks it | Language constraint |
|---|---|---|---|
| **HTTP/JSON admin API** | `:8080` | Your backend, to drive/observe one experiment | None — any HTTP client |
| **gRPC `FlTransport`** | `:50051` | FL clients (`conflux-node`, then Python `ClientApp`) | None in principle (any gRPC codegen toolchain), but this is *not* what your backend calls |

Your FastAPI/Django/Express/Axum backend talks to the **admin API only** —
it's the same JSON-over-HTTP contract regardless of what calls it. The
`FlTransport` gRPC service is between Conflux and the machines actually
doing training; your product backend doesn't need a gRPC client at all
unless it's *also* orchestrating participant devices directly (uncommon —
usually a separate concern).

`conflux-server`'s admin router
([`crates/conflux-server/src/http.rs`](../crates/conflux-server/src/http.rs)):

| Method | Path | Does |
|---|---|---|
| `GET` | `/health` | Liveness check — `"ok"` |
| `GET` | `/round/status` | `{"round": <u64>}` |
| `POST` | `/clients/register` | `{"client_id": "..."}` → `{"accepted": bool}` |
| `GET` | `/admin/allowlist` | `{"client_ids": [...]}` |
| `POST` | `/admin/allowlist` | `{"client_id": "...", "identity": {"kind": "cert_fingerprint"\|"shared_token", ...}}` |
| `DELETE` | `/admin/allowlist/{client_id}` | Revokes a node |

This is real and already exercised — the E2E demo scripts poll `/health`
today. There's no SDK wrapper around it in any language yet; it's just
JSON, called with whatever your framework's normal HTTP client is.

## Two integration patterns — only one applies to a non-Rust backend

### Pattern A — sidecar process (the one you'll use)

`conflux-server` runs as its own OS process (or container), one per
experiment ([ADR 0003](adr/0003-no-multi-tenancy.md) — Conflux is
explicitly single-tenant per process; running many experiments means
running many processes, and that orchestration is deliberately left to
whatever deploys it — i.e., your app). Your backend spawns it, records
where it's listening, and calls its admin API. This is stack-agnostic by
construction, since it's just a subprocess plus HTTP calls:

**FastAPI:**

```python
import asyncio, httpx
from fastapi import FastAPI

app = FastAPI()

async def spawn_experiment_server(cfg: ExperimentConfig, http_port: int, grpc_port: int) -> asyncio.subprocess.Process:
    env = {
        "CONFLUX_TOPOLOGY": cfg.topology,             # "cross_silo", etc.
        "CONFLUX_MODE": cfg.mode,                      # "research" | "production"
        "CONFLUX_AGGREGATOR": cfg.aggregator,           # "fedavg" | "krum" | ...
        "CONFLUX_QUORUM": str(cfg.quorum),
        "CONFLUX_ROUND_TIMEOUT_SECS": str(cfg.round_timeout_secs),
        "CONFLUX_INITIAL_WEIGHTS_DIM": str(cfg.model_dim),
    }
    return await asyncio.create_subprocess_exec(
        "conflux-server", env=env, stdout=asyncio.subprocess.DEVNULL,
    )

@app.get("/experiments/{experiment_id}/status")
async def experiment_status(experiment_id: str):
    admin_addr = await lookup_admin_addr(experiment_id)  # your own DB
    async with httpx.AsyncClient() as client:
        resp = await client.get(f"{admin_addr}/round/status")
        return resp.json()
```

**Django** (views are sync by default — either use `sync_to_async`/an ASGI
view, or push the spawn/poll work to Celery so a request handler never
blocks on it):

```python
import requests
from django.http import JsonResponse

def experiment_status(request, experiment_id):
    admin_addr = Experiment.objects.get(id=experiment_id).conflux_admin_addr
    resp = requests.get(f"{admin_addr}/round/status", timeout=5)
    return JsonResponse(resp.json())
```

```python
# tasks.py — spawning is slow/long-lived, so it's a Celery task,
# not something a request handler does inline
import subprocess
from celery import shared_task

@shared_task
def spawn_experiment_server(experiment_id, cfg: dict):
    env = {**os.environ, "CONFLUX_TOPOLOGY": cfg["topology"], "CONFLUX_AGGREGATOR": cfg["aggregator"], ...}
    proc = subprocess.Popen(["conflux-server"], env=env, stdout=subprocess.DEVNULL)
    Experiment.objects.filter(id=experiment_id).update(pid=proc.pid, conflux_admin_addr="http://127.0.0.1:8080")
```

Same shape either way: your framework's normal async/sync HTTP client,
calling plain JSON endpoints. Nothing Rust-specific about any of this.

### Pattern B — embed the library — Rust backends only

If (and only if) your API layer is itself Rust — e.g. Axum — you can import
`conflux-server`'s crates directly and merge its router into your own,
running one process instead of two. This isn't available from Python/Node,
since it means linking Conflux's Rust code into your binary at compile
time, not calling it over a wire. If your stack is FastAPI or Django, skip
this section entirely — Pattern A is your only (and honestly, simplest)
option regardless.

```rust
// Axum-only — merging conflux-server's admin router into your own
use conflux_server::{AppState, router as conflux_admin_router};

let platform_routes = Router::new().route("/experiments", post(create_experiment));
let app = platform_routes.nest("/conflux", conflux_admin_router(conflux_state));
```

## Two real constraints this changes for a non-Rust backend

These matter more now than they would for an Axum backend, because Pattern
B — which would have sidestepped both — isn't on the table:

1. **`conflux-server` defaults to `127.0.0.1`, but this is now overridable.**
   [`main.rs`](../crates/conflux-server/src/main.rs) reads
   `CONFLUX_GRPC_ADDR`/`CONFLUX_HTTP_ADDR` (default `127.0.0.1:50051` /
   `127.0.0.1:8080` when unset). If your FastAPI/Django backend runs in a
   *different* container than `conflux-server`, set
   `CONFLUX_HTTP_ADDR=0.0.0.0:8080` (or a specific reachable interface) so
   it's not loopback-only. The default stays loopback deliberately — see
   the auth point below — so treat binding wider as an explicit, considered
   choice for your deployment, not something you flip without also solving
   point 2.
2. **The admin API has no auth of its own.** `http.rs`'s router applies no
   auth middleware — anything that can reach `:8080` can read round status
   and rewrite the node allowlist. That's fine as long as it's genuinely
   unreachable except from your own backend (the constraint above actually
   helps here — loopback-only is also a de facto access control, as long as
   nothing else shares that namespace). Your backend is the only thing with
   real authentication (your own user/session system) — it's the trust
   boundary, and it must stay one. Never expose `conflux-server`'s admin
   port directly to the public internet or to your React frontend.

## Where React fits

**React never talks to `conflux-server` directly — it talks to your own
backend, same as it does for everything else.** Your FastAPI/Django API is
the only thing that calls Conflux's admin API; React calls *your* API
(`GET /api/experiments/:id/status`, say), and your handler is the one that
turns around and calls `conflux-server`'s `/round/status` (or reads a
value you've already cached/polled into your own DB). This isn't a special
rule for Conflux — it's the same reason React doesn't talk to your Postgres
instance directly either. The admin API's lack of its own auth (point 2
above) makes this the only *safe* topology, not just the advisable one —
`CONFLUX_HTTP_ADDR` can now bind it somewhere a browser could technically
reach, but nothing should ever rely on that.

A typical shape:

```
React  --(your normal authenticated API)-->  FastAPI/Django backend
                                                     |
                                     (HTTP admin API, same network ns)
                                                     v
                                              conflux-server (per experiment)
                                                     |
                                          (gRPC FlTransport, separate concern)
                                                     v
                                    conflux-node -> Python ClientApp (on participant devices)
```

Your backend polls or receives updates from `conflux-server`'s admin API
and pushes them to React however you already push data (REST polling,
WebSocket, SSE — your existing pattern, nothing FL-specific).

## What's still missing for a real platform

- No auth on the admin HTTP API — acceptable only as long as it stays
  unreachable except from your own backend. `CONFLUX_HTTP_ADDR` now lets
  you bind wider than loopback (see above), which means it's now possible
  to accidentally expose an unauthenticated admin API — binding beyond
  loopback and *not* isolating the port behind your own network policy is
  the actual gap now, not the lack of the override itself.
- No experiment-listing endpoint — a `conflux-server` process only knows
  about the one experiment it's running; your own DB is the only source of
  truth for "what experiments exist," by design (ADR 0003).
- No client SDK in any language for *writing* participant-side training
  code (`flwr`'s `NumPyClient` has no equivalent) — deferred per
  [ADR 0005](adr/0005-python-sdk-deferred.md). If you're also writing that
  side, `python/conflux_client/examples/*/trainer_client.py` is the closest
  thing to a template today (hand-rolled generated protobuf stubs, not a
  packaged SDK).
- The reputation-filter finding from `docs/E2E_TESTING.md`'s "Real
  findings" section still applies regardless of what drives Conflux's admin
  API — it's a pipeline-internal issue, not an integration-layer one.

## Summary

- The interface is plain HTTP/JSON (`conflux-server`'s admin API,
  `:8080`) — any backend language calls it the same way, with its own
  normal HTTP client.
- One `conflux-server` process per experiment (ADR 0003); your backend
  spawns and tracks them (Pattern A) — this works identically whether
  that backend is FastAPI, Django, or Axum.
- Merging Conflux's router into your own process (Pattern B) only exists
  for a Rust backend — Python/Node backends always use Pattern A.
- Two real constraints to plan around today: the admin API is
  loopback-only *by default* (`CONFLUX_GRPC_ADDR`/`CONFLUX_HTTP_ADDR`
  override it if your backend runs in a separate container) and has no
  auth of its own — so widen the bind address only alongside your own
  network policy, never as a standalone fix.
- React (or any frontend) talks only to your own backend, never to Conflux
  directly — same architecture you'd use for any other subsystem.
