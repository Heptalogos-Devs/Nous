// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;

#[test]
fn weighted_representatives_match_expanded_sample_pca() {
    let representatives = [
        vec![1.0, 0.0, 0.0],
        vec![0.0, 1.0, 0.0],
        vec![0.0, 0.0, 1.0],
    ];
    let multiplicities = [240usize, 40, 20];
    let mut expanded = Vec::new();
    let mut key = 0;
    for (vector, count) in representatives.iter().zip(multiplicities) {
        for _ in 0..count {
            expanded.push((key, vector.clone()));
            key += 1;
        }
    }
    let compressed = build_epa_basis(&expanded, "test-space", &EpaPolicy::reference())
        .expect("compressed EPA basis");
    let explicit_weights = vec![1.0; expanded.len()];
    let explicit = build_weighted_epa_basis(
        &expanded,
        &explicit_weights,
        "test-space",
        representatives[0].len(),
        &EpaPolicy::reference(),
    )
    .expect("expanded EPA basis");

    assert_eq!(compressed.mean, explicit.mean);
    assert_eq!(compressed.basis.len(), explicit.basis.len());
    for (compressed_axis, explicit_axis) in compressed.basis.iter().zip(&explicit.basis) {
        let alignment = compressed_axis
            .iter()
            .zip(explicit_axis)
            .map(|(left, right)| left * right)
            .sum::<f64>()
            .abs();
        assert!(alignment > 1.0 - 1e-10, "axis alignment={alignment}");
    }
    for (compressed_energy, explicit_energy) in compressed
        .singular_energies
        .iter()
        .zip(&explicit.singular_energies)
    {
        assert!((compressed_energy - explicit_energy).abs() < 1e-10);
    }
}
