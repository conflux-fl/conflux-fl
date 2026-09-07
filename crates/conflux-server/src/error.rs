//! Every failure mode `run_round` (or the HTTP surface) can produce,
//! wrapping the downstream crates' own error types rather than flattening
//! them into strings.

#[derive(Debug, thiserror::Error)]
/// Why a round failed.
pub enum ServerError {
    /// `budget_exhausted_action = halt` (the production default)
    /// and the accountant reports the epsilon budget is spent.
    #[error("privacy budget exhausted for this experiment")]
    BudgetExhausted,
    /// A robust method's batch requirement was not met by the batch that
    /// actually arrived.
    ///
    /// Only reachable when no `quorum` is configured — with one, a round
    /// cannot flush below it and configuration validation refuses the
    /// deployment at startup instead, which is earlier and cheaper.
    #[error(
        "round {round}: {aggregator} would aggregate a batch of {batch}, but {citation} \
         (f = {excluded} here, so it needs {required}) — refusing to produce a result \
         outside the cited guarantee. Set CONFLUX_QUORUM to {required} or more so \
         rounds wait for a batch the citation covers."
    )]
    PreconditionViolated {
        /// The round that could not be aggregated.
        round: u64,
        /// The method whose citation was not satisfied.
        aggregator: String,
        /// How many submissions arrived.
        batch: u32,
        /// The smallest batch the citation covers.
        required: u64,
        /// How many the implementation would exclude at this size.
        excluded: u64,
        /// The paper's own statement of the rule.
        citation: String,
    },
    /// The `PerClient`-scope counterpart — `budget_exhausted_
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
    /// this round — no sidecar configured, unreachable,
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
    /// A loop that stopped on *every* error but `EmptyBatch` would let
    /// one Redis reconnect or one client sending a `NaN` end the
    /// experiment permanently — while the gRPC and HTTP servers kept
    /// running, so nothing outside the process could tell. The
    /// distinction that matters is not "how bad is this error" but
    /// **"can the next round differ from this one?"**
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
    ///   waiting produces more budget.
    ///
    /// Retrying a transient error is not the same as ignoring it: the
    /// caller backs off, counts consecutive failures, and reports the
    /// round loop as degraded so an operator and an orchestrator can both
    /// see it.
    pub fn is_transient(&self) -> bool {
        match self {
            // Halt means halt. Waiting cannot produce more epsilon.
            ServerError::BudgetExhausted | ServerError::BudgetExhaustedForClient { .. } => false,
            // Fatal for the same reason: the next round would aggregate
            // outside the citation exactly as this one would, and the
            // result would look ordinary. Retrying produces more invalid
            // rounds, not fewer.
            ServerError::PreconditionViolated { .. } => false,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_precondition_violation_is_fatal_not_retryable() {
        // The classification *is* the behaviour: transient errors back
        // off and retry, and retrying here would aggregate outside the
        // citation again, writing a checkpoint indistinguishable from a
        // sound one. It belongs beside `BudgetExhausted` — halt means
        // halt.
        let e = ServerError::PreconditionViolated {
            round: 7,
            aggregator: "krum".to_string(),
            batch: 4,
            required: 5,
            excluded: 1,
            citation: "Krum requires n ≥ 2f + 3".to_string(),
        };
        assert!(!e.is_transient());
    }

    #[test]
    fn the_violation_message_carries_the_arithmetic_and_the_way_out() {
        let message = ServerError::PreconditionViolated {
            round: 7,
            aggregator: "krum".to_string(),
            batch: 4,
            required: 5,
            excluded: 1,
            citation: "Krum requires n ≥ 2f + 3".to_string(),
        }
        .to_string();
        assert!(message.contains("Krum requires n ≥ 2f + 3"), "{message}");
        assert!(message.contains("CONFLUX_QUORUM"), "{message}");
        // Line continuations have mangled user-facing strings in this
        // codebase repeatedly; this one is read in a terminal.
        assert!(!message.contains("  "), "double space in: {message}");
    }
}
