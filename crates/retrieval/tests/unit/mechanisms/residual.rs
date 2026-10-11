// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;

#[test]
fn orthogonal_weak_cue_survives_first_direction() {
    let result = residual_pyramid(
        &[1.0, 1.0, 0.0],
        &[(1, vec![1.0, 0.0, 0.0]), (2, vec![0.0, 1.0, 0.0])],
        ResidualConfig {
            max_levels: 3,
            stop_ratio: 0.01,
            top_k_per_level: 12,
            collinear_epsilon: 1e-6,
            candidate_oversampling: 4,
        },
    )
    .unwrap();
    assert!(!result.levels.is_empty());
    assert!(
        result
            .final_residual
            .iter()
            .map(|value| value * value)
            .sum::<f64>()
            < 1e-8
    );
    let keys = result
        .levels
        .iter()
        .flat_map(|level| level.sensed.iter().map(|tag| tag.tag_key))
        .collect::<Vec<_>>();
    assert!(keys.contains(&1) && keys.contains(&2));
}
