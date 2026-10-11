// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;

#[test]
fn reference_values_match_the_frozen_profile() {
    let policy = AccessibilityPolicy::default();
    assert!((policy.activation(0.0, &[]) - 0.019802627).abs() < 1e-6);
    assert_eq!(
        policy.level_from_activation(policy.activation(180.0, &[])),
        AccessibilityLevel::Deep
    );
    assert_eq!(
        policy.level_from_activation(policy.activation(1000.0, &[])),
        AccessibilityLevel::Explicit
    );
}

#[test]
fn meaningful_use_weights_and_mode_gates_are_distinct() {
    let policy = AccessibilityPolicy::default();
    let base = policy.activation(0.0, &[]);
    for kind in [
        "referenced",
        "acted_on",
        "result_supported",
        "result_refuted",
        "corrected",
        "pinned",
    ] {
        assert!(policy.activation(0.0, &[(kind.into(), 0.0)]) > base);
    }
    assert_eq!(policy.activation(0.0, &[("presented".into(), 0.0)]), base);
    assert!(eligible(
        AccessibilityLevel::Deep,
        CognitiveEffort::Deep,
        false
    ));
    assert!(!eligible(
        AccessibilityLevel::Deep,
        CognitiveEffort::Normal,
        false
    ));
    assert!(!eligible(
        AccessibilityLevel::Explicit,
        CognitiveEffort::Maximum,
        false
    ));
    assert!(eligible(
        AccessibilityLevel::Explicit,
        CognitiveEffort::Light,
        true
    ));
}
