//! Runnable "try it" for the [crate-deep-dives article on
//! `conflux-selector`](https://confluxfl.dev/crate-deep-dives/conflux-selector/).
//!
//! Run with:
//!   cargo run --example select_clients -p conflux-selector
//!
//! Shows the one thing this crate is about: choosing which clients
//! train in a round, and why a fixed seed makes that choice reproducible
//! (the same round, re-run, selects the same clients) while still
//! varying from one round to the next.

use conflux_selector::{SelectionSeed, build_selector};

fn main() {
    let candidates: Vec<String> = (0..20).map(|i| format!("client-{i}")).collect();

    let selector = build_selector("uniform_random", SelectionSeed::Fixed(42))
        .expect("\"uniform_random\" is a registered conflux-selector strategy");

    let round_5_run_1 = selector.select(&candidates, 5, 5);
    let round_5_run_2 = selector.select(&candidates, 5, 5);
    println!("round 5, run 1: {round_5_run_1:?}");
    println!("round 5, run 2: {round_5_run_2:?}");
    println!(
        "same seed + same round -> identical selection: {}",
        round_5_run_1 == round_5_run_2
    );

    let round_6 = selector.select(&candidates, 5, 6);
    println!("round 6, run 1: {round_6:?}");
    println!(
        "same seed + different round -> different selection: {}",
        round_5_run_1 != round_6
    );

    let os_random_selector = build_selector("uniform_random", SelectionSeed::OsRandom)
        .expect("\"uniform_random\" is a registered conflux-selector strategy");
    let os_random_pick = os_random_selector.select(&candidates, 5, 0);
    println!("OsRandom pick (not reproducible run to run): {os_random_pick:?}");
}
