//! The `conflux-trusted-reference` sidecar binary.
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
//! extension the sidecar boundary exists to enable, and the dependency
//! it keeps out of `conflux-server`.

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

/// Reads `name` from the environment, or returns `default` when it is
/// unset. A value that is *set* but does not parse is an error rather
/// than a silent fallback: a typo in a learning rate must not quietly
/// train with a different one.
fn env_or<T>(name: &str, default: T) -> Result<T, String>
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    parse_or(name, std::env::var(name).ok(), default)
}

/// The pure half of [`env_or`], so it can be tested without touching
/// the process environment.
fn parse_or<T>(name: &str, raw: Option<String>, default: T) -> Result<T, String>
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    match raw {
        Some(raw) => raw
            .trim()
            .parse()
            .map_err(|e| format!("{name}={raw:?} is not valid: {e}")),
        None => Ok(default),
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    let addr: SocketAddr = env_or(
        "CONFLUX_SIDECAR_ADDR",
        "127.0.0.1:50100"
            .parse()
            .expect("the builtin default is a socket address"),
    )?;

    // No default path. The trusted dataset is the thing this whole
    // component exists to hold, and silently starting with an invented
    // one would be the worst possible failure: a running sidecar serving
    // a reference nobody chose.
    let dataset_path = std::env::var("CONFLUX_TRUSTED_DATASET_PATH").map_err(|_| {
        "CONFLUX_TRUSTED_DATASET_PATH is required — this sidecar has no meaningful default \
         trusted dataset, and starting without one would serve a reference nobody chose"
    })?;

    let dataset = read_dataset(&dataset_path)?;
    let learning_rate = env_or("CONFLUX_TRUSTED_LEARNING_RATE", 0.05_f32)?;
    let steps = env_or("CONFLUX_TRUSTED_STEPS", 200_usize)?;

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unset_variable_yields_the_default() {
        assert_eq!(parse_or("X", None, 200_usize), Ok(200));
    }

    #[test]
    fn a_set_value_is_parsed() {
        assert_eq!(parse_or("X", Some(" 0.5 ".into()), 0.05_f32), Ok(0.5));
    }

    #[test]
    fn a_malformed_value_is_an_error_not_the_default() {
        let err = parse_or("CONFLUX_TRUSTED_STEPS", Some("2OO".into()), 200_usize)
            .expect_err("must not fall back silently");
        assert!(err.contains("CONFLUX_TRUSTED_STEPS=\"2OO\""), "{err}");
    }
}
