//! Three clients you can run without writing one.
//!
//! Enough to exercise a federation end to end when the point is the
//! federation rather than the model: a smoke test, a demonstration, a
//! first look. Every one is **a demo model, not your model**, and each
//! carries the sentence that should be said out loud whenever it runs —
//! see [`DemoModel::caveat`].
//!
//! All three are deliberately dependency-free: no `burn`, no `rand`, no
//! linear-algebra crate. `cflux` compiles them into the static binary the
//! installer ships, and a model that dragged a deep-learning framework in
//! behind it would make that binary something nobody wants to download.
//! That constraint is why there is no MLP here and why there should not
//! be one — a model with hidden layers wants
//! [Burn](https://github.com/tracel-ai/burn), which is what
//! `conflux-client`'s `burn_mlp` example is for.
//!
//! Two of the three are arranged so that **no single client can solve the
//! problem alone**: each client's data holds one feature nearly constant,
//! and a coefficient you never vary is a coefficient you cannot learn.
//! Without that, a demo proves only that the loop ran.

use std::sync::{Arc, Mutex};

use crate::{ClientApp, TrainResult};

/// How a demo model scores a global model, on data no client trained on.
#[derive(Debug, Clone, Copy)]
pub struct DemoScore {
    /// What the number is — `"test MSE"`, `"held-out accuracy"`.
    pub label: &'static str,
    /// The number.
    pub value: f32,
    /// Which direction is better, so a caller can say "improved"
    /// without knowing the metric.
    pub lower_is_better: bool,
}

/// Builds the demo client for one participant.
///
/// `usize` is the participant index, which is also its data shard.
/// `Arc<Mutex<Vec<f32>>>` is where it writes every global model it is
/// handed — see [`DemoModel::make`].
pub type MakeDemoClient = fn(usize, Arc<Mutex<Vec<f32>>>) -> Box<dyn ClientApp + Send>;

/// A demo client, and everything a caller needs to run and describe one.
pub struct DemoModel {
    /// The name a manifest or a `--model` flag uses.
    pub name: &'static str,
    /// How many `f32` the model is, which is what the server's
    /// placeholder initialization has to match.
    pub weights_dim: usize,
    /// One line on what it does.
    pub headline: &'static str,
    /// What has to be said out loud whenever this runs. Not optional
    /// text: `stub` does not learn, and a demo that let someone believe
    /// otherwise would be worse than no demo.
    pub caveat: &'static str,
    /// Builds the client for participant `i`, which is also its data
    /// shard.
    ///
    /// Every client writes the global model it is handed into `seen`.
    /// A caller otherwise has no way to look at what was learned: `run`
    /// takes ownership of the apps, and reading the server's final
    /// checkpoint back is not exposed yet.
    pub make: MakeDemoClient,
    /// Scores a global model on data no client trained on.
    ///
    /// `None` for a model with nothing meaningful to score — `stub`
    /// returns fixed weights, and any number computed from them would be
    /// an invitation to read meaning into noise.
    pub score: Option<fn(&[f32]) -> DemoScore>,
}

/// Every demo client, in the order a catalog should list them.
pub const MODELS: &[DemoModel] = &[
    DemoModel {
        name: "stub",
        weights_dim: 4,
        headline: "Fixed weights, no training.",
        caveat: "This does not learn. It exercises the transport — registration, \
                 task fetch, chunked submission, quorum, aggregation, checkpoint — \
                 and nothing else. Any accuracy you compute from it is noise.",
        make: |_, seen| Box::new(Stub { seen }),
        score: None,
    },
    DemoModel {
        name: "linreg",
        weights_dim: 4,
        headline: "Least squares over three features. Recovers the model exactly.",
        caveat: "A demo model, not yours. Each client's data holds a different \
                 feature nearly constant, so no client can learn the model alone — \
                 which is the property being demonstrated, not a property of your \
                 data.",
        make: |i, seen| Box::new(LinReg::for_client(i, seen)),
        score: Some(linreg_score),
    },
    DemoModel {
        name: "logreg",
        weights_dim: 4,
        headline: "Binary logistic regression over three features.",
        caveat: "A demo model, not yours. Same non-IID arrangement as `linreg`: \
                 each client is blind to one coefficient, so the federation \
                 separates the classes and no single client does.",
        make: |i, seen| Box::new(LogReg::for_client(i, seen)),
        score: Some(logreg_score),
    },
];

/// The demo client called `name`, if there is one.
pub fn lookup(name: &str) -> Option<&'static DemoModel> {
    MODELS.iter().find(|m| m.name == name)
}

/// Every demo client's name, for an error message that lists what exists.
pub fn names() -> Vec<&'static str> {
    MODELS.iter().map(|m| m.name).collect()
}

/// A small deterministic PRNG, so two runs produce the same numbers.
///
/// `rand` is not a dependency of this crate and is not worth making one
/// for a demo's data generator.
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

const SAMPLES: usize = 120;
const LOCAL_STEPS: usize = 60;
const FEATURES: usize = 3;

/// One client's features, with `frozen` held nearly constant.
///
/// Nearly, not exactly: a column of identical values is a degenerate case
/// that an implementation could special-case away. Nearly constant is the
/// realistic version — the signal is there and is too small to learn
/// from.
fn features(frozen: usize, seed: u64) -> Vec<[f32; FEATURES]> {
    let mut rng = Lcg::new(seed);
    (0..SAMPLES)
        .map(|_| {
            let mut x = [rng.next() * 2.0, rng.next() * 2.0, rng.next() * 2.0];
            x[frozen] = 1.0 + rng.next() * 0.02;
            x
        })
        .collect()
}

/// Returns fixed weights, exactly like `stub_client.py`.
///
/// The Rust counterpart to the Python stub, and here for the same reason:
/// something has to be able to prove the pipeline moves bytes without
/// anybody installing an ML stack first.
struct Stub {
    seen: Arc<Mutex<Vec<f32>>>,
}

impl ClientApp for Stub {
    fn train(&mut self, weights: &[f32], _round: u64) -> TrainResult {
        *self.seen.lock().expect("mutex poisoned") = weights.to_vec();
        TrainResult::new(vec![0.5; weights.len()], 1)
    }
}

/// The true model both regressions are trying to recover.
const TRUE_W: [f32; FEATURES] = [1.5, -2.0, 0.5];
const TRUE_B: f32 = 3.0;

struct LinReg {
    xs: Vec<[f32; FEATURES]>,
    ys: Vec<f32>,
    seen: Arc<Mutex<Vec<f32>>>,
}

impl LinReg {
    fn for_client(index: usize, seen: Arc<Mutex<Vec<f32>>>) -> Self {
        let xs = features(index % FEATURES, 1_000 + index as u64);
        let mut rng = Lcg::new(7_000 + index as u64);
        let ys = xs
            .iter()
            .map(|x| {
                TRUE_W[0] * x[0] + TRUE_W[1] * x[1] + TRUE_W[2] * x[2] + TRUE_B + rng.next() * 0.1
            })
            .collect();
        Self { xs, ys, seen }
    }
}

impl ClientApp for LinReg {
    fn train(&mut self, weights: &[f32], _round: u64) -> TrainResult {
        *self.seen.lock().expect("mutex poisoned") = weights.to_vec();
        // Round one arrives as the server's placeholder zeros, which for
        // a linear model is a perfectly good shared starting point — and
        // *shared* is what matters, since averaging two models that
        // started from different places averages nothing meaningful.
        let mut p = weights.to_vec();
        let n = self.xs.len() as f32;
        for _ in 0..LOCAL_STEPS {
            let mut grad = [0.0f32; 4];
            for (x, y) in self.xs.iter().zip(&self.ys) {
                let error = p[0] * x[0] + p[1] * x[1] + p[2] * x[2] + p[3] - y;
                for (j, xj) in x.iter().enumerate() {
                    grad[j] += 2.0 * error * xj;
                }
                grad[3] += 2.0 * error;
            }
            for (w, g) in p.iter_mut().zip(grad) {
                *w -= 0.05 * g / n;
            }
        }
        TrainResult::new(p, self.xs.len() as u64).with_local_steps(LOCAL_STEPS as u32)
    }
}

struct LogReg {
    xs: Vec<[f32; FEATURES]>,
    ys: Vec<f32>,
    seen: Arc<Mutex<Vec<f32>>>,
}

impl LogReg {
    fn for_client(index: usize, seen: Arc<Mutex<Vec<f32>>>) -> Self {
        let xs = features(index % FEATURES, 2_000 + index as u64);
        let ys = xs
            .iter()
            .map(|x| {
                let z = TRUE_W[0] * x[0] + TRUE_W[1] * x[1] + TRUE_W[2] * x[2] + TRUE_B;
                if z > 3.0 { 1.0 } else { 0.0 }
            })
            .collect();
        Self { xs, ys, seen }
    }
}

/// `σ(z)`, in `f64` because `exp` of a large negative saturates in `f32`
/// and the gradient then vanishes where it should not.
fn sigmoid(z: f32) -> f32 {
    (1.0 / (1.0 + (-(z as f64)).exp())) as f32
}

impl ClientApp for LogReg {
    fn train(&mut self, weights: &[f32], _round: u64) -> TrainResult {
        *self.seen.lock().expect("mutex poisoned") = weights.to_vec();
        let mut p = weights.to_vec();
        let n = self.xs.len() as f32;
        let mut loss = 0.0f32;
        for _ in 0..LOCAL_STEPS {
            let mut grad = [0.0f32; 4];
            loss = 0.0;
            for (x, y) in self.xs.iter().zip(&self.ys) {
                let z = p[0] * x[0] + p[1] * x[1] + p[2] * x[2] + p[3];
                let pred = sigmoid(z);
                let error = pred - y;
                for (j, xj) in x.iter().enumerate() {
                    grad[j] += error * xj;
                }
                grad[3] += error;
                // Clamped so a saturated prediction gives a finite loss
                // rather than an infinity the server would reject.
                loss -= y * pred.max(1e-7).ln() + (1.0 - y) * (1.0 - pred).max(1e-7).ln();
            }
            for (w, g) in p.iter_mut().zip(grad) {
                *w -= 0.5 * g / n;
            }
        }
        TrainResult::new(p, self.xs.len() as u64)
            .with_local_steps(LOCAL_STEPS as u32)
            .with_local_loss(loss / n)
    }
}

/// Features drawn with **every** coordinate varying — the only fair way
/// to ask whether a model generalizes past the slice its owner saw.
fn held_out() -> Vec<[f32; FEATURES]> {
    let mut rng = Lcg::new(9_999);
    (0..400)
        .map(|_| [rng.next() * 2.0, rng.next() * 2.0, rng.next() * 2.0])
        .collect()
}

fn linreg_score(params: &[f32]) -> DemoScore {
    let xs = held_out();
    let total: f32 = xs
        .iter()
        .map(|x| {
            let truth = TRUE_W[0] * x[0] + TRUE_W[1] * x[1] + TRUE_W[2] * x[2] + TRUE_B;
            let pred = params[0] * x[0] + params[1] * x[1] + params[2] * x[2] + params[3];
            (pred - truth) * (pred - truth)
        })
        .sum();
    DemoScore {
        label: "held-out MSE",
        value: total / xs.len() as f32,
        lower_is_better: true,
    }
}

fn logreg_score(params: &[f32]) -> DemoScore {
    let xs = held_out();
    let correct = xs
        .iter()
        .filter(|x| {
            let truth = TRUE_W[0] * x[0] + TRUE_W[1] * x[1] + TRUE_W[2] * x[2] + TRUE_B > 3.0;
            let pred = params[0] * x[0] + params[1] * x[1] + params[2] * x[2] + params[3] > 0.0;
            truth == pred
        })
        .count();
    DemoScore {
        label: "held-out accuracy",
        value: correct as f32 / xs.len() as f32,
        lower_is_better: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_model_is_reachable_by_name() {
        for model in MODELS {
            assert_eq!(lookup(model.name).map(|m| m.name), Some(model.name));
        }
        assert!(lookup("no_such_model").is_none());
    }

    #[test]
    fn every_model_carries_a_caveat() {
        for model in MODELS {
            assert!(
                !model.caveat.is_empty(),
                "{} must say what it is not",
                model.name
            );
        }
    }

    /// Federated averaging over the demo shards must actually converge,
    /// or the demo demonstrates the opposite of what it claims.
    #[test]
    fn linreg_converges_when_averaged_and_no_client_gets_there_alone() {
        let clients = 3;
        let mut global = vec![0.0f32; 4];
        let mut apps: Vec<_> = (0..clients)
            .map(|i| LinReg::for_client(i, Arc::new(Mutex::new(Vec::new()))))
            .collect();

        for _ in 0..40 {
            let updates: Vec<Vec<f32>> = apps
                .iter_mut()
                .map(|a| a.train(&global, 1).weights)
                .collect();
            for j in 0..4 {
                global[j] = updates.iter().map(|u| u[j]).sum::<f32>() / clients as f32;
            }
        }

        let expected = [TRUE_W[0], TRUE_W[1], TRUE_W[2], TRUE_B];
        for (got, want) in global.iter().zip(expected) {
            assert!(
                (got - want).abs() < 0.05,
                "federated fit should recover {expected:?}, got {global:?}"
            );
        }

        // And alone, a client is blind to the coefficient it never
        // varies — the property the caveat claims.
        let mut solo = LinReg::for_client(0, Arc::new(Mutex::new(Vec::new())));
        let mut alone = vec![0.0f32; 4];
        for _ in 0..40 {
            alone = solo.train(&alone, 1).weights;
        }
        assert!(
            (alone[0] - TRUE_W[0]).abs() > 0.2,
            "client 0 freezes x0 and should not recover its coefficient: {alone:?}"
        );
    }
}
