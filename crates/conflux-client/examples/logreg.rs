//! A complete Rust `ClientApp`: logistic regression, no Python anywhere.
//!
//! Run with:
//!   cargo run --example logreg -p conflux-client -- --client-id c1 --client-index 0
//!
//! It mirrors
//! `python/conflux_client/examples/e2e_numpy_logreg` — same model, same
//! flat `[w_1..w_d, bias]` layout, same full-batch gradient descent — so
//! the two can be compared rather than argued about.
//!
//! # What it demonstrates
//!
//! The client loop closes with no Python process, no local gRPC hop to
//! one, and no interpreter to install: receive flat `f32`, train, return
//! flat `f32`. It also reports `local_steps` and `local_loss`, so a Rust
//! client drives FedNova and q-FedAvg exactly as the Python one does.
//!
//! The data is arranged so **no single client can solve the problem
//! alone** — see [`shard`]. Without that, a demo proves only that the
//! loop ran.
//!
//! # What it does not demonstrate
//!
//! Anything about deep learning in Rust. Logistic regression needs no
//! framework — the gradient is a few lines — which is exactly why it is
//! the right spike: it isolates *the architecture* from *the ML stack*.
//! A model with hidden layers wants [Burn](https://github.com/tracel-ai/burn)
//! or equivalent — see the `burn_mlp` example.

use conflux_client::{ClientApp, RunConfig, TrainResult, is_placeholder_init, run};

/// A small deterministic PRNG. `rand` is not a dependency of this crate
/// and is not worth making one for a demo's data generator.
struct Lcg(u64);

impl Lcg {
    fn new(seed: u64) -> Self {
        Self(seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1))
    }

    fn next(&mut self) -> f32 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((self.0 >> 33) as f32) / ((1u64 << 31) as f32)
    }
}

/// `σ(z)`, in `f64` because `exp` of a large negative saturates and the
/// loss below takes a logarithm of the result.
fn sigmoid(z: f64) -> f64 {
    1.0 / (1.0 + (-z).exp())
}

/// `p(y=1 | x)`. `weights` is `[w_1..w_d, bias]` — already flat, so no
/// unflattening step, unlike a real `nn.Module`.
fn predict(weights: &[f32], x: &[f32]) -> f64 {
    let bias = *weights.last().expect("weights are never empty") as f64;
    let z: f64 = weights[..weights.len() - 1]
        .iter()
        .zip(x)
        .map(|(w, xi)| *w as f64 * *xi as f64)
        .sum();
    sigmoid(z + bias)
}

/// Mean binary cross-entropy, clamped away from 0 and 1 before the log —
/// which is what stops a confident-and-wrong prediction reaching
/// infinity.
fn loss(weights: &[f32], xs: &[Vec<f32>], ys: &[f32]) -> f32 {
    if xs.is_empty() {
        return 0.0;
    }
    let total: f64 = xs
        .iter()
        .zip(ys)
        .map(|(x, y)| {
            let p = predict(weights, x).clamp(1e-7, 1.0 - 1e-7);
            -(*y as f64 * p.ln() + (1.0 - *y as f64) * (1.0 - p).ln())
        })
        .sum();
    (total / xs.len() as f64) as f32
}

/// Full-batch gradient descent. The caller's weights are never mutated —
/// the same contract the NumPy version's `.copy()` provides.
/// `correction`, when present, is SCAFFOLD's `c − c_i`, added to the
/// gradient each step so the update follows `g − c_i + c` — the same
/// contract the Python harness's `train_steps(..., correction=)`
/// implements, field for field, flat layout and all.
fn train_steps(
    weights: &[f32],
    xs: &[Vec<f32>],
    ys: &[f32],
    lr: f32,
    steps: usize,
    correction: Option<&[f32]>,
) -> Vec<f32> {
    let dim = weights.len() - 1;
    let mut w: Vec<f64> = weights[..dim].iter().map(|v| *v as f64).collect();
    let mut b = weights[dim] as f64;
    let n = xs.len().max(1) as f64;

    for _ in 0..steps {
        let mut grad_w = vec![0.0f64; dim];
        let mut grad_b = 0.0f64;

        for (x, y) in xs.iter().zip(ys) {
            let z: f64 = w.iter().zip(x).map(|(wi, xi)| wi * *xi as f64).sum();
            let error = sigmoid(z + b) - *y as f64;
            for (g, xi) in grad_w.iter_mut().zip(x) {
                *g += error * *xi as f64 / n;
            }
            grad_b += error / n;
        }

        if let Some(corr) = correction {
            // Applied to the *gradient*, not the loss — the correction
            // is not the gradient of anything. Last entry corrects the
            // bias, matching the flat `[w_1..w_d, bias]` layout.
            for (g, c) in grad_w.iter_mut().zip(corr) {
                *g += *c as f64;
            }
            grad_b += corr[dim] as f64;
        }

        for (wi, g) in w.iter_mut().zip(&grad_w) {
            *wi -= lr as f64 * g;
        }
        b -= lr as f64 * grad_b;
    }

    w.into_iter().chain([b]).map(|v| v as f32).collect()
}

fn accuracy(weights: &[f32], xs: &[Vec<f32>], ys: &[f32]) -> f32 {
    let correct = xs
        .iter()
        .zip(ys)
        .filter(|(x, y)| ((predict(weights, x) >= 0.5) as u8 as f32 - **y).abs() < 0.5)
        .count();
    correct as f32 / xs.len().max(1) as f32
}

/// A problem **no single client can solve alone**, which is the point.
///
/// The true model is `w = [1, 1, 1, 1]`, `b = 0`, so the label is
/// `sum(x) > 0` and every feature matters equally. But client *i* only
/// ever sees data where **feature `i` varies** — the rest sit near zero.
/// Locally that looks like a one-feature problem, so a client trained on
/// it learns `w_i` and nothing about the other three.
///
/// The first version of this example sharded IID, and every client
/// reached 1.000 on its own data *before* federating. That proved the
/// loop ran and nothing else: if local-only already solves the problem,
/// the federated number is not evidence of anything.
fn shard(client_index: u64, n: usize, dim: usize) -> (Vec<Vec<f32>>, Vec<f32>) {
    let mut rng = Lcg::new(client_index.wrapping_add(1));
    let informative = (client_index as usize) % dim;

    let mut xs = Vec::with_capacity(n);
    let mut ys = Vec::with_capacity(n);
    for _ in 0..n {
        let x: Vec<f32> = (0..dim)
            .map(|j| {
                if j == informative {
                    (rng.next() - 0.5) * 2.0
                } else {
                    (rng.next() - 0.5) * 0.02
                }
            })
            .collect();
        let label = if x.iter().sum::<f32>() > 0.0 {
            1.0
        } else {
            0.0
        };
        xs.push(x);
        ys.push(label);
    }
    (xs, ys)
}

/// The held-out set every client is scored against: the *global*
/// problem, where all features vary. Deterministic, so every client and
/// every run sees exactly the same test set.
fn global_test_set(n: usize, dim: usize) -> (Vec<Vec<f32>>, Vec<f32>) {
    let mut rng = Lcg::new(u64::MAX / 3);
    let mut xs = Vec::with_capacity(n);
    let mut ys = Vec::with_capacity(n);
    for _ in 0..n {
        let x: Vec<f32> = (0..dim).map(|_| (rng.next() - 0.5) * 2.0).collect();
        let label = if x.iter().sum::<f32>() > 0.0 {
            1.0
        } else {
            0.0
        };
        xs.push(x);
        ys.push(label);
    }
    (xs, ys)
}

struct LogRegClient {
    xs: Vec<Vec<f32>>,
    ys: Vec<f32>,
    /// The shared held-out set — the global problem, which this client's
    /// own shard cannot represent.
    test_xs: Vec<Vec<f32>>,
    test_ys: Vec<f32>,
    lr: f32,
    steps: usize,
    /// SCAFFOLD's client half, mirroring the Python harness: `c_i` is
    /// THIS client's control variate (persisted across rounds, zeros to
    /// start — the paper's own initialization), `c` is the server's,
    /// delivered before each round via `on_control_variate`.
    scaffold: bool,
    c_i: Option<Vec<f32>>,
    c: Option<Vec<f32>>,
    announced_c: bool,
}

impl ClientApp for LogRegClient {
    fn on_control_variate(&mut self, c: &[f32]) {
        // Say so, out loud — once. A SCAFFOLD run where `c` never
        // arrives is indistinguishable from a correct one by accuracy
        // alone: the correction silently becomes `-c_i`, which
        // *increases* variance. Same announcement, same reason, as the
        // Python harness.
        if !self.announced_c && c.iter().any(|v| *v != 0.0) {
            let norm = c.iter().map(|v| (*v as f64).powi(2)).sum::<f64>().sqrt();
            println!("SCAFFOLD: first nonzero c received (l2 norm {norm:.4})");
            self.announced_c = true;
        }
        self.c = Some(c.to_vec());
    }

    fn train(&mut self, weights: &[f32], round: u64) -> TrainResult {
        // The accuracy of the *incoming global model* on the shared test
        // set. This is the number that says whether federation works: it
        // should climb across rounds even though no client ever sees
        // another's data.
        if !is_placeholder_init(weights) {
            println!(
                "  round {round}: global model scores {:.3} on the shared test set",
                accuracy(weights, &self.test_xs, &self.test_ys)
            );
        }

        // The all-zero placeholder is fine here, unlike for a network
        // with hidden units: logistic regression has no symmetry to
        // break. Checked anyway, because the next model someone writes
        // will not be this one.
        let start = weights.to_vec();

        // The loss *before* training — which is what q-FedAvg's
        // `F_k(w^t)` means, measured at the round's starting weights.
        let loss_before = loss(&start, &self.xs, &self.ys);

        if !self.scaffold {
            let trained = train_steps(&start, &self.xs, &self.ys, self.lr, self.steps, None);
            return TrainResult::new(trained, self.ys.len() as u64)
                .with_local_steps(self.steps as u32)
                .with_local_loss(loss_before);
        }

        // --- SCAFFOLD (Karimireddy et al. 2020, Algorithm 1, option
        // II), exactly the Python harness's arithmetic ---------------
        let dim = start.len();
        let c_i = self.c_i.get_or_insert_with(|| vec![0.0; dim]);
        let zeros = vec![0.0; dim];
        let c = self.c.as_deref().unwrap_or(&zeros);

        // Local steps follow g − c_i + c, so the per-parameter
        // correction is (c − c_i). Both zero at initialization: round
        // one is plain local training, by the paper's own design.
        let correction: Vec<f32> = c.iter().zip(c_i.iter()).map(|(cv, ci)| cv - ci).collect();
        let trained = train_steps(
            &start,
            &self.xs,
            &self.ys,
            self.lr,
            self.steps,
            Some(&correction),
        );

        // Option II: c_i+ = c_i − c + (x − y)/(K·lr), so the *delta*
        // this client reports is Δc_i = (x − y)/(K·lr) − c. The server
        // folds it in damped by 1/N; c_i advances locally so next
        // round's correction uses this round's evidence.
        let scale = 1.0 / (self.steps as f32 * self.lr);
        let delta_c: Vec<f32> = start
            .iter()
            .zip(&trained)
            .zip(c)
            .map(|((x, y), cv)| (x - y) * scale - cv)
            .collect();
        for (ci, d) in c_i.iter_mut().zip(&delta_c) {
            *ci += d;
        }

        TrainResult::new(trained, self.ys.len() as u64)
            .with_local_steps(self.steps as u32)
            .with_local_loss(loss_before)
            .with_control_variate(delta_c)
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt().with_target(false).init();

    let mut args = std::env::args().skip(1).peekable();
    let (mut client_id, mut rounds, mut address, mut index, mut scaffold) = (
        "rust-client-1".to_string(),
        8usize,
        "http://127.0.0.1:47100".to_string(),
        0u64,
        false,
    );
    while let Some(flag) = args.next() {
        if flag == "--scaffold" {
            scaffold = true;
            continue;
        }
        let value = args.next().unwrap_or_default();
        match flag.as_str() {
            "--client-id" => client_id = value,
            "--rounds" => rounds = value.parse()?,
            "--address" => address = value,
            "--client-index" => index = value.parse()?,
            other => return Err(format!("unknown flag {other}").into()),
        }
    }

    let dim = 4;
    let (xs, ys) = shard(index, 300, dim);
    let (test_xs, test_ys) = global_test_set(500, dim);

    // The comparison that makes the federated number mean something:
    // what this client reaches on its own data, scored on the *global*
    // problem. It should be poor — it only ever saw one feature vary.
    let solo = train_steps(&vec![0.0; dim + 1], &xs, &ys, 0.5, 500, None);
    println!(
        "[{client_id}] shard sees feature {} only; local-only model scores {:.3} on the \
         shared test set",
        index as usize % dim,
        accuracy(&solo, &test_xs, &test_ys)
    );

    if scaffold {
        println!("[{client_id}] SCAFFOLD: client-side control variate active");
    }
    let mut app = LogRegClient {
        xs,
        ys,
        test_xs,
        test_ys,
        lr: 0.5,
        steps: 20,
        scaffold,
        c_i: None,
        c: None,
        announced_c: false,
    };

    let completed = run(
        &mut app,
        RunConfig {
            address,
            client_id: client_id.clone(),
            rounds,
            ..Default::default()
        },
    )
    .await?;

    println!("[{client_id}] completed {completed} federated rounds, no Python involved");
    Ok(())
}
