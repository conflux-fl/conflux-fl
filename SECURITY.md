# Security policy

## Supported versions

| Version | Supported |
|---|---|
| 0.1.x | Yes |

Conflux FL is pre-1.0. Fixes land on the current minor version; there
are no long-term support branches yet.

## Reporting a vulnerability

**Do not open a public issue.** Use GitHub's private vulnerability
reporting on this repository (Security → Report a vulnerability), which
opens a channel visible only to maintainers.

Please include what you have: affected version or commit, the
configuration involved (topology, mode, aggregator, whether node auth is
on), and the smallest reproduction you can manage. A configuration is
often the difference between a finding and a non-finding here.

You should get an acknowledgement within a few days. If a report turns
out to be a real vulnerability, we will agree a disclosure timeline with
you before publishing.

## What is in scope

Conflux FL coordinates untrusted clients by design, so the interesting
boundary is what a **participant** can do to a deployment:

- Anything a client can send that crashes, hangs, or corrupts the
  server. This has been a real class of bug here: a single client
  sending four bytes of `NaN` once panicked six aggregators, and a
  finite-but-extreme update could permanently poison a stateful
  aggregator's stored state.
- Bypassing node authentication (mTLS or JWT) or the node allow-list.
- Reaching the HTTP admin API without a token when one is configured.
- Causing a client's update to be attributed to another client.
- Any way to make the privacy accountant under-report cumulative
  epsilon.

## What is out of scope

These are known properties of the design, documented rather than
defended, and reporting them will get you this section back:

- **`num_samples` and `local_steps` are self-reported and
  unauthenticated.** Inflating them buys proportional influence. That is
  an assumption of the published aggregation methods, not a transport
  guarantee; the defense is a robust aggregator or not accepting
  unauthenticated counts. Values are bounded only against the degenerate
  case.
- **A malicious client can submit arbitrary weights.** That is the
  threat model the `robust` family exists for. If a specific method
  fails to deliver its paper's stated guarantee, that is a genuine bug —
  say which paper and which claim.
- **`conflux-attacks` contains working attack implementations.** It is
  dev/test-only, `publish = false`, and CI enforces that
  `conflux-server` cannot depend on it at any depth.
- **The node↔client loopback hop is plaintext.** It is localhost-only by
  design; the node has already authenticated upstream on the client's
  behalf.
- **Research-mode defaults are permissive** — unauthenticated admin API,
  stub clients allowed, budget-exhaustion warnings rather than refusals.
  Production mode refuses to start in those configurations. A finding
  that requires `CONFLUX_MODE=research` is a finding about research
  mode.

## Hardening a deployment

See [docs/USAGE.md](docs/USAGE.md) for the full list. The short version:
run in production mode, set `CONFLUX_ADMIN_TOKEN`, enable node auth,
bind the admin API to loopback, and set `max_update_bytes` to something
your models actually need.
