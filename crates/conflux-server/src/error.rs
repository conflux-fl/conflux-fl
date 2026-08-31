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
    /// A `trusted`-family aggregator's reference could not be obtained
    /// this round (ADR 0011) — no sidecar configured, unreachable,
    /// answering for the wrong round, or returning something
    /// undecodable.
    ///
    /// Fatal to the round rather than something to continue past. A
    /// trusted-family method with no reference has nothing to be trusted
    /// *against*; aggregating anyway would mean falling back to some
    /// other rule at exactly the moment the defense was supposed to
    /// engage, and writing a checkpoint indistinguishable from a healthy
    /// one.
    #[error("no trusted reference available for round {round}: {reason}")]
    TrustedReferenceUnavailable {
        /// The round that could not proceed.
        round: u64,
        /// What went wrong, for the operator.
        reason: String,
    },
}

impl ServerError {
    /// Whether the round loop should try again, or stop for good.
    ///
    /// Tier 5 (H2). The loop previously stopped on *every* error but
    /// `EmptyBatch`, which meant one Redis reconnect or one client
    /// sending a `NaN` ended the experiment permanently — while the gRPC
    /// and HTTP servers kept running, so nothing outside the process
    /// could tell. The distinction that matters is not "how bad is this
    /// error" but **"can the next round differ from this one?"**
    ///
    /// - **Transient**: backend I/O ([`ServerError::Registry`],
    ///   [`ServerError::Store`]) and every aggregation rejection. A
    ///   rejected batch is a statement about *this round's* batch — the
    ///   client that sent `NaN` may not be selected next round, and if it
    ///   is, the rejection is doing its job every time. Retrying is
    ///   correct in both cases.
    /// - **Fatal**: an exhausted privacy budget, in either scope. This is
    ///   the one case where stopping *is* the specified behavior rather
    ///   than a failure to handle something —
    ///   `budget_exhausted_action = halt` means halt, and no amount of
    ///   waiting produces more budget (ADR 0006).
    ///
    /// Retrying a transient error is not the same as ignoring it: the
    /// caller backs off, counts consecutive failures, and reports the
    /// round loop as degraded so an operator and an orchestrator can both
    /// see it.
    pub fn is_transient(&self) -> bool {
        match self {
            // Halt means halt. Waiting cannot produce more epsilon.
            ServerError::BudgetExhausted | ServerError::BudgetExhaustedForClient { .. } => false,
            // A backend that was unreachable a moment ago may be
            // reachable now — this is the case the old behavior got
            // most wrong.
            ServerError::Registry(_) | ServerError::Store(_) => true,
            // Every aggregation rejection describes one batch, not the
            // experiment. `EmptyBatch` in particular is the ordinary
            // "nobody has registered yet" startup case.
            ServerError::Aggregator(_) => true,
            // Transient for the same reason `Registry`/`Store` are: a
            // sidecar is a backend, and one that was unreachable a moment
            // ago may be reachable now. The loop backs off and reports
            // itself degraded rather than stopping — which is right even
            // for the "no sidecar configured" case, since that is a
            // misconfiguration an operator fixes by starting one, and a
            // crash-looping server is a worse way to say so than a
            // degraded health endpoint that names the problem.
            ServerError::TrustedReferenceUnavailable { .. } => true,
        }
    }
}
