//! Compile-time strategy registry (ADR 0002, spec §5). An algorithm
//! implementation in another crate — a new `AveragingWeighting`, a new
//! `RobustSelection` member — submits one [`StrategyEntry`] via
//! `inventory::submit!` to become selectable by name from config
//! (`aggregator = "fedavg"`) without `conflux-server` needing to know
//! about it at compile time.
//!
//! No real crate submits an entry yet — that starts in Phase 2, when
//! `conflux-selector`/`conflux-privacy`/`conflux-core` ship their first
//! family members. This phase only builds and tests the mechanism itself.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StrategyKind {
    Aggregator,
    PrivacyMechanism,
    Selector,
}

pub struct StrategyEntry {
    pub kind: StrategyKind,
    pub name: &'static str,
}

inventory::collect!(StrategyEntry);

/// Finds the strategy named `name` within `kind`, if any crate linked into
/// the binary submitted one.
pub fn lookup(kind: StrategyKind, name: &str) -> Option<&'static StrategyEntry> {
    inventory::iter::<StrategyEntry>()
        .into_iter()
        .find(|entry| entry.kind == kind && entry.name == name)
}

#[cfg(test)]
mod tests {
    use super::*;

    inventory::submit! {
        StrategyEntry { kind: StrategyKind::Aggregator, name: "test_dummy_aggregator" }
    }

    #[test]
    fn submitted_entry_is_found_by_lookup() {
        let found = lookup(StrategyKind::Aggregator, "test_dummy_aggregator");

        assert!(found.is_some());
    }

    #[test]
    fn unknown_name_is_not_found() {
        let found = lookup(StrategyKind::Aggregator, "does_not_exist");

        assert!(found.is_none());
    }

    #[test]
    fn same_name_in_different_kind_is_not_found() {
        let found = lookup(StrategyKind::Selector, "test_dummy_aggregator");

        assert!(found.is_none());
    }
}
