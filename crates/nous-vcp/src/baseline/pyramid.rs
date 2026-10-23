// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use crate::{Error, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ReferenceResidualCandidate {
    pub id: i64,
    pub name: String,
    pub vector: Vec<f32>,
    pub similarity: f64,
}
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ReferencePyramidConfig {
    pub max_levels: usize,
    pub top_k: usize,
    pub min_energy_ratio: f64,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReferencePyramidTag {
    pub id: i64,
    pub name: String,
    pub similarity: f64,
    pub contribution: f64,
    pub handshake_magnitude: f64,
}
#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReferenceHandshake {
    pub direction_coherence: f64,
    pub pattern_strength: f64,
    pub novelty_signal: f64,
    pub noise_signal: f64,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReferencePyramidLevel {
    pub level: usize,
    pub tags: Vec<ReferencePyramidTag>,
    pub projection_magnitude: f64,
    pub residual_magnitude: f64,
    pub residual_energy_ratio: f64,
    pub energy_explained: f64,
    pub handshake_features: ReferenceHandshake,
}
#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReferencePyramidFeatures {
    pub depth: usize,
    pub coverage: f64,
    pub novelty: f64,
    pub coherence: f64,
    pub activation: f64,
    pub expansion_signal: f64,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReferencePyramid {
    pub levels: Vec<ReferencePyramidLevel>,
    pub total_explained_energy: f64,
    pub features: ReferencePyramidFeatures,
}
fn norm(vector: &[f32]) -> f64 {
    vector
        .iter()
        .map(|x| f64::from(*x).powi(2))
        .sum::<f64>()
        .sqrt()
}
fn dot(a: &[f64], b: &[f64]) -> f64 {
    a.iter().zip(b).map(|(a, b)| a * b).sum()
}
fn project(query: &[f32], tags: &[ReferenceResidualCandidate]) -> (Vec<f32>, Vec<f32>, Vec<f64>) {
    let mut units = Vec::<Vec<f64>>::new();
    let mut contributions = vec![0.0; tags.len()];
    let query64 = query.iter().map(|x| f64::from(*x)).collect::<Vec<_>>();
    for (index, tag) in tags.iter().enumerate() {
        let mut axis = tag.vector.iter().map(|x| f64::from(*x)).collect::<Vec<_>>();
        for unit in &units {
            let gain = dot(&axis, unit);
            for (v, u) in axis.iter_mut().zip(unit) {
                *v -= gain * u;
            }
        }
        let magnitude = dot(&axis, &axis).sqrt();
        if magnitude <= 1e-12 {
            continue;
        }
        for v in &mut axis {
            *v /= magnitude;
        }
        contributions[index] = dot(&query64, &axis).abs();
        units.push(axis);
    }
    let mut projection = vec![0.0f32; query.len()];
    for axis in units {
        let gain = dot(&query64, &axis);
        for (value, axis_value) in projection.iter_mut().zip(axis) {
            *value += (gain * axis_value) as f32;
        }
    }
    let residual = query.iter().zip(&projection).map(|(q, p)| q - p).collect();
    (projection, residual, contributions)
}
fn handshakes(
    query: &[f32],
    tags: &[ReferenceResidualCandidate],
) -> (ReferenceHandshake, Vec<f64>) {
    let mut mean = vec![0.0; query.len()];
    let mut directions = Vec::new();
    let mut magnitudes = Vec::new();
    for tag in tags {
        let mut direction = query
            .iter()
            .zip(&tag.vector)
            .map(|(q, t)| f64::from(q - t))
            .collect::<Vec<_>>();
        let distance = dot(&direction, &direction).sqrt();
        magnitudes.push(distance);
        if distance > 1e-9 {
            for (value, total) in direction.iter_mut().zip(&mut mean) {
                *value /= distance;
                *total += *value;
            }
        }
        directions.push(direction);
    }
    for value in &mut mean {
        *value /= tags.len() as f64;
    }
    let coherence = dot(&mean, &mean).sqrt().clamp(0.0, 1.0);
    let mut pairs = 0;
    let mut alignment = 0.0;
    for (index, left) in directions.iter().take(5).enumerate() {
        for right in directions.iter().take(5).skip(index + 1) {
            pairs += 1;
            alignment += dot(left, right).abs();
        }
    }
    let pattern = if pairs > 0 {
        (alignment / pairs as f64).clamp(0.0, 1.0)
    } else {
        0.0
    };
    (
        ReferenceHandshake {
            direction_coherence: coherence,
            pattern_strength: pattern,
            novelty_signal: coherence,
            noise_signal: (1.0 - coherence) * (1.0 - pattern),
        },
        magnitudes,
    )
}

/// Search is supplied by the adapter and cannot mutate prepared base hits.
/// Candidate order is part of the input to the orthogonalization contract.
pub fn reference_pyramid(
    query: &[f32],
    config: &ReferencePyramidConfig,
    mut search: impl FnMut(&[f32], usize) -> Result<Vec<ReferenceResidualCandidate>>,
) -> Result<ReferencePyramid> {
    let original = norm(query).powi(2);
    let mut levels = Vec::new();
    let mut total = 0.0;
    if original <= 1e-12 {
        return Ok(ReferencePyramid {
            levels,
            total_explained_energy: 0.0,
            features: ReferencePyramidFeatures {
                novelty: 1.0,
                expansion_signal: 1.0,
                ..Default::default()
            },
        });
    }
    let mut residual = query.to_vec();
    for level in 0..config.max_levels.clamp(1, 8) {
        let tags = search(&residual, config.top_k.clamp(1, 128))?;
        if tags.is_empty() {
            break;
        }
        if tags.iter().any(|t| t.vector.len() != query.len()) {
            return Err(Error::Invalid(
                "reference residual dimension mismatch".into(),
            ));
        }
        let current = norm(&residual).powi(2);
        let (projection, next, contributions) = project(&residual, &tags);
        let (handshake, magnitudes) = handshakes(&residual, &tags);
        let residual_magnitude = norm(&next);
        let residual_energy = residual_magnitude.powi(2);
        let explained = (current - residual_energy).max(0.0) / original;
        total += explained;
        levels.push(ReferencePyramidLevel {
            level,
            tags: tags
                .iter()
                .zip(contributions)
                .zip(magnitudes)
                .map(
                    |((tag, contribution), handshake_magnitude)| ReferencePyramidTag {
                        id: tag.id,
                        name: tag.name.clone(),
                        similarity: tag.similarity,
                        contribution,
                        handshake_magnitude,
                    },
                )
                .collect(),
            projection_magnitude: norm(&projection),
            residual_magnitude,
            residual_energy_ratio: residual_energy / original,
            energy_explained: explained,
            handshake_features: handshake,
        });
        residual = next;
        if residual_energy / original < config.min_energy_ratio.clamp(0.0, 1.0) {
            break;
        }
    }
    let coverage = total.clamp(0.0, 1.0);
    let first = levels.first().map(|l| &l.handshake_features);
    let novelty =
        ((1.0 - coverage) * 0.7 + first.map_or(0.0, |f| f.novelty_signal) * 0.3).clamp(0.0, 1.0);
    let coherence = first.map_or(0.0, |f| f.pattern_strength);
    let activation =
        (coverage * coherence * (1.0 - first.map_or(0.0, |f| f.noise_signal))).clamp(0.0, 1.0);
    let features = ReferencePyramidFeatures {
        depth: levels.len(),
        coverage,
        novelty,
        coherence,
        activation,
        expansion_signal: novelty,
    };
    Ok(ReferencePyramid {
        levels,
        total_explained_energy: total,
        features,
    })
}
