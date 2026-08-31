//! Every failure mode `run_round` (or the HTTP surface) can produce,
//! wrapping the downstream crates' own error types rather than flattening
//! them into strings.

#[derive(Debug, thiserror::Error)]
/// Why a round failed.
pub enum ServerError {
    /// `budget_exhausted_action = halt` (production default, spec §4.1)
    /// and the accountant reports the epsilon budget is spent.
    #[error("privacy budget exhausted for this experiment")]
    BudgetExhausted,
    /// Phase 14: the `PerClient`-scope counterpart — `budget_exhausted_
    /// action = halt` and a specific client's own cumulative epsilon
    /// (not the experiment-wide total) has reached `target_epsilon`.
    #[error("privacy budget exhausted for client {client_id}")]
    BudgetExhaustedForClient {
        /// The client whose per-client epsilon budget is spent.
        client_id: String,
    },
    #[error(transparent)]
    /// The client registry was unreachable or refused an operation.
    Registry(#[from] conflux_registry::RegistryError),
    #[error(transparent)]
    /// A checkpoint could not be read or written.
    Store(#[from] conflux_store::StoreError),
    #[error(transparent)]
    /// The batch could not be aggregated — see `AggregatorError` for
    /// which validation rejected it.
    Aggregator(#[from] conflux_core::AggregatorError),
}
