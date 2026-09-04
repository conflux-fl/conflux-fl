//! Runnable "try it" for the [crate-deep-dives article on
//! `conflux-privacy`](https://confluxfl.dev/crate-deep-dives/conflux-privacy/):
//! the crate's two independent halves — clipping + noising one client's
//! update, and tracking cumulative epsilon across several rounds with
//! `RdpAccountant`.
//!
//! Run with:
//!
//! ```sh
//! cargo run --example local_dp_and_accounting -p conflux-privacy
//! ```

use conflux_privacy::{
    GaussianClippingPrivacy, PrivacyAccountant, PrivacyMechanism, RdpAccountant,
};
use rand::SeedableRng;
use rand::rngs::StdRng;

fn l2_norm(weights: &[f32]) -> f32 {
    weights.iter().map(|w| w * w).sum::<f32>().sqrt()
}

fn main() {
    // --- Part 1: clip + noise a single client update ---

    let mechanism = GaussianClippingPrivacy {
        clip_norm: 1.0,
        noise_multiplier: 0.5,
    };

    let mut weights = vec![3.0_f32, 4.0]; // L2 norm 5.0, well above clip_norm
    println!("before: {:?} (L2 norm {:.4})", weights, l2_norm(&weights));

    let mut rng = StdRng::seed_from_u64(42); // fixed seed: this example's output is reproducible
    mechanism.clip(&mut weights);
    println!(
        "after clip only: {:?} (L2 norm {:.4})",
        weights,
        l2_norm(&weights)
    );

    mechanism.add_noise(&mut weights, &mut rng);
    println!("after clip + noise: {:?}", weights);

    // The same transform, reached through the `Box<dyn PrivacyMechanism>`
    // a caller gets back from `build_privacy_mechanism` — this is the
    // trait-object path `conflux-server` actually uses at runtime.
    let boxed: Box<dyn PrivacyMechanism> =
        conflux_privacy::build_privacy_mechanism("gaussian_clipping", 1.0, 0.5)
            .expect("gaussian_clipping is a registered mechanism");
    let mut weights2 = vec![3.0_f32, 4.0];
    let mut rng2 = StdRng::seed_from_u64(42);
    boxed.transform(&mut weights2, &mut rng2);
    println!(
        "same transform via Box<dyn PrivacyMechanism>: {:?}",
        weights2
    );

    // --- Part 2: epsilon accounting across rounds ---

    let mut accountant = RdpAccountant::new();
    let delta = 1e-5;
    let target_epsilon = 15.0;

    println!(
        "\nrunning {} rounds at noise_multiplier=1.0, sample_rate=0.1, target_epsilon={target_epsilon}, delta={delta}",
        8
    );
    for round in 1..=8 {
        accountant.record_round(1.0, 0.1);
        let epsilon = accountant.current_epsilon(delta);
        let exhausted = accountant.budget_exhausted(target_epsilon, delta);
        println!(
            "round {round}: cumulative epsilon = {epsilon:.4} (budget exhausted: {exhausted})"
        );
    }

    // --- Part 3: per-client accounting is a separate history ---

    let mut per_client = RdpAccountant::new();
    for _ in 0..5 {
        per_client.record_round_for_client("heavy-user", 1.0, 0.1);
    }
    per_client.record_round_for_client("light-user", 1.0, 0.1);

    println!(
        "\nheavy-user epsilon: {:.4}",
        per_client.current_epsilon_for_client("heavy-user", delta)
    );
    println!(
        "light-user epsilon: {:.4}",
        per_client.current_epsilon_for_client("light-user", delta)
    );
    println!(
        "experiment-wide epsilon (unaffected by per-client calls): {:.4}",
        per_client.current_epsilon(delta)
    );
}
