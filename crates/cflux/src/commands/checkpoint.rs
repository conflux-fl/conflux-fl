//! `cflux checkpoint` — what a durable store actually holds.
//!
//! The question this answers is "did the thing I think ran, run?", asked
//! from outside the server. A round number in a log says a round closed;
//! a checkpoint says a model was written, and its shape says whether the
//! model is one anybody should resume from.
//!
//! It reads the store directly rather than through the server, which is
//! why it works while the server is down — and why an in-memory store is
//! not readable here at all. That backend keeps its checkpoint inside the
//! server's own process, and a separate process cannot reach into it.

use clap::{Args, Subcommand};
use conflux_server::{StoreBackend, backend_selection_from_env};
use conflux_store::{AnyStore, PostgresStore, S3Store, Store, StoreError};

use crate::format::Report;
use crate::{CliError, EXIT_NEGATIVE, guide};

#[derive(Args)]
#[command(after_help = guide("checkpoint"))]
pub struct CheckpointArgs {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Every round the configured store holds a checkpoint for.
    List,
    /// One round's shape: how many parameters, how large, and whether
    /// anything in it is not a number.
    Show {
        /// The round to inspect.
        round: u64,
    },
}

/// What a caller can be told about a checkpoint without downloading it
/// themselves.
struct Shape {
    parameters: usize,
    l2_norm: f64,
    min: f32,
    max: f32,
    non_finite: usize,
    placeholder: bool,
}

impl Shape {
    fn of(weights: &[f32]) -> Self {
        // Accumulated in `f64` before the square root: a 10⁶-parameter
        // model summing squares in `f32` loses the small terms entirely
        // once the running total is large, and the norm is the number
        // most likely to be compared between rounds.
        let mut sum_squares = 0.0f64;
        let mut min = f32::INFINITY;
        let mut max = f32::NEG_INFINITY;
        let mut non_finite = 0;
        for &w in weights {
            if w.is_finite() {
                sum_squares += (w as f64) * (w as f64);
                // Positive comparisons, so a NaN that slipped past the
                // guard above could not silently win either bound.
                if w < min {
                    min = w;
                }
                if w > max {
                    max = w;
                }
            } else {
                non_finite += 1;
            }
        }
        Self {
            parameters: weights.len(),
            l2_norm: sum_squares.sqrt(),
            min: if min.is_finite() { min } else { 0.0 },
            max: if max.is_finite() { max } else { 0.0 },
            non_finite,
            placeholder: weights.iter().all(|w| *w == 0.0),
        }
    }
}

pub fn run(args: CheckpointArgs) -> Result<Report, CliError> {
    let backends = backend_selection_from_env().map_err(CliError::ServerEnv)?;

    // Named before anything connects, because "your checkpoints are
    // somewhere this command cannot look" is an answer, not an error —
    // and it is the answer for the default configuration.
    if matches!(backends.store, StoreBackend::Memory) {
        let text = "store backend is in-memory: checkpoints live inside the server's own \
                    process and no separate process can read them.\nSet \
                    CONFLUX_STORE_BACKEND=postgres or =s3 for checkpoints this command can \
                    inspect.\n"
            .to_string();
        return Ok(Report::plain(
            text,
            serde_json::json!({
                "ok": false,
                "backend": "memory",
                "reason": "checkpoints are process-local and unreadable from another process",
            }),
            EXIT_NEGATIVE,
        ));
    }

    let runtime = crate::commands::run::runtime()?;
    runtime.block_on(async {
        let store = connect(&backends.store).await?;
        match args.command {
            Command::List => list(&store).await,
            Command::Show { round } => show(&store, round).await,
        }
    })
}

async fn connect(backend: &StoreBackend) -> Result<AnyStore, CliError> {
    Ok(match backend {
        // Handled by the caller, which returns before reaching here.
        StoreBackend::Memory => unreachable!("in-memory is answered before connecting"),
        StoreBackend::Postgres { url } => AnyStore::Postgres(
            PostgresStore::connect(url)
                .await
                .map_err(CliError::Checkpoint)?,
        ),
        StoreBackend::S3 {
            endpoint,
            bucket,
            access_key,
            secret_key,
        } => AnyStore::S3(
            S3Store::connect(endpoint, bucket.clone(), access_key, secret_key)
                .await
                .map_err(CliError::Checkpoint)?,
        ),
    })
}

async fn list(store: &AnyStore) -> Result<Report, CliError> {
    let rounds = store
        .list_checkpoints()
        .await
        .map_err(CliError::Checkpoint)?;
    if rounds.is_empty() {
        return Ok(Report::plain(
            "no checkpoints saved yet\n".to_string(),
            serde_json::json!({ "ok": true, "rounds": [], "count": 0 }),
            0,
        ));
    }

    let first = rounds[0];
    let last = rounds[rounds.len() - 1];
    let mut text = format!("{} checkpoint(s), rounds {first} … {last}\n", rounds.len());

    // Elided rather than printed whole. A long experiment holds hundreds
    // of rounds, and a terminal full of consecutive integers is not an
    // answer to any question someone had. `--format json` carries them
    // all, for the caller that wants them.
    const HEAD_TAIL: usize = 8;
    if rounds.len() <= HEAD_TAIL * 2 + 1 {
        for round in &rounds {
            text.push_str(&format!("  {round}\n"));
        }
    } else {
        for round in &rounds[..HEAD_TAIL] {
            text.push_str(&format!("  {round}\n"));
        }
        text.push_str(&format!("  … {} more …\n", rounds.len() - HEAD_TAIL * 2));
        for round in &rounds[rounds.len() - HEAD_TAIL..] {
            text.push_str(&format!("  {round}\n"));
        }
    }

    // Gaps are worth naming — a run that checkpointed 1..40 and then 45
    // lost five rounds somewhere, and nothing else reports that. Counted
    // as gaps rather than as "n of m present": round numbers are not
    // required to start at one or to be dense, and a store shared by
    // several experiments makes "m" a number nobody wants to read.
    let gaps = rounds.windows(2).filter(|w| w[1] > w[0] + 1).count();
    if gaps > 0 {
        let widest = rounds
            .windows(2)
            .map(|w| w[1] - w[0] - 1)
            .max()
            .unwrap_or(0);
        text.push_str(&format!(
            "\n{gaps} gap(s) in the sequence, the widest {widest} round(s) wide\n"
        ));
    }
    Ok(Report::plain(
        text,
        serde_json::json!({ "ok": true, "rounds": rounds, "count": rounds.len() }),
        0,
    ))
}

async fn show(store: &AnyStore, round: u64) -> Result<Report, CliError> {
    let weights = match store.load_checkpoint(round).await {
        Ok(weights) => weights,
        Err(StoreError::NoCheckpoint) => {
            return Ok(Report::plain(
                format!("no checkpoint for round {round}\n"),
                serde_json::json!({ "ok": false, "round": round, "found": false }),
                EXIT_NEGATIVE,
            ));
        }
        Err(e) => return Err(CliError::Checkpoint(e)),
    };

    let shape = Shape::of(&weights);
    let mut text = format!(
        "round {round}\n  parameters:  {}\n  l2 norm:     {:.6}\n  range:       {:.6} … {:.6}\n",
        shape.parameters, shape.l2_norm, shape.min, shape.max
    );
    if shape.placeholder {
        text.push_str(
            "  placeholder: yes — every weight is zero, which is the seed the server \
             hands out before any client has trained\n",
        );
    }
    if shape.non_finite > 0 {
        // Called out rather than folded into the range: a single NaN
        // reaching a checkpoint poisons every client that resumes from
        // it, and the range alone would not show it.
        text.push_str(&format!(
            "  NOT A NUMBER: {} of {} weights are NaN or infinite — this checkpoint is \
             not safe to resume from\n",
            shape.non_finite, shape.parameters
        ));
    }

    let exit_code = if shape.non_finite > 0 {
        EXIT_NEGATIVE
    } else {
        0
    };
    Ok(Report::plain(
        text,
        serde_json::json!({
            "ok": shape.non_finite == 0,
            "round": round,
            "parameters": shape.parameters,
            "l2_norm": shape.l2_norm,
            "min": shape.min,
            "max": shape.max,
            "non_finite": shape.non_finite,
            "placeholder": shape.placeholder,
        }),
        exit_code,
    ))
}
