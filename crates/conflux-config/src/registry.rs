//! A compile-time strategy registry: lets an algorithm implementation in
//! another crate become selectable by name from config
//! (`aggregator = "fedavg"`) without `conflux-config` — or
//! `conflux-server`, which reads it — ever needing to import that other
//! crate. Each implementation submits one [`StrategyEntry`] via
//! `inventory::submit!` at the top level of its own file; nothing has to
//! collect these into a central list by hand.
//!
//! `conflux-core`, `conflux-selector`, and `conflux-privacy` each submit
//! one entry per algorithm they ship — `conflux-core` alone currently
//! registers every member of its `averaging` and `robust` aggregator
//! families this way. Each of those crates also calls [`lookup`]
//! themselves, in their own "construct the implementation for this
//! configured name" dispatch function, to check a name against every
//! entry submitted anywhere in the final binary before building the
//! corresponding implementation.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Which family a registered strategy belongs to. The three families
/// spec §5 defines.
pub enum StrategyKind {
    /// `conflux-core`'s aggregation methods.
    Aggregator,
    /// `conflux-privacy`'s mechanisms.
    PrivacyMechanism,
    /// `conflux-selector`'s client-sampling strategies.
    Selector,
}

/// One registered implementation, submitted at compile time via
/// `inventory::submit!` from the crate that implements it.
///
/// This is what lets configuration select an implementation by name
/// without `conflux-server` naming the type — and without this crate
/// depending on the crates that implement them.
pub struct StrategyEntry {
    /// Which family it belongs to.
    pub kind: StrategyKind,
    /// The name configuration selects it by, e.g. `"fedavg"`.
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
