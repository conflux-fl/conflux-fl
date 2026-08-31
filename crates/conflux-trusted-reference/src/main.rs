//! The `conflux-trusted-reference` sidecar binary (ADR 0011).
//!
//! Run this **only** if a deployment has configured `aggregator =
//! "fltrust"` or `"zeno"`. Nothing else in Conflux needs it, and a
//! deployment that has not configured one never opens a connection to it.
//!
//! ```bash
//! CONFLUX_SIDECAR_ADDR=127.0.0.1:50100 \
//!   cargo run -p conflux-trusted-reference
//! ```
//!
//! # What this binary ships with, and what it does not
//!
//! It serves [`LinearLeastSquares`] over a trusted root dataset read from
//! a CSV file. That is a real model trained by real gradient descent —
//! not a stub — and it is a faithful trusted reference for a *linear*
//! task and nothing else.
//!
//! A deployment training anything else does not use this binary. It
//! implements `TrustedModel` against a runtime that can run its
//! architecture and serves that instead, via `serve()`. That is the
//! extension ADR 0011 exists to enable, and the dependency it refused to
//! put into `conflux-server`.

use std::net::SocketAddr;

use conflux_trusted_reference::{LinearLeastSquares, serve};

/// One training example: its features, and the target they should
/// predict. Named rather than spelled inline because the tuple-in-a-vec
/// shape reads as noise at every use site.
type Example = (Vec<f32>, f32);

/// Reads a trusted root dataset: one example per line, comma-separated,
/// with the target last. `1.0,2.0,5.0` is features `[1.0, 2.0]`, target
/// `5.0`.
///
/// A deliberately boring format. The root dataset is the one input the
/// entire FLTrust defense rests on, so the parser that reads it should be
/// something an operator can verify by looking at the file.
fn read_dataset(path: &str) -> Result<Vec<Example>, Box<dyn std::error::Error>> {
    let contents = std::fs::read_to_string(path)
        .map_err(|e| format!("could not read the trusted dataset at {path}: {e}"))?;

    let mut rows = Vec::new();
    for (line_no, line) in contents.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut values: Vec<f32> = Vec::new();
        for field in line.split(',') {
            values.push(field.trim().parse().map_err(|e| {
                format!("{path}:{}: {:?} is not a number: {e}", line_no + 1, field)
            })?);
        }
        let target = values
            .pop()
            .ok_or_else(|| format!("{path}:{}: empty row", line_no + 1))?;
        rows.push((values, target));
    }

    if rows.is_empty() {
        return Err(format!("{path} contains no examples").into());
    }
    // Every row must agree on width, or the gradient below is meaningless.
    let width = rows[0].0.len();
    if let Some(bad) = rows.iter().position(|(f, _)| f.len() != width) {
        return Err(format!(
            "{path}: row {} has {} features, but row 1 has {width}",
            bad + 1,
            rows[bad].0.len()
        )
        .into());
    }

    Ok(rows)
}

fn env_or<T: std::str::FromStr>(name: &str, default: T) -> T {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    let addr: SocketAddr = env_or("CONFLUX_SIDECAR_ADDR", "127.0.0.1:50100".to_string())
        .parse()
        .map_err(|e| format!("CONFLUX_SIDECAR_ADDR is not a socket address: {e}"))?;

    // No default path. The trusted dataset is the thing this whole
    // component exists to hold, and silently starting with an invented
    // one would be the worst possible failure: a running sidecar serving
    // a reference nobody chose.
    let dataset_path = std::env::var("CONFLUX_TRUSTED_DATASET_PATH").map_err(|_| {
        "CONFLUX_TRUSTED_DATASET_PATH is required — this sidecar has no meaningful default \
         trusted dataset, and starting without one would serve a reference nobody chose"
    })?;

    let dataset = read_dataset(&dataset_path)?;
    let learning_rate = env_or("CONFLUX_TRUSTED_LEARNING_RATE", 0.05_f32);
    let steps = env_or("CONFLUX_TRUSTED_STEPS", 200_usize);

    tracing::info!(
        path = %dataset_path,
        examples = dataset.len(),
        features = dataset[0].0.len(),
        learning_rate,
        steps,
        "loaded the trusted root dataset"
    );

    serve(addr, LinearLeastSquares::new(dataset, learning_rate, steps)).await
}
