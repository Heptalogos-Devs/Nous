// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use crate::*;

/// Replacement boundary for methods that discover weak semantic directions.
///
/// The contract speaks in semantic cue-sensing terms.  Physical algorithm
/// names stay in the concrete implementation, not in a public query protocol.
pub trait SemanticCueSensing: Send + Sync {
    fn sense(
        &self,
        query: &[f32],
        generation: &DenseGeneration,
        config: ResidualConfig,
    ) -> Result<Option<ResidualResult>>;
}

/// Default semantic cue sensing: EPA basis support plus bounded residual search.
#[derive(Debug, Default, Clone, Copy)]
pub struct EpaResidualCueSensing;

impl SemanticCueSensing for EpaResidualCueSensing {
    fn sense(
        &self,
        query: &[f32],
        generation: &DenseGeneration,
        config: ResidualConfig,
    ) -> Result<Option<ResidualResult>> {
        let query = query
            .iter()
            .map(|value| f64::from(*value))
            .collect::<Vec<_>>();
        residual_pyramid_with_search(&query, config, |residual, limit| {
            let residual = residual
                .iter()
                .map(|value| *value as f32)
                .collect::<Vec<_>>();
            generation
                .search(
                    &residual,
                    limit
                        .saturating_mul(config.candidate_oversampling)
                        .max(limit),
                )
                .map(|matches| tag_candidates(generation, matches, limit))
        })
    }
}

fn tag_candidates(
    generation: &DenseGeneration,
    matches: Vec<DenseMatch>,
    limit: usize,
) -> Vec<(u64, Vec<f64>)> {
    let mut result = Vec::new();
    for item in matches {
        let Some(record) = item.record else {
            continue;
        };
        if !matches!(record.reference, CognitiveRef::Tag(_)) {
            continue;
        }
        let Some(vector) = generation.vector(record.serving_doc_id) else {
            continue;
        };
        result.push((
            record.serving_doc_id,
            vector.iter().map(|value| f64::from(*value)).collect(),
        ));
        if result.len() >= limit {
            break;
        }
    }
    result
}

/// Replacement boundary for bounded associative expansion.
pub trait AssociativeExpansion: Send + Sync {
    fn expand(
        &self,
        graph: &WaveGraphGeneration,
        seeds: &[WeightedCognitiveSeed],
        rounds: usize,
        nodes: usize,
    ) -> QueryRiver;
}

/// Default bounded associative expansion.
#[derive(Debug, Default, Clone, Copy)]
pub struct BoundedWaveExpansion;

impl AssociativeExpansion for BoundedWaveExpansion {
    fn expand(
        &self,
        graph: &WaveGraphGeneration,
        seeds: &[WeightedCognitiveSeed],
        rounds: usize,
        nodes: usize,
    ) -> QueryRiver {
        propagate_weighted_with_budget(graph, seeds, rounds, nodes)
    }
}
