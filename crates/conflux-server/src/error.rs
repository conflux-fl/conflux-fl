//! Every failure mode `run_round` (or the HTTP surface) can produce,
//! wrapping the downstream crates' own error types rather than flattening
//! them into strings.

#[derive(Debug, thiserror::Error)]
pub enum ServerError {
    /// `budget_exhausted_action = halt` (production default, spec §4.1)
    /// and the accountant reports the epsilon budget is spent.
    #[error("privacy budget exhausted for this experiment")]
    BudgetExhausted,
    #[error(transparent)]
    Registry(#[from] conflux_registry::RegistryError),
    #[error(transparent)]
    Store(#[from] conflux_store::StoreError),
    #[error(transparent)]
    Aggregator(#[from] conflux_core::AggregatorError),
}
