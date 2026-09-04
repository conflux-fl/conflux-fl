//! Proves `config.privacy_mechanism.value` actually drives
//! construction through `conflux-config`'s strategy registry.

use conflux_config::{Mode, Overrides, Topology};
use conflux_server::AppState;

#[test]
fn explicit_privacy_mechanism_override_resolves_and_still_clips() {
    let mut overrides = Overrides {
        privacy_mechanism: Some("gaussian_clipping".to_string()),
        clip_norm: Some(1.0),
        noise_multiplier: Some(0.0),
        ..Default::default()
    };
    overrides.round_timeout_secs.get_or_insert(5);
    let config = conflux_config::resolve(
        Topology::CrossDevice,
        Mode::Research,
        Some(("test", &overrides)),
        &Overrides::default(),
        &Overrides::default(),
    )
    .unwrap();

    let state = AppState::new(config, vec![0.0]);
    let mut weights = vec![3.0, 4.0]; // L2 norm 5.0
    let mut rng = rand::rng();

    state.privacy.transform(&mut weights, &mut rng);

    let norm = (weights[0] * weights[0] + weights[1] * weights[1]).sqrt();
    assert!(
        (norm - 1.0).abs() < 1e-4,
        "registry-constructed mechanism should still clip to clip_norm=1.0, got norm={norm}"
    );
}

#[test]
fn unknown_privacy_mechanism_override_panics_at_construction() {
    let mut overrides = Overrides {
        privacy_mechanism: Some("does_not_exist".to_string()),
        ..Default::default()
    };
    overrides.round_timeout_secs.get_or_insert(5);
    let config = conflux_config::resolve(
        Topology::CrossDevice,
        Mode::Research,
        Some(("test", &overrides)),
        &Overrides::default(),
        &Overrides::default(),
    )
    .unwrap();

    let result = std::panic::catch_unwind(|| AppState::new(config, vec![0.0]));
    assert!(
        result.is_err(),
        "an unregistered privacy mechanism name must fail loudly at construction"
    );
}
