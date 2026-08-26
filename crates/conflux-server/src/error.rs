//! Every failure mode `run_round` (or the HTTP surface) can produce,
//! wrapping the downstream crates' own error types rather than flattening
//! them into strings.

#[derive(Debug, thiserror::Error)]
pub enum ServerError {
    /// `budget_exhausted_action = halt` (production default, spec §4.1)
    /// and the accountant reports the epsilon budget is spent.
    #[error("privacy budget exhausted for this experiment")]
    BudgetExhausted,
    /// Phase 14: the `PerClient`-scope counterpart — `budget_exhausted_
    /// action = halt` and a specific client's own cumulative epsilon
    /// (not the experiment-wide total) has reached `target_epsilon`.
    #[error("privacy budget exhausted for client {client_id}")]
    BudgetExhaustedForClient { client_id: String },
    #[error(transparent)]
    Registry(#[from] conflux_registry::RegistryError),
    #[error(transparent)]
    Store(#[from] conflux_store::StoreError),
    #[error(transparent)]
    Aggregator(#[from] conflux_core::AggregatorError),
}
