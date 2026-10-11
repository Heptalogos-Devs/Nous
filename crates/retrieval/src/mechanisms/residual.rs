// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use nous_core::Result;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct ResidualConfig {
    pub max_levels: usize,
    pub top_k_per_level: usize,
    pub stop_ratio: f64,
    pub collinear_epsilon: f64,
    pub candidate_oversampling: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SensedTag {
    pub tag_key: u64,
    pub coefficient: f64,
    pub seed_weight: f64,
    pub level: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResidualLevel {
    pub level: usize,
    pub original_energy: f64,
    pub explained_energy: f64,
    pub residual_energy: f64,
    pub sensed: Vec<SensedTag>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResidualResult {
    pub original_energy: f64,
    pub final_residual: Vec<f64>,
    pub levels: Vec<ResidualLevel>,
    pub stop_reason: String,
}

pub fn residual_pyramid(
    query: &[f64],
    tags: &[(u64, Vec<f64>)],
    config: ResidualConfig,
) -> Option<ResidualResult> {
    residual_pyramid_with_search(query, config, |residual, limit| {
        let mut candidates = tags
            .iter()
            .filter(|(_, vector)| vector.len() == query.len())
            .map(|(key, vector)| (*key, vector.clone(), cosine(residual, vector)))
            .collect::<Vec<_>>();
        candidates.sort_by(|left, right| {
            right
                .2
                .total_cmp(&left.2)
                .then_with(|| left.0.cmp(&right.0))
        });
        candidates.truncate(limit);
        Ok(candidates
            .into_iter()
            .map(|(key, vector, _)| (key, vector))
            .collect::<Vec<_>>())
    })
    .ok()
    .flatten()
}

/// Run the residual algorithm with a bounded candidate provider. Serving
/// callers can use ANN search for each residual instead of scanning all Tags.
pub fn residual_pyramid_with_search<F>(
    query: &[f64],
    config: ResidualConfig,
    mut search: F,
) -> Result<Option<ResidualResult>>
where
    F: FnMut(&[f64], usize) -> Result<Vec<(u64, Vec<f64>)>>,
{
    if query.is_empty() || query.iter().any(|value| !value.is_finite()) {
        return Ok(None);
    }
    let original_energy = dot(query, query);
    if original_energy <= f64::EPSILON {
        return Ok(None);
    }
    let mut residual = query.to_vec();
    let mut levels = Vec::new();
    let mut stop_reason = "max_levels".to_owned();
    for level in 0..config.max_levels {
        let residual_norm = dot(&residual, &residual).sqrt();
        if residual_norm * residual_norm / original_energy < config.stop_ratio {
            stop_reason = "energy_cutoff".into();
            break;
        }
        let candidates = search(&residual, config.top_k_per_level)?;
        if candidates.is_empty() {
            stop_reason = "no_candidates".into();
            break;
        }

        let mut basis = Vec::<Vec<f64>>::new();
        let mut basis_sources = Vec::<(u64, Vec<f64>)>::new();
        for (key, vector) in candidates {
            let mut direction = vector.clone();
            for previous in &basis {
                let coefficient = dot(&direction, previous);
                for (value, base) in direction.iter_mut().zip(previous) {
                    *value -= coefficient * base;
                }
            }
            let norm = dot(&direction, &direction).sqrt();
            if norm < config.collinear_epsilon {
                continue;
            }
            for value in &mut direction {
                *value /= norm;
            }
            basis.push(direction);
            basis_sources.push((key, vector));
        }
        if basis.is_empty() {
            stop_reason = "no_novel_basis_direction".into();
            break;
        }

        let before_energy = dot(&residual, &residual);
        let mut projection = vec![0.0; residual.len()];
        let mut sensed = Vec::new();
        for (axis, (key, source)) in basis.iter().zip(&basis_sources) {
            let coefficient = dot(&residual, axis);
            for (value, axis_value) in projection.iter_mut().zip(axis) {
                *value += coefficient * axis_value;
            }
            let contribution = coefficient.abs() / residual_norm.max(f64::EPSILON);
            let seed_weight = cosine(&residual, source).max(0.0) * contribution;
            sensed.push(SensedTag {
                tag_key: *key,
                coefficient: contribution,
                seed_weight,
                level,
            });
        }
        for (value, projected) in residual.iter_mut().zip(projection) {
            *value -= projected;
        }
        let residual_energy = dot(&residual, &residual).max(0.0);
        let explained_energy = (before_energy - residual_energy).max(0.0);
        levels.push(ResidualLevel {
            level,
            original_energy,
            explained_energy,
            residual_energy,
            sensed,
        });
        if residual_energy / original_energy < config.stop_ratio {
            stop_reason = "energy_cutoff".into();
            break;
        }
    }
    Ok(Some(ResidualResult {
        original_energy,
        final_residual: residual,
        levels,
        stop_reason,
    }))
}

pub fn cosine(left: &[f64], right: &[f64]) -> f64 {
    if left.len() != right.len() {
        return 0.0;
    }
    let denominator = dot(left, left).sqrt() * dot(right, right).sqrt();
    if denominator <= f64::EPSILON {
        0.0
    } else {
        dot(left, right) / denominator
    }
}

fn dot(left: &[f64], right: &[f64]) -> f64 {
    left.iter()
        .zip(right)
        .map(|(left, right)| left * right)
        .sum()
}

#[cfg(test)]
mod tests {
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
}
