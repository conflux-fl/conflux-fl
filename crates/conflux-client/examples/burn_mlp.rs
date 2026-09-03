//! A real **Burn** MLP `ClientApp` — the deep-learning-in-Rust evaluation
//! the `logreg` spike deliberately isolated itself from — now driving the
//! **real** cited `conflux-core` aggregators, so it reproduces FedAvg AND
//! the Byzantine-robust papers (Krum, Trimmed Mean, …) with Burn clients.
//!
//! Run:
//!   # FedAvg (default): local-only ~0.67 vs federated ~0.99
//!   cargo run --example burn_mlp -p conflux-client --features burn --release
//!
//!   # A robust-aggregation baseline: 5 Burn clients, 1 poisoned attacker.
//!   # fedavg collapses; krum / trimmed_mean defend.
//!   cargo run --example burn_mlp -p conflux-client --features burn --release -- \
//!       --aggregator krum --clients 5 --attackers 1
//!
//! # What this proves
//!
//! `logreg` proved the *architecture* (a Rust `ClientApp` closes the loop,
//! no Python) with a hand-rolled gradient. This proves the *ML stack*: a
//! model **with a hidden layer**, needing real autodiff, trained by Burn
//! (`ndarray` CPU backend — no GPU/CUDA/LibTorch), plugged into the same
//! `ClientApp` trait and the same opaque flat-`f32` wire contract.
//!
//! # What it deliberately does NOT re-test
//!
//! The gRPC transport (`logreg` already ran Rust clients over real
//! `conflux-net` gRPC with zero server changes), and the aggregation math
//! (it calls `conflux-core`'s actual `build_aggregator`, the cited
//! implementations the server uses). This harness drives `ClientApp::train`
//! directly and feeds the results to the real aggregator in-process,
//! isolating the one new question: does a **Burn** client train correctly
//! against the flat-weight contract, under FedAvg and under attack?
//!
//! The problem is the LR spike's, unchanged: a non-IID split where no
//! single client can solve it alone (each sees one informative feature),
//! scored on the global problem.

use burn::backend::{Autodiff, NdArray};
use burn::module::{Module, Param};
use burn::nn::{Linear, LinearConfig};
use burn::optim::{GradientsParams, Optimizer, SgdConfig};
use burn::tensor::backend::Backend;
use burn::tensor::{Tensor, TensorData, activation};

use conflux_client::{ClientApp, TrainResult, is_placeholder_init};
// The REAL cited aggregators. The Burn client feeds them exactly as the
// server would, so a robust baseline reproduces through the actual
// `conflux-core` Krum/Trimmed-Mean — never a re-implementation.
use conflux_core::{AggregatorParams, build_aggregator};
use conflux_proto::{ClientDelta, encode_weights};

// The training backend: NdArray (pure-Rust CPU) wrapped in Autodiff so
// `.backward()` works. `Dev` is its device handle (CPU).
type AB = Autodiff<NdArray>;
type Dev = <AB as Backend>::Device;

const DIM: usize = 4; // the problem's feature count (true w = [1,1,1,1])
const HIDDEN: usize = 16; // the hidden layer LR does not have

// ---------------------------------------------------------------------------
// The model — a 2-layer MLP. `#[derive(Module)]` is Burn's equivalent of a
// PyTorch `nn.Module`: it makes the struct's `Param` fields discoverable by
// the optimizer and autodiff.
// ---------------------------------------------------------------------------

#[derive(Module, Debug)]
struct Mlp<B: Backend> {
    fc1: Linear<B>,
    fc2: Linear<B>,
}

impl<B: Backend> Mlp<B> {
    fn build(device: &B::Device) -> Self {
        Self {
            fc1: LinearConfig::new(DIM, HIDDEN).init(device),
            fc2: LinearConfig::new(HIDDEN, 1).init(device),
        }
    }

    fn forward(&self, x: Tensor<B, 2>) -> Tensor<B, 2> {
        let x = activation::relu(self.fc1.forward(x));
        self.fc2.forward(x) // logits; sigmoid is applied in the loss/eval
    }
}

// ---------------------------------------------------------------------------
// The flat-weight bridge — the crux of "Burn plugs into the opaque `&[f32]`
// contract". `flatten` and `load` traverse the same params in the same order,
// so the round-trip (Burn params -> flat f32 -> aggregated -> flat f32 -> Burn
// params) is self-consistent regardless of Burn's internal tensor layout.
// ---------------------------------------------------------------------------

fn to_vec<const D: usize>(t: Tensor<AB, D>) -> Vec<f32> {
    t.into_data().to_vec::<f32>().expect("f32 tensor")
}

fn flatten(m: &Mlp<AB>) -> Vec<f32> {
    let mut v = Vec::new();
    v.extend(to_vec(m.fc1.weight.val()));
    v.extend(to_vec(m.fc1.bias.as_ref().unwrap().val()));
    v.extend(to_vec(m.fc2.weight.val()));
    v.extend(to_vec(m.fc2.bias.as_ref().unwrap().val()));
    v
}

fn load(m: &mut Mlp<AB>, flat: &[f32], device: &Dev) {
    let (w1, rest) = flat.split_at(DIM * HIDDEN);
    let (b1, rest) = rest.split_at(HIDDEN);
    let (w2, rest) = rest.split_at(HIDDEN);
    let (b2, _) = rest.split_at(1);
    m.fc1.weight = Param::from_tensor(Tensor::from_data(
        TensorData::new(w1.to_vec(), [DIM, HIDDEN]),
        device,
    ));
    m.fc1.bias = Some(Param::from_tensor(Tensor::from_data(
        TensorData::new(b1.to_vec(), [HIDDEN]),
        device,
    )));
    m.fc2.weight = Param::from_tensor(Tensor::from_data(
        TensorData::new(w2.to_vec(), [HIDDEN, 1]),
        device,
    ));
    m.fc2.bias = Some(Param::from_tensor(Tensor::from_data(
        TensorData::new(b2.to_vec(), [1]),
        device,
    )));
}

// ---------------------------------------------------------------------------
// Loss + metrics
// ---------------------------------------------------------------------------

/// Binary cross-entropy from logits, clamped away from 0/1 before the log.
fn bce(logits: Tensor<AB, 2>, y: Tensor<AB, 2>) -> Tensor<AB, 1> {
    let p = activation::sigmoid(logits).clamp(1e-7, 1.0 - 1e-7);
    let one_minus_p = p.clone().mul_scalar(-1.0).add_scalar(1.0);
    let one_minus_y = y.clone().mul_scalar(-1.0).add_scalar(1.0);
    let terms = y.mul(p.log()).add(one_minus_y.mul(one_minus_p.log()));
    terms.mean().mul_scalar(-1.0)
}

fn matrix(xs: &[Vec<f32>], device: &Dev) -> Tensor<AB, 2> {
    let n = xs.len();
    let flat: Vec<f32> = xs.iter().flatten().copied().collect();
    Tensor::from_data(TensorData::new(flat, [n, DIM]), device)
}

fn column(ys: &[f32], device: &Dev) -> Tensor<AB, 2> {
    Tensor::from_data(TensorData::new(ys.to_vec(), [ys.len(), 1]), device)
}

fn accuracy(flat: &[f32], xs: &[Vec<f32>], ys: &[f32], device: &Dev) -> f32 {
    let mut m = Mlp::<AB>::build(device);
    load(&mut m, flat, device);
    let probs = to_vec(activation::sigmoid(m.forward(matrix(xs, device))));
    let correct = probs
        .iter()
        .zip(ys)
        .filter(|(p, y)| (((**p >= 0.5) as u8 as f32) - **y).abs() < 0.5)
        .count();
    correct as f32 / xs.len().max(1) as f32
}

// ---------------------------------------------------------------------------
// The client — a real `impl ClientApp`, identical trait to `logreg`'s.
// ---------------------------------------------------------------------------

struct BurnMlpClient {
    xs: Vec<Vec<f32>>,
    ys: Vec<f32>,
    steps: usize,
    lr: f64,
    device: Dev,
    /// A Byzantine client submits a large-offset update instead of
    /// training — the same attack the Python harness's `--poison` uses.
    poison: bool,
}

impl ClientApp for BurnMlpClient {
    fn train(&mut self, weights: &[f32], _round: u64) -> TrainResult {
        if self.poison {
            let offset: Vec<f32> = weights.iter().map(|w| w + 20.0).collect();
            return TrainResult::new(offset, self.ys.len() as u64);
        }

        let mut model = Mlp::<AB>::build(&self.device);
        // The server's all-zero placeholder would break ReLU symmetry
        // (see `is_placeholder_init`), so on the first round keep this
        // client's own random init instead of loading zeros.
        if !is_placeholder_init(weights) {
            load(&mut model, weights, &self.device);
        }

        let x = matrix(&self.xs, &self.device);
        let y = column(&self.ys, &self.device);

        let loss_before: f32 = bce(model.forward(x.clone()), y.clone()).into_scalar();

        let mut optim = SgdConfig::new().init();
        for _ in 0..self.steps {
            let loss = bce(model.forward(x.clone()), y.clone());
            let grads = loss.backward();
            let grads = GradientsParams::from_grads(grads, &model);
            model = optim.step(self.lr, model, grads);
        }

        TrainResult::new(flatten(&model), self.ys.len() as u64)
            .with_local_steps(self.steps as u32) // FedNova
            .with_local_loss(loss_before) // q-FedAvg
    }
}

// ---------------------------------------------------------------------------
// Aggregation — the REAL cited `conflux-core` implementations, fed the same
// way the server feeds them. No aggregation math is re-implemented here.
// ---------------------------------------------------------------------------

fn aggregate(name: &str, results: &[TrainResult], byzantine_fraction: f32, round: u64) -> Vec<f32> {
    let agg = build_aggregator(
        name,
        AggregatorParams {
            byzantine_fraction,
            ..Default::default()
        },
    )
    .unwrap_or_else(|e| {
        eprintln!("build_aggregator('{name}') failed: {e:?}");
        std::process::exit(1);
    });
    let batch: Vec<ClientDelta> = results
        .iter()
        .enumerate()
        .map(|(i, r)| ClientDelta {
            client_id: format!("c{i}"),
            round,
            weights: encode_weights(&r.weights),
            num_samples: r.num_samples,
            ..Default::default()
        })
        .collect();
    agg.aggregate(&batch).unwrap_or_else(|e| {
        eprintln!("aggregate failed: {e:?}");
        std::process::exit(1);
    })
}

// ---------------------------------------------------------------------------
// Data — the LR spike's non-IID problem, verbatim in intent.
// ---------------------------------------------------------------------------

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

/// Client `i` only sees feature `i % DIM` varying — locally a one-feature
/// problem, so no client can learn the global `sum(x) > 0` rule alone.
fn shard(client_index: u64, n: usize) -> (Vec<Vec<f32>>, Vec<f32>) {
    let mut rng = Lcg::new(client_index.wrapping_add(1));
    let informative = client_index as usize % DIM;
    let mut xs = Vec::with_capacity(n);
    let mut ys = Vec::with_capacity(n);
    for _ in 0..n {
        let x: Vec<f32> = (0..DIM)
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

/// The shared held-out set: the global problem, all features varying.
fn global_test_set(n: usize) -> (Vec<Vec<f32>>, Vec<f32>) {
    let mut rng = Lcg::new(u64::MAX / 3);
    let mut xs = Vec::with_capacity(n);
    let mut ys = Vec::with_capacity(n);
    for _ in 0..n {
        let x: Vec<f32> = (0..DIM).map(|_| (rng.next() - 0.5) * 2.0).collect();
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

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

struct Args {
    aggregator: String,
    attackers: usize,
    clients: usize,
    rounds: usize,
    steps: usize,
}

fn parse_args() -> Args {
    let mut a = Args {
        aggregator: "fedavg".into(),
        attackers: 0,
        clients: 5,
        rounds: 8,
        steps: 40,
    };
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < argv.len() {
        let next = |i: &mut usize| -> String {
            *i += 1;
            argv.get(*i).cloned().unwrap_or_else(|| {
                eprintln!("missing value for {}", argv[*i - 1]);
                std::process::exit(1);
            })
        };
        match argv[i].as_str() {
            "--aggregator" => a.aggregator = next(&mut i),
            "--attackers" => a.attackers = next(&mut i).parse().expect("int"),
            "--clients" => a.clients = next(&mut i).parse().expect("int"),
            "--rounds" => a.rounds = next(&mut i).parse().expect("int"),
            "--steps" => a.steps = next(&mut i).parse().expect("int"),
            other => {
                eprintln!("unknown arg: {other}");
                std::process::exit(1);
            }
        }
        i += 1;
    }
    a
}

fn main() {
    let args = parse_args();
    let lr = 0.5;
    let byz = args.attackers as f32 / args.clients.max(1) as f32;

    let device: Dev = Default::default();
    AB::seed(0); // reproducible init

    let (test_xs, test_ys) = global_test_set(400);
    // One shared initialization every client starts from — flat, so it
    // travels the same wire the federation uses.
    let init = flatten(&Mlp::<AB>::build(&device));

    println!(
        "config: aggregator={} clients={} attackers={} rounds={} steps={}",
        args.aggregator, args.clients, args.attackers, args.rounds, args.steps
    );

    // The last `attackers` clients are poisoned (mirrors run_demo.sh).
    let mut clients: Vec<BurnMlpClient> = (0..args.clients)
        .map(|idx| {
            let (xs, ys) = shard(idx as u64, 200);
            let poison = idx >= args.clients - args.attackers;
            BurnMlpClient {
                xs,
                ys,
                steps: args.steps,
                lr,
                device,
                poison,
            }
        })
        .collect();

    // Local-only baseline (honest clients): each trains ONLY on its own
    // shard for the same total steps, then is scored on the global set it
    // has never represented.
    println!("\n=== local-only (no federation) ===");
    for (i, c) in clients.iter_mut().enumerate() {
        if c.poison {
            continue;
        }
        let mut w = init.clone();
        for r in 0..args.rounds as u64 {
            w = c.train(&w, r).weights;
        }
        println!(
            "  rc-{i} (feature {}): local-only {:.3}",
            i % DIM,
            accuracy(&w, &test_xs, &test_ys, &device)
        );
    }

    // Federated, through the REAL aggregator.
    println!(
        "\n=== federated ({}, {} Burn clients, {} poisoned, {} rounds) ===",
        args.aggregator, args.clients, args.attackers, args.rounds
    );
    let mut global = init.clone();
    for r in 0..args.rounds as u64 {
        let results: Vec<TrainResult> = clients.iter_mut().map(|c| c.train(&global, r)).collect();
        global = aggregate(&args.aggregator, &results, byz, r);
        println!(
            "  round {r}: global acc = {:.3}",
            accuracy(&global, &test_xs, &test_ys, &device)
        );
    }

    let final_acc = accuracy(&global, &test_xs, &test_ys, &device);
    println!(
        "\n=== federated {} final: {final_acc:.3} on the global test set ===",
        args.aggregator
    );
    // Machine-parseable line: the baselines runner greps `held_out_accuracy=`
    // for BOTH edges (the Python eval client prints the same token).
    println!("RESULT held_out_accuracy={final_acc:.4}");
    if args.attackers > 0 {
        println!(
            "{} of {} clients poisoned — a robust aggregator should hold here where fedavg collapses.",
            args.attackers, args.clients
        );
    } else {
        println!(
            "local-only ~0.67 vs federated {final_acc:.3} — the gap is the evidence a Burn client federates."
        );
    }
}
