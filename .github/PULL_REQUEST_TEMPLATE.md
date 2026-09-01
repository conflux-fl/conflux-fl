## What and why

<!-- What changed, and why the obvious alternative was not chosen.
     The "why" is the part that is expensive to reconstruct later. -->

## How it was verified

<!-- Not "tests pass" — what did you actually run, and what would have
     failed if the change were wrong? If you fixed a defect, a test that
     was red before the fix is the strongest evidence. -->

- [ ] `cargo fmt --all` and `cargo clippy --workspace --all-targets` clean
- [ ] `cargo test --workspace` passes
- [ ] Python client touched? `cd python/conflux_client && ./ci_smoke.sh 2`

## Checklist

- [ ] Fallible public functions return a `thiserror` enum, not a `String`
- [ ] Comments explain *why*, not *what*
- [ ] New aggregation method? It cites its paper, and the implementation
      is literal (ADR 0008)
- [ ] Keeps cross-round state? It has cross-round tests
- [ ] Numeric code accumulates in `f64` and normalizes before summing
- [ ] Touches `conflux-proto`? Say so — it reaches every deployed client
      and both SDKs at once
