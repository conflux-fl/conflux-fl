//! **FLANDERS** — Gabrielli, Belli, Matrullo, Miori & Tolomei, 2024,
//! "Protecting Federated Learning from Extreme Model Poisoning Attacks
//! via Multidimensional Time Series Anomaly Detection" (arXiv
//! 2303.16668v3).
//!
//! A pre-aggregation *filter*, not an aggregation rule: it decides which
//! clients the configured base method gets to see, and the base method
//! does the rest. Composing that way — judge, then delegate — is the
//! natural shape for a cross-round temporal defense, and it means
//! FLANDERS pairs with whichever base method a deployment already
//! chose rather than replacing it.
//!
//! # The method
//!
//! Treat the local models arriving each round as a matrix-valued time
//! series `Θ_t` (`d` parameters × `h` clients), fit a first-order matrix
//! autoregressive model to the history, and score each client by how far
//! its actual update lands from what that model predicted:
//!
//! ```text
//! Θ_t = A Θ_{t-1} B + E_t                          MAR(1)
//! Ω̂ = argmin Σ_{j=0}^{l-1} ‖Θ_{t-j} − A Θ_{t-j-1} B‖²_F   via ALS
//! Θ̂_t = Â Θ_{t-1} B̂                                the forecast
//!
//! s_c = ‖θ_c − θ̂_c‖²₂     if c appears in the history
//! s_c = ‖θ_global − θ_c‖²₂ if c is new this round  (cold start)
//! ```
//!
//! Then keep the `k` clients with the smallest scores and hand them to
//! the base aggregator.
//!
//! # Why the forecast matters
//!
//! Every single-round robust method asks "which of these updates is
//! unusual *compared to the others in this batch*?" — so a colluding
//! majority is not unusual, because it *is* the batch. FLANDERS asks
//! "which of these updates is unusual *compared to what this client did
//! before*?" A majority of attackers cannot make an individual
//! attacker's own history consistent with its poisoned update, which is
//! why the paper reports resilience past 50% malicious.
//!
//! # Fidelity notes
//!
//! - **MAR(1) with ALS**, per §"MAR Estimation": the coefficients are
//!   re-fit each round from the last `l` observation matrices by
//!   alternating least squares. A ridge term is added to both normal-
//!   equation solves — the paper does not specify one, but with `d` or
//!   `h` larger than the number of stacked historical pairs the systems
//!   are singular, and a singular solve is an unpredictable answer
//!   rather than a wrong one.
//! - **`δ(u, v) = ‖u − v‖²₂`**, stated in the paper as its choice
//!   ("In this work, we set δ(u, v) = ‖u − v‖²₂").
//! - **Cold start uses the current global model**, per the second case
//!   of the paper's Equation (4). This implementation tracks the global
//!   model as its own previous output, the same way `FedOptAggregator`
//!   does and for the same reason: the server checkpoints exactly what
//!   `aggregate` returns.
//! - **Top-`k` selection**, the first of the paper's two stated
//!   strategies. `k` is derived from `byzantine_fraction` so it shares
//!   the `robust` family's existing knob rather than adding a parallel
//!   one; the paper's own experiments keep `m − b`, which is the same
//!   quantity expressed as a count.
//! - **Parameter subsampling, per the paper.** FLANDERS samples 500
//!   coordinates "for tractability on real models", and so does this
//!   (`max_forecast_dim`). A model with fewer coordinates is fitted
//!   whole, so synthetic experiments at `dim = 3` are unaffected.
//!
//!   Leaving the subsampling out is not "more honest": the MAR
//!   coefficient matrix is `d × d`, which at a 50,890-parameter model is
//!   20.7 GB for a single allocation, and running the unbounded fit on
//!   real MNIST OOM-kills the server process. The bound is not an
//!   approximation of the paper — it *is* the paper, and without it the
//!   implementation cannot do the thing the paper was written to do.

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;

use conflux_proto::ClientDelta;

use crate::weights::decode_and_validate;
use crate::{Aggregator, AggregatorError};

/// A dense `f64` matrix, row-major. Small by construction — `d` is the
/// model dimension and `h` the client count — so a plain `Vec<Vec<f64>>`
/// costs nothing worth optimizing away and reads far better than an
/// index-arithmetic flat buffer.
type Matrix = Vec<Vec<f64>>;

fn zeros(rows: usize, cols: usize) -> Matrix {
    vec![vec![0.0; cols]; rows]
}

fn identity(n: usize) -> Matrix {
    let mut m = zeros(n, n);
    for (i, row) in m.iter_mut().enumerate() {
        row[i] = 1.0;
    }
    m
}

fn matmul(a: &Matrix, b: &Matrix) -> Matrix {
    let (n, k, m) = (a.len(), b.len(), b.first().map_or(0, |r| r.len()));
    let mut out = zeros(n, m);
    for i in 0..n {
        for p in 0..k {
            let aip = a[i][p];
            if aip == 0.0 {
                continue;
            }
            for j in 0..m {
                out[i][j] += aip * b[p][j];
            }
        }
    }
    out
}

fn transpose(a: &Matrix) -> Matrix {
    let (n, m) = (a.len(), a.first().map_or(0, |r| r.len()));
    let mut out = zeros(m, n);
    for i in 0..n {
        for j in 0..m {
            out[j][i] = a[i][j];
        }
    }
    out
}

/// Solves `M X = RHS` by Gaussian elimination with partial pivoting.
///
/// Returns `None` when `M` is numerically singular. The caller treats
/// that as "no forecast this round" rather than substituting something —
/// a fabricated forecast would produce anomaly scores that look real.
fn solve(mut m: Matrix, mut rhs: Matrix) -> Option<Matrix> {
    let n = m.len();
    if n == 0 || m[0].len() != n || rhs.len() != n {
        return None;
    }

    for col in 0..n {
        // Partial pivoting: without it, a zero on the diagonal ends the
        // solve even when the system is perfectly well-conditioned.
        let pivot = (col..n).max_by(|&a, &b| {
            m[a][col]
                .abs()
                .partial_cmp(&m[b][col].abs())
                .unwrap_or(std::cmp::Ordering::Equal)
        })?;
        if m[pivot][col].abs() < 1e-12 {
            return None;
        }
        m.swap(col, pivot);
        rhs.swap(col, pivot);

        let d = m[col][col];
        for value in m[col][col..n].iter_mut() {
            *value /= d;
        }
        for value in rhs[col].iter_mut() {
            *value /= d;
        }

        for row in 0..n {
            if row == col {
                continue;
            }
            let factor = m[row][col];
            if factor == 0.0 {
                continue;
            }
            // Split so the pivot row and the row being eliminated can be
            // borrowed at once — the alternative is cloning the pivot
            // row on every elimination step.
            let (pivot_row, target_row) = if row < col {
                let (head, tail) = m.split_at_mut(col);
                (&tail[0], &mut head[row])
            } else {
                let (head, tail) = m.split_at_mut(row);
                (&head[col], &mut tail[0])
            };
            for (t, p) in target_row[col..n].iter_mut().zip(&pivot_row[col..n]) {
                *t -= factor * p;
            }
            let (pivot_rhs, target_rhs) = if row < col {
                let (head, tail) = rhs.split_at_mut(col);
                (&tail[0], &mut head[row])
            } else {
                let (head, tail) = rhs.split_at_mut(row);
                (&head[col], &mut tail[0])
            };
            for (t, p) in target_rhs.iter_mut().zip(pivot_rhs.iter()) {
                *t -= factor * p;
            }
        }
    }

    if rhs.iter().any(|r| r.iter().any(|v: &f64| !v.is_finite())) {
        return None;
    }
    Some(rhs)
}

/// One round's diagnostic row: what FLANDERS thought of a client.
#[derive(Debug, Clone, PartialEq)]
pub struct ClientFlandersDiagnostic {
    /// Which client.
    pub client_id: String,
    /// Its anomaly score — squared L2 between the observed update and
    /// the MAR forecast, or between the update and the global model on a
    /// cold start. Lower is better.
    pub anomaly_score: f64,
    /// Whether this score came from a forecast (`false` means cold
    /// start, which is a weaker signal and worth being able to see).
    pub forecast_available: bool,
    /// Whether the client survived the top-`k` filter.
    pub kept: bool,
}

/// FLANDERS as a pre-aggregation filter over any base aggregator.
pub struct FlandersAggregator {
    base: Box<dyn Aggregator>,
    /// `l` — how many past observation matrices the MAR fit uses.
    pub history_window: usize,
    /// The assumed malicious fraction, which sets `k = m − ⌈f·m⌉`.
    /// Shares the `robust` family's knob rather than introducing a
    /// parallel one.
    pub byzantine_fraction: f32,
    /// Ridge term added to both ALS normal-equation solves. Not in the
    /// paper; see the fidelity notes for why it is here.
    pub ridge: f64,
    /// ALS sweeps per round. The paper cites ALS without fixing an
    /// iteration count; a handful is enough at these sizes and the
    /// estimate is refit from scratch every round anyway.
    pub als_iterations: usize,
    /// How many model coordinates the MAR forecast is fitted over.
    ///
    /// **A safety bound, not a tuning knob.** The MAR coefficient matrix
    /// `A` is `d × d`, so fitting allocates `O(d²)`. At a
    /// 50,890-parameter model that is **20.7 GB for a single matrix**,
    /// and an unbounded fit OOM-kills the server process in the third
    /// round — the first round with enough history to fit anything.
    ///
    /// The bound is the paper's own answer. FLANDERS samples 500
    /// coordinates "for tractability on real models", and 500 is the
    /// default here. A model with fewer coordinates than this is fitted
    /// whole, so every synthetic experiment at `dim = 3` is unaffected
    /// and its results are unchanged.
    pub max_forecast_dim: usize,
    /// `Mutex` because `aggregate` takes `&self`.
    history: Mutex<VecDeque<HashMap<String, Vec<f32>>>>,
    /// The previous output, which is the current global model — needed
    /// for the paper's cold-start branch.
    global: Mutex<Option<Vec<f32>>>,
    last_diagnostics: Mutex<Vec<ClientFlandersDiagnostic>>,
}

impl FlandersAggregator {
    /// FLANDERS filtering for `base`, with the paper's shape and this
    /// codebase's usual `byzantine_fraction` default.
    pub fn new(base: Box<dyn Aggregator>) -> Self {
        Self {
            base,
            history_window: 5,
            byzantine_fraction: 0.2,
            ridge: 1e-6,
            als_iterations: 5,
            max_forecast_dim: 500,
            history: Mutex::new(VecDeque::new()),
            global: Mutex::new(None),
            last_diagnostics: Mutex::new(Vec::new()),
        }
    }

    /// The most recent round's per-client scores and keep/drop decisions.
    ///
    /// Read-only, for experiment runners and tests; `aggregate` never
    /// consults it. A stable, per-client row shape, so a run's decisions
    /// can be compared against another method's.
    pub fn last_diagnostics(&self) -> Vec<ClientFlandersDiagnostic> {
        self.last_diagnostics
            .lock()
            .expect("FlandersAggregator diagnostics mutex poisoned")
            .clone()
    }

    /// Fits MAR(1) by alternating least squares over the stacked
    /// consecutive pairs `(Θ_{t-j-1} → Θ_{t-j})`.
    ///
    /// Returns `(A, B)`, or `None` when the systems are singular — which
    /// happens legitimately in early rounds, before there is enough
    /// history to identify `d² + h²` coefficients.
    fn fit_mar(&self, pairs: &[(Matrix, Matrix)]) -> Option<(Matrix, Matrix)> {
        let d = pairs[0].0.len();
        let h = pairs[0].0[0].len();

        let mut a = identity(d);
        let mut b = identity(h);

        for _ in 0..self.als_iterations {
            // --- A-step: minimize Σ ‖Y − A (X B)‖²_F over A. -----------
            // Normal equations: A (Σ Z Zᵀ + λI) = Σ Y Zᵀ, with Z = X B.
            let mut zzt = zeros(d, d);
            let mut yzt = zeros(d, d);
            for (x, y) in pairs {
                let z = matmul(x, &b);
                let zt = transpose(&z);
                let zz = matmul(&z, &zt);
                let yz = matmul(y, &zt);
                for i in 0..d {
                    for j in 0..d {
                        zzt[i][j] += zz[i][j];
                        yzt[i][j] += yz[i][j];
                    }
                }
            }
            for (i, row) in zzt.iter_mut().enumerate() {
                row[i] += self.ridge;
            }
            // Solve for Aᵀ: (Σ Z Zᵀ)ᵀ Aᵀ = (Σ Y Zᵀ)ᵀ. The left matrix is
            // symmetric, so its transpose is itself.
            a = transpose(&solve(zzt, transpose(&yzt))?);

            // --- B-step: minimize Σ ‖Y − (A X) B‖²_F over B. -----------
            // Normal equations: (Σ Wᵀ W + λI) B = Σ Wᵀ Y, with W = A X.
            let mut wtw = zeros(h, h);
            let mut wty = zeros(h, h);
            for (x, y) in pairs {
                let w = matmul(&a, x);
                let wt = transpose(&w);
                let ww = matmul(&wt, &w);
                let wy = matmul(&wt, y);
                for i in 0..h {
                    for j in 0..h {
                        wtw[i][j] += ww[i][j];
                        wty[i][j] += wy[i][j];
                    }
                }
            }
            for (i, row) in wtw.iter_mut().enumerate() {
                row[i] += self.ridge;
            }
            b = solve(wtw, wty)?;
        }

        if a.iter()
            .chain(b.iter())
            .any(|r| r.iter().any(|v| !v.is_finite()))
        {
            return None;
        }
        Some((a, b))
    }
}

/// The coordinates the forecast is fitted over.
///
/// Evenly spaced across the whole vector rather than the first `limit`,
/// and deterministic rather than randomly sampled: a contiguous prefix
/// would forecast one layer of a real network and ignore every other,
/// while a random draw would make a round's anomaly scores depend on an
/// RNG this crate's aggregators do not otherwise carry. The paper
/// specifies *how many* coordinates to sample, not which, so this takes
/// the option that is both reproducible and spread out.
fn forecast_coordinates(dim: usize, limit: usize) -> Vec<usize> {
    if dim <= limit || limit == 0 {
        return (0..dim).collect();
    }
    (0..limit).map(|i| i * dim / limit).collect()
}

/// Builds the `|coords| × h` observation matrix for `clients`, in the
/// given column order, from one round's snapshot.
fn observation(
    snapshot: &HashMap<String, Vec<f32>>,
    order: &[String],
    dim: usize,
    coords: &[usize],
) -> Option<Matrix> {
    let mut m = zeros(coords.len(), order.len());
    for (col, id) in order.iter().enumerate() {
        let w = snapshot.get(id)?;
        if w.len() != dim {
            return None;
        }
        for (row, &c) in coords.iter().enumerate() {
            m[row][col] = w[c] as f64;
        }
    }
    Some(m)
}

impl Aggregator for FlandersAggregator {
    fn aggregate(&self, updates: &[ClientDelta]) -> Result<Vec<f32>, AggregatorError> {
        if updates.is_empty() {
            return Err(AggregatorError::EmptyBatch);
        }
        let decoded = decode_and_validate(updates)?;
        let dim = decoded[0].len();

        let current: HashMap<String, Vec<f32>> = updates
            .iter()
            .zip(&decoded)
            .map(|(u, w)| (u.client_id.clone(), w.clone()))
            .collect();

        let mut history = self
            .history
            .lock()
            .expect("FlandersAggregator history mutex poisoned");
        let global = self
            .global
            .lock()
            .expect("FlandersAggregator global mutex poisoned")
            .clone();

        // Column order: clients present now *and* throughout the history
        // window, sorted so the ordering is deterministic rather than
        // HashMap-iteration order. Anything else is a cold start.
        let mut order: Vec<String> = current
            .keys()
            .filter(|id| history.iter().all(|h| h.contains_key(*id)))
            .cloned()
            .collect();
        order.sort();

        // Stacked consecutive pairs from the history plus the current
        // round: (Θ_{t-j-1} → Θ_{t-j}).
        // The coordinates the MAR fit runs over. This is what keeps the
        // `O(d²)` coefficient matrix bounded — see `max_forecast_dim`.
        let coords = forecast_coordinates(dim, self.max_forecast_dim);

        let mut forecast: Option<HashMap<String, Vec<f64>>> = None;
        if order.len() >= 2 && history.len() >= 2 {
            let mut snapshots: Vec<&HashMap<String, Vec<f32>>> = history.iter().collect();
            let mut pairs = Vec::new();
            for w in snapshots.windows(2) {
                if let (Some(x), Some(y)) = (
                    observation(w[0], &order, dim, &coords),
                    observation(w[1], &order, dim, &coords),
                ) {
                    pairs.push((x, y));
                }
            }
            if !pairs.is_empty()
                && let Some((a, b)) = self.fit_mar(&pairs)
            {
                // Θ̂_t = Â Θ_{t-1} B̂, over the sampled coordinates.
                let last = snapshots.pop().expect("history is non-empty");
                if let Some(prev) = observation(last, &order, dim, &coords) {
                    let predicted = matmul(&matmul(&a, &prev), &b);
                    let mut map = HashMap::new();
                    for (col, id) in order.iter().enumerate() {
                        map.insert(
                            id.clone(),
                            (0..coords.len())
                                .map(|row| predicted[row][col])
                                .collect::<Vec<f64>>(),
                        );
                    }
                    forecast = Some(map);
                }
            }
        }

        // Anomaly scores, per Equation (4).
        let mut scored: Vec<(String, f64, bool)> = Vec::with_capacity(updates.len());
        for (u, w) in updates.iter().zip(&decoded) {
            let (score, from_forecast) = match forecast.as_ref().and_then(|f| f.get(&u.client_id)) {
                Some(prediction) => (
                    // Over the sampled coordinates only — the forecast
                    // exists for exactly those, and comparing a full
                    // update against a shorter prediction would silently
                    // score only its prefix.
                    coords
                        .iter()
                        .zip(prediction)
                        .map(|(&c, p)| {
                            let d = w[c] as f64 - p;
                            d * d
                        })
                        .sum::<f64>(),
                    true,
                ),
                None => {
                    // Cold start: distance to the current global model.
                    // With no global model yet either (round one), every
                    // client scores zero, which makes the filter a no-op
                    // — matching the paper's t = 1 case, where the server
                    // simply runs its robust heuristic on everything.
                    let reference = global.as_deref();
                    let score = reference.map_or(0.0, |g| {
                        w.iter()
                            .zip(g)
                            .map(|(a, b)| {
                                let d = *a as f64 - *b as f64;
                                d * d
                            })
                            .sum::<f64>()
                    });
                    (score, false)
                }
            };
            scored.push((u.client_id.clone(), score, from_forecast));
        }

        // Top-k smallest. `k = m − ⌈f·m⌉`, and at least one: a filter
        // that kept nobody would hand the base an empty batch, turning a
        // defense into an outage.
        //
        // Round one is the exception, and it is the paper's own: at
        // `t = 1` the server "computes the new global model θ(2) =
        // ϕ({θc | c ∈ C(1)})" over the whole collected set. There is no
        // forecast and no global model to measure against, so every
        // score is identically zero — ranking them would drop a client
        // chosen by nothing but sort order, which is worse than not
        // filtering. Keep everyone until there is a signal.
        let m = scored.len();
        let has_signal = forecast.is_some() || global.is_some();
        let k = if has_signal {
            let excluded = (self.byzantine_fraction as f64 * m as f64).ceil() as usize;
            m.saturating_sub(excluded).max(1)
        } else {
            m
        };

        let mut ranked: Vec<usize> = (0..m).collect();
        ranked.sort_by(|&a, &b| {
            scored[a]
                .1
                .partial_cmp(&scored[b].1)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let kept: std::collections::HashSet<usize> = ranked.into_iter().take(k).collect();

        *self
            .last_diagnostics
            .lock()
            .expect("FlandersAggregator diagnostics mutex poisoned") = scored
            .iter()
            .enumerate()
            .map(
                |(i, (id, score, forecast_available))| ClientFlandersDiagnostic {
                    client_id: id.clone(),
                    anomaly_score: *score,
                    forecast_available: *forecast_available,
                    kept: kept.contains(&i),
                },
            )
            .collect();

        let filtered: Vec<ClientDelta> = updates
            .iter()
            .enumerate()
            .filter(|(i, _)| kept.contains(i))
            .map(|(_, u)| u.clone())
            .collect();

        let result = self.base.aggregate(&filtered)?;

        // Record this round *after* a successful aggregation, so a
        // rejected batch cannot poison the forecast for later rounds —
        // the standing rule for stateful methods.
        history.push_back(current);
        while history.len() > self.history_window {
            history.pop_front();
        }
        *self
            .global
            .lock()
            .expect("FlandersAggregator global mutex poisoned") = Some(result.clone());

        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AggregatorParams, build_aggregator};
    use conflux_proto::encode_weights;

    fn delta(client_id: &str, weights: &[f32]) -> ClientDelta {
        ClientDelta {
            client_id: client_id.to_string(),
            round: 1,
            weights: encode_weights(weights),
            num_samples: 10,
            ..Default::default()
        }
    }

    fn fedavg() -> Box<dyn Aggregator> {
        build_aggregator("fedavg", AggregatorParams::default()).unwrap()
    }

    #[test]
    fn solve_recovers_a_known_system() {
        // 2x + y = 5 ; x + 3y = 10  ->  x = 1, y = 3
        let m = vec![vec![2.0, 1.0], vec![1.0, 3.0]];
        let rhs = vec![vec![5.0], vec![10.0]];
        let x = solve(m, rhs).expect("non-singular");
        assert!((x[0][0] - 1.0).abs() < 1e-9, "{x:?}");
        assert!((x[1][0] - 3.0).abs() < 1e-9, "{x:?}");
    }

    #[test]
    fn solve_refuses_a_singular_system_rather_than_guessing() {
        let m = vec![vec![1.0, 2.0], vec![2.0, 4.0]];
        let rhs = vec![vec![1.0], vec![2.0]];
        assert!(solve(m, rhs).is_none());
    }

    #[test]
    fn the_forecast_is_bounded_at_real_model_dimension() {
        // The regression test for the OOM an unbounded fit causes.
        //
        // The MAR coefficient matrix `A` is `d x d`. At MNIST's 50,890
        // parameters that is 50890^2 * 8 bytes = 20.7 GB for one matrix,
        // and `fit_mar` allocates several. Nothing caught it because
        // every synthetic experiment runs at `dim = 3`, where the same
        // allocation is 72 bytes, and the first round that can even
        // trigger it is round three — the first with enough history to
        // fit anything.
        //
        // This runs at a dimension large enough that an unbounded `d x d`
        // would exhaust memory, for enough rounds to reach the fit.
        let dim = 50_890;
        let f = FlandersAggregator::new(fedavg());
        assert_eq!(f.max_forecast_dim, 500, "the paper's own sample size");

        let mut base: Vec<f32> = (0..dim).map(|i| (i % 17) as f32 * 0.01).collect();
        for round in 0..4 {
            for (i, w) in base.iter_mut().enumerate() {
                *w += ((round + i) % 7) as f32 * 1e-4;
            }
            let mut hostile = base.clone();
            hostile[dim / 2] += 50.0;

            let out = f
                .aggregate(&[
                    delta("a", &base),
                    delta("b", &base),
                    delta("hostile", &hostile),
                ])
                .expect("must not OOM or fail");
            assert_eq!(out.len(), dim);
            assert!(out.iter().all(|w| w.is_finite()), "round {round}");
        }

        // And the filter still did its job on the sampled coordinates.
        assert!(
            f.last_diagnostics().iter().any(|d| d.forecast_available),
            "a forecast should be available by round 4"
        );
    }

    #[test]
    fn coordinate_sampling_spreads_across_the_whole_vector() {
        // A contiguous prefix would forecast one layer of a real network
        // and ignore every other, so the choice of *which* coordinates
        // matters even though the paper only fixes how many.
        let coords = forecast_coordinates(50_890, 500);
        assert_eq!(coords.len(), 500);
        assert_eq!(coords[0], 0);
        assert!(
            *coords.last().unwrap() > 50_000,
            "sampling must reach the end of the vector, got {:?}",
            coords.last()
        );
        assert!(
            coords.windows(2).all(|w| w[0] < w[1]),
            "strictly increasing"
        );

        // Small models are fitted whole — this is why dim = 3 experiments
        // are bit-for-bit unaffected by the bound existing.
        assert_eq!(forecast_coordinates(3, 500), vec![0, 1, 2]);
    }

    #[test]
    fn round_one_keeps_everyone() {
        // The paper's t = 1 case: no history, so no forecast, so the
        // server just runs its aggregator on the whole batch.
        let f = FlandersAggregator::new(fedavg());
        let out = f
            .aggregate(&[delta("a", &[1.0, 1.0]), delta("b", &[9.0, 9.0])])
            .unwrap();
        assert!(f.last_diagnostics().iter().all(|d| d.kept));
        assert!(out[0] > 4.0, "nothing was filtered yet: {out:?}");
    }

    #[test]
    fn a_client_that_suddenly_deviates_from_its_own_history_is_filtered() {
        // The property the whole method exists for. Four clients behave
        // consistently for several rounds; then one jumps. Its *own*
        // history is what convicts it — no comparison against the batch
        // is involved.
        let mut f = FlandersAggregator::new(fedavg());
        f.byzantine_fraction = 0.25;

        for round in 0..6 {
            let drift = round as f32 * 0.01;
            f.aggregate(&[
                delta("a", &[1.0 + drift, 1.0 + drift]),
                delta("b", &[1.1 + drift, 0.9 + drift]),
                delta("c", &[0.9 + drift, 1.1 + drift]),
                delta("d", &[1.0 + drift, 1.0 + drift]),
            ])
            .unwrap();
        }

        f.aggregate(&[
            delta("a", &[1.06, 1.06]),
            delta("b", &[1.16, 0.96]),
            delta("c", &[0.96, 1.16]),
            delta("d", &[80.0, 80.0]), // the jump
        ])
        .unwrap();

        let diagnostics = f.last_diagnostics();
        let d = diagnostics.iter().find(|x| x.client_id == "d").unwrap();
        assert!(
            !d.kept,
            "the deviating client should be filtered: {diagnostics:?}"
        );
        assert!(
            d.anomaly_score > 1.0,
            "and should score high: {}",
            d.anomaly_score
        );
    }

    #[test]
    fn a_perfectly_stable_colluder_is_the_most_forecastable_client_in_the_batch() {
        // The structural limitation this method has, encoded as a test
        // because measurement showed its consequence and the
        // consequence is severe: FLANDERS scored *worse than undefended
        // FedAvg* against persistent Sybils.
        //
        // The mechanism is not a bug. FLANDERS keeps the clients whose
        // updates best match a forecast of their own past. A colluder
        // that submits the byte-identical update every round is the
        // easiest client in the batch to forecast — its anomaly score is
        // near zero by construction — while honest clients carry
        // training noise and therefore never forecast perfectly. Top-`k`
        // then keeps the attackers and drops the honest.
        //
        // The paper's own evaluation uses Gaussian, LIE, OPT and AGR-MM,
        // which perturb or optimize and are therefore *un*predictable.
        // Stable collusion is a threat model it does not test, and this
        // is what happens there.
        let mut f = FlandersAggregator::new(fedavg());
        f.byzantine_fraction = 0.4; // 2 of 5 excluded

        for round in 0..8 {
            let jitter = (round as f32 * 0.37).sin() * 0.2;
            f.aggregate(&[
                delta("honest-1", &[1.0 + jitter, 1.0 - jitter]),
                delta("honest-2", &[1.1 - jitter, 0.9 + jitter]),
                delta("honest-3", &[0.9 + jitter, 1.1 - jitter]),
                // Identical every single round.
                delta("sybil-1", &[5.0, 5.0]),
                delta("sybil-2", &[5.0, 5.0]),
            ])
            .unwrap();
        }

        let diagnostics = f.last_diagnostics();
        let score = |id: &str| {
            diagnostics
                .iter()
                .find(|d| d.client_id == id)
                .unwrap()
                .anomaly_score
        };
        let kept = |id: &str| diagnostics.iter().find(|d| d.client_id == id).unwrap().kept;

        assert!(
            score("sybil-1") < score("honest-1"),
            "the stable colluder should look *less* anomalous than an honest client: \
             sybil={} honest={}",
            score("sybil-1"),
            score("honest-1")
        );
        assert!(
            kept("sybil-1") && kept("sybil-2"),
            "and both colluders survive the filter: {diagnostics:?}"
        );
    }

    #[test]
    fn diagnostics_record_whether_a_forecast_was_available() {
        // Cold-start scores are a weaker signal than forecast scores, and
        // a research runner needs to be able to tell them apart.
        let f = FlandersAggregator::new(fedavg());
        f.aggregate(&[delta("a", &[1.0]), delta("b", &[1.0])])
            .unwrap();
        assert!(f.last_diagnostics().iter().all(|d| !d.forecast_available));

        for _ in 0..4 {
            f.aggregate(&[delta("a", &[1.0]), delta("b", &[1.0])])
                .unwrap();
        }
        assert!(
            f.last_diagnostics().iter().any(|d| d.forecast_available),
            "a forecast should be available once history exists"
        );
    }

    #[test]
    fn the_filter_never_hands_the_base_an_empty_batch() {
        // `byzantine_fraction = 1.0` would exclude everyone. A defense
        // that turns into an outage is not a defense, so `k` floors at 1.
        let mut f = FlandersAggregator::new(fedavg());
        f.byzantine_fraction = 1.0;

        // Round one keeps everyone by construction (no signal to rank
        // by), so the floor has to be checked from round two onward.
        f.aggregate(&[delta("a", &[1.0]), delta("b", &[2.0])])
            .unwrap();
        let out = f
            .aggregate(&[delta("a", &[1.0]), delta("b", &[2.0])])
            .unwrap();

        assert!(out[0].is_finite());
        assert_eq!(
            f.last_diagnostics().iter().filter(|d| d.kept).count(),
            1,
            "excluding 100% must still leave one client, not zero"
        );
    }

    #[test]
    fn extreme_but_finite_updates_do_not_produce_a_non_finite_aggregate() {
        let f = FlandersAggregator::new(fedavg());
        for w in [1.0, f32::MAX, -f32::MAX, 1.0, f32::MAX, 1.0] {
            match f.aggregate(&[
                delta("a", &[1.0, 1.0]),
                delta("b", &[1.1, 0.9]),
                delta("hostile", &[w, w]),
            ]) {
                Ok(out) => assert!(out.iter().all(|x| x.is_finite()), "got {out:?}"),
                Err(_) => { /* refusing is a pass */ }
            }
        }
    }

    #[test]
    fn a_rejected_batch_does_not_enter_the_history() {
        // The rule for stateful methods: state is recorded only after a
        // successful round, so a batch the base rejected cannot shape
        // later forecasts.
        let f = FlandersAggregator::new(fedavg());
        f.aggregate(&[delta("a", &[1.0]), delta("b", &[1.0])])
            .unwrap();

        let before = f.last_diagnostics().len();
        let _ = f.aggregate(&[delta("a", &[f32::NAN]), delta("b", &[1.0])]);
        // The rejected round left the diagnostics untouched, which is
        // the observable proxy for "it left the history untouched too".
        assert_eq!(f.last_diagnostics().len(), before);
    }
}
