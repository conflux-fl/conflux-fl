# 0007 — Explainable config resolution is mandatory

## Context
With two independent config axes plus CLI/env/file overrides (see
[[0001-two-axis-configuration]]), it's easy for an operator to be unsure
where a given parameter's value actually came from — especially in
production, where misdiagnosing a config source can mean debugging the wrong
layer entirely.

## Decision
Every resolved parameter logs its source at startup — `conflux-server` does
not reach "ready" without emitting this in full, via a `ConfigSource` enum
(`Cli`, `EnvVar`, `ExperimentFile`, `ModeProfile`, `TopologyProfile`,
`BuiltinFallback`). Format is configurable (`config_log_format: json | text`),
defaulting to JSON in production (machine-parseable for log aggregation and
audit trails) and text in research (readable at a glance), overridable
either way regardless of mode. This "say so, out loud" principle extends
beyond startup config to runtime decisions: `conflux-buffer` logs whether a
round closed on quorum or timeout; `conflux-reputation` logs every rejected
update with its score and threshold; `conflux-privacy`'s accountant logs
cumulative epsilon after every round, not just at exhaustion.

## Consequences
- Config resolution logging is not optional verbosity — it's a startup
  requirement, testable as part of Definition of Done for `conflux-config`.
- Any new mode/topology-scoped parameter must carry its `ConfigSource`
  through to the log line, not just its resolved value.
- The same principle applies to `conflux-buffer`, `conflux-reputation`, and
  `conflux-privacy` — new runtime decision points in those crates should log
  their reasoning by default, not just on failure.
