# docs/

The documentation moved to **https://confluxfl.dev** — guides, reference,
tutorials, and the crate deep dives. This repository keeps code and
development files; this directory keeps only what the build needs:

- `AGGREGATION_CATALOG.generated.md` — emitted by
  `cargo run -p conflux-core --example catalog` from the strategy
  registry. A golden-file test
  (`crates/conflux-core/tests/catalog_generated.rs`) fails CI if it
  drifts. Regenerate it; don't hand-edit it. The site's
  [aggregation catalog](https://confluxfl.dev/reference/aggregation-catalog/)
  presents the same facts with the prose around them.

Where each former doc went is listed in the
[README's documentation map](../README.md#-documentation).
