// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use crate::*;

pub fn bounded_restart_field(
    graph: &WaveGraphGeneration,
    source: &SparseField,
    alpha: f64,
    iterations: usize,
) -> SparseField {
    let mut current = normalize_field(source);
    if current.is_empty() {
        return current;
    }
    for _ in 0..iterations {
        let mut next = SparseField::new();
        for (&node, &mass) in &current {
            for (target, conductance) in graph.outgoing(node) {
                *next.entry(target).or_default() += alpha * mass * conductance;
            }
        }
        for (&node, &mass) in source {
            *next.entry(node).or_default() += (1.0 - alpha) * mass;
        }
        current = normalize_field(&next);
    }
    max_normalize(&current)
}

pub fn local_and_transfer_fields(
    graph: &WaveGraphGeneration,
    source: &SparseField,
) -> (SparseField, SparseField) {
    (
        bounded_restart_field(graph, source, 0.35, 4),
        bounded_restart_field(graph, source, 0.72, 8),
    )
}

pub(crate) fn normalize_field(field: &SparseField) -> SparseField {
    let total: f64 = field.values().copied().filter(|value| *value > 0.0).sum();
    if total <= 0.0 {
        return SparseField::new();
    }
    field
        .iter()
        .filter(|(_, value)| **value > 0.0)
        .map(|(&node, &value)| (node, value / total))
        .collect()
}

pub(crate) fn max_normalize(field: &SparseField) -> SparseField {
    let max = field.values().copied().fold(0.0, f64::max);
    if max <= 0.0 {
        return SparseField::new();
    }
    field
        .iter()
        .map(|(&node, &value)| (node, value / max))
        .collect()
}
