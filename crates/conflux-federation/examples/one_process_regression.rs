//! A federation that actually learns, in one process, in one command.
//!
//! Run with:
//!   cargo run --example one_process_regression -p conflux-federation
//!
//! Three clients fit `y = w·x + b` over three features. Every hop is
//! real — local gRPC to each node, loopback gRPC to the server, the
//! server's own buffer, quorum flush, aggregation and checkpointing — so
//! this is a federated training run, not a simulation of one. The only
//! thing it does not do is use three machines.
//!
//! # Why three clients and not one
//!
//! Each client's data holds one feature **nearly constant**: client 0
//! barely varies `x₀`, client 1 barely varies `x₁`, client 2 barely
//! varies `x₂`. A coefficient you never vary is a coefficient you cannot
//! learn, so each client alone is structurally unable to recover the
//! model — and the run prints each solo fit next to the federated one to
//! show it rather than assert it.
//!
//! That property is the point. A demo where one client could have done
//! the job proves that the loop ran, which is not what anyone evaluating
//! a federated learning framework came to see.

use std::time::Duration;

use conflux_federation::{ClientApp, FederationConfig, TrainResult};

/// The model everyone is trying to recover.
const TRUE_W: [f32; 3] = [1.5, -2.0, 0.5];
const TRUE_B: f32 = 3.0;

/// Four parameters — `[w₀, w₁, w₂, b]` — which is also the server's
/// default `CONFLUX_INITIAL_WEIGHTS_DIM`. Chosen to match so this example
/// needs no environment at all.
const DIM: usize = 4;

const SAMPLES: usize = 120;
const LOCAL_STEPS: usize = 60;
const LEARNING_RATE: f32 = 0.05;
const ROUNDS: usize = 40;

/// A small deterministic PRNG, so two runs of this example produce the
/// same numbers. `rand` is not a dependency of this crate and is not
/// worth making one for an example's data generator.
struct Lcg(u64);

impl Lcg {
    fn new(seed: u64) -> Self {
        Self(seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1))
    }
    /// Uniform in `[-1, 1)`.
    fn next(&mut self) -> f32 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((self.0 >> 33) as f32) / ((1u64 << 30) as f32) - 1.0
    }
}

/// One client's data: every feature varies except `frozen`, which sits
/// near a constant.
fn shard(frozen: usize, seed: u64) -> (Vec<[f32; 3]>, Vec<f32>) {
    let mut rng = Lcg::new(seed);
    let mut xs = Vec::with_capacity(SAMPLES);
    let mut ys = Vec::with_capacity(SAMPLES);
    for _ in 0..SAMPLES {
        let mut x = [rng.next() * 2.0, rng.next() * 2.0, rng.next() * 2.0];
        // Not exactly constant — a column of identical values is a
        // degenerate case that could be special-cased away. Nearly
        // constant is the realistic version: the signal is there and is
        // too small to learn from.
        x[frozen] = 1.0 + rng.next() * 0.02;
        let noise = rng.next() * 0.1;
        let y = TRUE_W[0] * x[0] + TRUE_W[1] * x[1] + TRUE_W[2] * x[2] + TRUE_B + noise;
        xs.push(x);
        ys.push(y);
    }
    (xs, ys)
}

/// Mean squared error of `params` on `(xs, ys)`.
fn mse(params: &[f32], xs: &[[f32; 3]], ys: &[f32]) -> f32 {
    let total: f32 = xs
        .iter()
        .zip(ys)
        .map(|(x, y)| {
            let pred = params[0] * x[0] + params[1] * x[1] + params[2] * x[2] + params[3];
            (pred - y) * (pred - y)
        })
        .sum();
    total / xs.len() as f32
}

/// `steps` full-batch gradient steps on MSE, starting from `params`.
fn descend(mut params: Vec<f32>, xs: &[[f32; 3]], ys: &[f32], steps: usize) -> Vec<f32> {
    let n = xs.len() as f32;
    for _ in 0..steps {
        let mut grad = [0.0f32; DIM];
        for (x, y) in xs.iter().zip(ys) {
            let pred = params[0] * x[0] + params[1] * x[1] + params[2] * x[2] + params[3];
            let error = pred - y;
            for (j, xj) in x.iter().enumerate() {
                grad[j] += 2.0 * error * xj;
            }
            grad[3] += 2.0 * error;
        }
        for (p, g) in params.iter_mut().zip(grad) {
            *p -= LEARNING_RATE * g / n;
        }
    }
    params
}

struct Regressor {
    xs: Vec<[f32; 3]>,
    ys: Vec<f32>,
    /// The last global model this client was handed.
    ///
    /// Shared with `main`, because `run` takes ownership of the apps and
    /// the caller otherwise has no way to see what was learned. Reading
    /// the server's final checkpoint back would be the better answer and
    /// is not yet exposed — noted in the crate docs.
    seen: std::sync::Arc<std::sync::Mutex<Vec<f32>>>,
}

impl ClientApp for Regressor {
    fn train(&mut self, weights: &[f32], _round: u64) -> TrainResult {
        // `weights` is flat and architecture-free. Round one arrives as
        // the server's placeholder zeros, which for a linear model is a
        // perfectly good shared starting point — and shared is what
        // matters, since averaging two models that started from
        // different places averages nothing meaningful.
        *self.seen.lock().expect("mutex poisoned") = weights.to_vec();
        let updated = descend(weights.to_vec(), &self.xs, &self.ys, LOCAL_STEPS);
        let loss = mse(&updated, &self.xs, &self.ys);
        TrainResult::new(updated, self.xs.len() as u64)
            .with_local_steps(LOCAL_STEPS as u32)
            .with_local_loss(loss)
    }
}

/// Sets one of the server's `CONFLUX_*` variables, unless the person
/// running this already set it.
///
/// Called before any runtime exists, which is what makes it sound: the
/// process is still single-threaded here, so nothing can be reading the
/// environment while it changes. The same thing `cflux run` does, for the
/// same reason — an in-process server still reads its configuration from
/// the environment, because one process is one experiment.
fn default_env(var: &str, value: &str) {
    if std::env::var_os(var).is_none() {
        // SAFETY: single-threaded — `main` has not built a runtime or
        // spawned a thread yet, and every reader of these variables runs
        // after the runtime starts below.
        unsafe { std::env::set_var(var, value) };
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Differential privacy off, said out loud rather than done quietly.
    //
    // The framework's defaults clip every update to an L2 norm of 1 and
    // add Gaussian noise at a multiplier of 1. That is the right default
    // and it is fatal to this demo: the model being recovered has a norm
    // near 4, so clipping alone discards most of the signal and forty
    // rounds converge to nothing recognizable. A demo that left them on
    // would "run" and teach the reader something false about whether it
    // learned.
    //
    // The honest version is to turn them off, say so, and let the
    // trade-off be visible — which is also the more useful lesson.
    default_env("CONFLUX_CLIP_NORM", "1000");
    default_env("CONFLUX_NOISE_MULTIPLIER", "0");

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()),
        )
        .init();

    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(federate())
}

async fn federate() -> Result<(), Box<dyn std::error::Error>> {
    const CLIENTS: usize = 3;
    let shards: Vec<(Vec<[f32; 3]>, Vec<f32>)> =
        (0..CLIENTS).map(|i| shard(i, 1_000 + i as u64)).collect();

    // A test set nobody trains on, drawn with every feature varying — the
    // only fair way to ask whether the model generalizes past the slice
    // its owner happened to see.
    let (test_xs, test_ys) = {
        let mut rng = Lcg::new(9_999);
        let mut xs = Vec::new();
        let mut ys = Vec::new();
        for _ in 0..400 {
            let x = [rng.next() * 2.0, rng.next() * 2.0, rng.next() * 2.0];
            let y = TRUE_W[0] * x[0] + TRUE_W[1] * x[1] + TRUE_W[2] * x[2] + TRUE_B;
            xs.push(x);
            ys.push(y);
        }
        (xs, ys)
    };

    println!("Federated linear regression — everything in this one process.\n");
    println!("  true model: y = {TRUE_W:?} · x + {TRUE_B}");
    println!(
        "  differential privacy is OFF for legibility (CONFLUX_CLIP_NORM=1000, \
         CONFLUX_NOISE_MULTIPLIER=0);\n  with the defaults this problem does not \
         converge in {ROUNDS} rounds, which is the trade-off working, not a bug.\n  \
         The accountant will warn every round that there is no guarantee. That is it \
         doing its job.\n"
    );

    // What each client gets on its own, with the same budget of steps the
    // federation spends in total. Printed first, so the comparison is not
    // a claim made after the fact.
    println!("  Each client alone (frozen feature in brackets):");
    for (i, (xs, ys)) in shards.iter().enumerate() {
        let solo = descend(vec![0.0; DIM], xs, ys, ROUNDS * LOCAL_STEPS);
        println!(
            "    client-{i} [x{i}]  test MSE {:>8.4}   w = [{:.2}, {:.2}, {:.2}], b = {:.2}",
            mse(&solo, &test_xs, &test_ys),
            solo[0],
            solo[1],
            solo[2],
            solo[3],
        );
    }

    let seen = std::sync::Arc::new(std::sync::Mutex::new(vec![0.0f32; DIM]));
    let seen_for_clients = std::sync::Arc::clone(&seen);

    let started = std::time::Instant::now();
    let summary = conflux_federation::run(
        FederationConfig {
            nodes: CLIENTS,
            rounds: ROUNDS,
            timeout: Duration::from_secs(300),
            ..FederationConfig::default()
        },
        move |i| Regressor {
            xs: shards[i].0.clone(),
            ys: shards[i].1.clone(),
            seen: std::sync::Arc::clone(&seen_for_clients),
        },
    )
    .await?;
    let elapsed = started.elapsed();

    println!(
        "\n  Federated: {} client(s) × {} round(s) in {:.1}s, over real gRPC on \
         ports nobody chose ({} and {}).",
        summary.clients.len(),
        ROUNDS,
        elapsed.as_secs_f32(),
        summary.server_grpc_addr,
        summary.server_http_addr,
    );
    for outcome in &summary.clients {
        println!(
            "    {} completed {} round(s)",
            outcome.client_id, outcome.rounds_completed
        );
    }

    // The global model as it stood at the start of the final round —
    // i.e. after `ROUNDS - 1` aggregations. Said precisely rather than
    // called "the final model", because the last round's own aggregate
    // is written to the server's checkpoint after every client has
    // already been handed its work.
    let federated = seen.lock().expect("mutex poisoned").clone();
    println!(
        "\n  Federated model after {} aggregation(s):\n    test MSE {:>8.4}   \
         w = [{:.2}, {:.2}, {:.2}], b = {:.2}",
        ROUNDS - 1,
        mse(&federated, &test_xs, &test_ys),
        federated[0],
        federated[1],
        federated[2],
        federated[3],
    );

    Ok(())
}
