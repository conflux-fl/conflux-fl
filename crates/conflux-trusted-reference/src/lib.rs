//! The optional trusted-reference sidecar: a separate process that holds
//! the trusted root dataset FLTrust and Zeno need.
//!
//! FLTrust (Cao, Fang, Liu, Jia & Gong, 2021) and Zeno/Zeno++ (Xie,
//! Koyejo & Gupta, 2019/2020) are the only two methods in Conflux's
//! tracked landscape whose published algorithm requires the *server* to
//! hold data and train or evaluate loss on it. Conflux keeps PyTorch
//! entirely client-side and `conflux-server` opaque to model
//! architecture — which is why the wire format is a flat `f32` array,
//! and why neither method was implementable inside the server at all.
//!
//! The resolution is to move the capability *out* of the server rather
//! than into it. This crate is that separate process. The
//! consequence worth stating plainly: **`conflux-server` does not depend
//! on this crate, at any depth, and must not be made to.** The client
//! the server uses lives in `conflux-net`
//! (`conflux_net::TrustedReferenceTransport`), so the server can call a
//! sidecar without ever linking one — the same separation the workspace
//! keeps between `conflux-server` and `conflux-attacks`.
//!
//! # What this crate does and does not provide
//!
//! It provides the **boundary**: the gRPC service, the
//! [`TrustedModel`] extension point, and a working implementation of it
//! ([`LinearLeastSquares`]) that genuinely trains — real gradient
//! descent on a real dataset, no stub, no fixed return value.
//!
//! It does **not** provide a general deep-learning runtime, and that is
//! deliberate rather than unfinished. [`LinearLeastSquares`] is honest
//! about its own reach: it is a linear model, so it is a faithful
//! trusted reference for a linear task and nothing else. A deployment
//! training a convolutional network needs a [`TrustedModel`] impl that
//! can run that architecture — an `ort`/ONNX binding, a `tch` binding,
//! or a Python sidecar speaking the same gRPC service. That is exactly
//! the extension this crate exists to make possible, and exactly the
//! dependency that must never be put in `conflux-server`.
//!
//! # Example
//!
//! ```
//! use conflux_trusted_reference::{LinearLeastSquares, TrustedModel};
//!
//! // A trusted root dataset: y = 2x₀ + 3x₁, which the model should
//! // recover from a deliberately wrong starting point.
//! let model = LinearLeastSquares::new(
//!     vec![
//!         (vec![1.0, 0.0], 2.0),
//!         (vec![0.0, 1.0], 3.0),
//!         (vec![1.0, 1.0], 5.0),
//!         (vec![2.0, 1.0], 7.0),
//!     ],
//!     0.1,  // learning rate
//!     500,  // steps
//! );
//!
//! let reference = model.train_reference(&[0.0, 0.0]);
//! assert!((reference[0] - 2.0).abs() < 0.1, "got {reference:?}");
//! assert!((reference[1] - 3.0).abs() < 0.1, "got {reference:?}");
//!
//! // And it can say which of two candidate updates is better, which is
//! // the other half of what a sidecar is for.
//! let good = model.score(&[0.0, 0.0], &[2.0, 3.0]);
//! let bad = model.score(&[0.0, 0.0], &[-5.0, 9.0]);
//! assert!(good > bad);
//! ```

#![warn(missing_docs)]

mod linear;
mod service;

pub use linear::LinearLeastSquares;
pub use service::{TrustedReferenceService, serve};

/// What the sidecar can actually do with a model, independent of gRPC.
///
/// The extension point of this crate. A deployment whose model is not
/// linear implements this against whatever runtime can run it — ONNX,
/// libtorch, a Python process — and gets FLTrust/Zeno support without
/// `conflux-server` learning anything about model architecture, which is
/// the entire point of running it as a separate process.
///
/// Both methods take `&self`: one model serves every round, and the
/// service holds it behind an `Arc`. A model needing to mutate state
/// across rounds follows the same interior-mutability rule `Aggregator`
/// uses — a `Mutex` field, not a `&mut self` signature.
pub trait TrustedModel: Send + Sync {
    /// Train from `global_weights` on the trusted root dataset and
    /// return the resulting weights.
    ///
    /// Returns **weights, not a delta**, because that is what Conflux
    /// transmits everywhere else: a client's submission is its trained
    /// weight vector, so the reference has to be the same shape to be
    /// comparable to one. FLTrust's own formulation is in terms of
    /// updates; the subtraction against the global model is the
    /// aggregator's job, where both vectors are in hand.
    ///
    /// The returned vector must have the same length as
    /// `global_weights`. An implementation that cannot honor that should
    /// return `global_weights` unchanged rather than a differently-shaped
    /// vector — a reference of the wrong length is rejected downstream,
    /// but a silently truncated one might not be.
    fn train_reference(&self, global_weights: &[f32]) -> Vec<f32>;

    /// How much better `candidate` is than `global_weights` on the
    /// held-out set. Higher is better; negative means the candidate made
    /// the held-out loss worse.
    ///
    /// This is Zeno's signal. Expressed as an improvement rather than a
    /// raw loss so the sign carries meaning on its own, without a caller
    /// needing to know the loss scale of a model it is opaque to.
    fn score(&self, global_weights: &[f32], candidate: &[f32]) -> f32;

    /// How many weights this model expects, if known before a request
    /// arrives.
    ///
    /// `None` is legitimate — a model built lazily cannot answer yet —
    /// but answering lets the server refuse a mismatched sidecar at
    /// startup instead of mid-round, which is the same fail-fast posture
    /// `validate_production_backends` already takes.
    fn model_dim(&self) -> Option<u64> {
        None
    }

    /// Free-text, for logs: what dataset, what model. Never parsed.
    fn description(&self) -> String {
        "unnamed trusted model".to_string()
    }

    /// Whether `train_reference` is meaningful for this model. FLTrust
    /// needs it.
    fn supports_reference_update(&self) -> bool {
        true
    }

    /// Whether `score` is meaningful for this model. Zeno needs it.
    fn supports_scoring(&self) -> bool {
        true
    }
}
