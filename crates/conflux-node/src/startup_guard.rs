//! Phase 9b — production stub-client guard. Closes gap 5 from
//! `docs/FLOWER_COMPARISON.md` / the CLAUDE.md constraint, implemented
//! where it architecturally belongs: `conflux-node` is the only process
//! with a local loopback listener (ADR 0004) a Python `ClientApp`
//! connects to — `conflux-server` never talks to Python at all, despite
//! the spec's original wording naming it.
//!
//! `conflux-node` has no `conflux-config` dependency (`docs/phases/
//! phase-6-node.md`'s deliberate scope decision, preserved here) — these
//! two enums are small, locally-defined stand-ins for the one bit of
//! `conflux-config::Mode` and one new concept this guard needs, not a
//! reason to pull the whole crate in.

/// Mirrors `conflux-config::Mode`, but defined locally rather than
/// depending on that crate for one enum — see the module doc comment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeMode {
    Research,
    Production,
}

/// What's actually listening on the local loopback port. `conflux-node`
/// has no protocol-level way to verify this on its own (no handshake
/// field carries it, and ADR 0005 defers the real Python SDK entirely) —
/// it's an explicit operator assertion, the same way `require_node_auth`
/// (Phase 8b) made a security posture an explicit config value rather
/// than an implicit assumption.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientAppKind {
    /// `python/conflux_client/stub_client.py` — fixed dummy weights, no
    /// PyTorch. Research-only per its own README; this guard is what
    /// makes that convention machine-enforced.
    Stub,
    /// Anything the operator affirmatively declares isn't the stub.
    Real,
}

#[derive(Debug, thiserror::Error)]
pub enum StartupGuardError {
    #[error(
        "mode = production with a stub ClientApp is refused (spec: allow_stub_client = false \
         in production) — set CONFLUX_CLIENT_APP_KIND=real once a real ClientApp is wired up, \
         or CONFLUX_ALLOW_STUB_CLIENT=true to override for a deliberate production pipeline test"
    )]
    ProductionRefusesStubClient,
}

/// Whether `conflux-node` may start serving its local loopback listener.
/// A pure function (no I/O, no env reads) — every branch is
/// unit-testable without a real process, mirroring `conflux-server`'s
/// `backend_selection::validate_production_backends` and
/// `auth_enforcement::resolve_server_tls`.
pub fn validate_client_app_startup(
    mode: RuntimeMode,
    allow_stub_client: bool,
    kind: ClientAppKind,
) -> Result<(), StartupGuardError> {
    if mode == RuntimeMode::Production && !allow_stub_client && kind == ClientAppKind::Stub {
        return Err(StartupGuardError::ProductionRefusesStubClient);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn production_with_stub_and_no_override_fails() {
        let err = validate_client_app_startup(RuntimeMode::Production, false, ClientAppKind::Stub)
            .unwrap_err();
        assert!(matches!(
            err,
            StartupGuardError::ProductionRefusesStubClient
        ));
    }

    #[test]
    fn production_with_stub_and_explicit_override_succeeds() {
        validate_client_app_startup(RuntimeMode::Production, true, ClientAppKind::Stub).unwrap();
    }

    #[test]
    fn production_with_real_kind_succeeds_regardless_of_override() {
        validate_client_app_startup(RuntimeMode::Production, false, ClientAppKind::Real).unwrap();
        validate_client_app_startup(RuntimeMode::Production, true, ClientAppKind::Real).unwrap();
    }

    #[test]
    fn research_with_stub_succeeds_this_is_todays_default() {
        validate_client_app_startup(RuntimeMode::Research, true, ClientAppKind::Stub).unwrap();
        // Even an explicit `allow_stub_client=false` override in research
        // mode doesn't matter — the guard only ever fires in production.
        validate_client_app_startup(RuntimeMode::Research, false, ClientAppKind::Stub).unwrap();
    }

    #[test]
    fn research_with_real_kind_succeeds() {
        validate_client_app_startup(RuntimeMode::Research, true, ClientAppKind::Real).unwrap();
    }
}
