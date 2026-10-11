// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use nous_core::{Error, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
pub struct ReferenceFieldProjectionInput {
    pub dimension: usize,
    pub field: Vec<(i64, f64)>,
    pub tag_vectors: Vec<(i64, Vec<f32>)>,
}

/// Project a field in its artifact node order using available index vectors.
pub fn reference_field_projection(input: &ReferenceFieldProjectionInput) -> Vec<f32> {
    let vectors = input
        .tag_vectors
        .iter()
        .map(|(id, vector)| (*id, vector))
        .collect::<std::collections::BTreeMap<_, _>>();
    let mut vector = vec![0.0; input.dimension];
    let mut total = 0.0;
    for (id, mass) in &input.field {
        if *mass <= 0.0 {
            continue;
        }
        let Some(source) = vectors.get(id).filter(|v| v.len() == input.dimension) else {
            continue;
        };
        total += mass;
        for (result, value) in vector.iter_mut().zip(source.iter()) {
            *result += f64::from(*value) * mass;
        }
    }
    if total > 0.0 {
        for value in &mut vector {
            *value /= total;
        }
        let magnitude = vector.iter().map(|v| v * v).sum::<f64>().sqrt();
        if magnitude > 1e-12 {
            for value in &mut vector {
                *value /= magnitude;
            }
        }
    }
    vector.into_iter().map(|v| v as f32).collect()
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ReferenceTransport {
    pub node_ids: Vec<i64>,
    pub row_offsets: Vec<usize>,
    pub targets: Vec<usize>,
    pub weights: Vec<f64>,
}

impl ReferenceTransport {
    pub(crate) fn validate(&self) -> Result<()> {
        if self.row_offsets.len() != self.node_ids.len() + 1
            || self.row_offsets.first() != Some(&0)
            || self.row_offsets.windows(2).any(|pair| pair[0] > pair[1])
            || self.row_offsets.last() != Some(&self.targets.len())
            || self.targets.len() != self.weights.len()
            || self
                .targets
                .iter()
                .any(|index| *index >= self.node_ids.len())
            || self
                .weights
                .iter()
                .any(|weight| !weight.is_finite() || *weight < 0.0)
        {
            return Err(Error::new(
                nous_core::ErrorOwner::Resource,
                nous_core::ErrorCode::InvalidRequest,
                "invalid reference transport",
            ));
        }
        Ok(())
    }
    fn transfer(&self, field: &[f64]) -> Vec<f64> {
        let mut result = vec![0.0; field.len()];
        for (index, mass) in field.iter().enumerate() {
            for cursor in self.row_offsets[index]..self.row_offsets[index + 1] {
                result[self.targets[cursor]] += mass * self.weights[cursor];
            }
        }
        result
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ReferenceFieldConfig {
    pub local_alpha: f64,
    pub transfer_alpha: f64,
    pub max_iterations: usize,
    pub local_tolerance: f64,
    pub transfer_tolerance: f64,
    pub local_mass_ratio: f64,
    pub transfer_mass_ratio: f64,
}

#[derive(Debug, Serialize)]
pub struct ReferenceDualFields {
    pub local_field: Vec<(i64, f64)>,
    pub transfer_field: Vec<(i64, f64)>,
    pub local_domain: Vec<i64>,
    pub transfer_domain: Vec<i64>,
    pub iterations: usize,
    pub local_converged: bool,
    pub transfer_converged: bool,
    pub local_residual: f64,
    pub transfer_residual: f64,
}

fn supported_domain(nodes: &[i64], field: &[f64], ratio: f64) -> Vec<i64> {
    let mut ordered = nodes
        .iter()
        .copied()
        .zip(field.iter().copied())
        .filter(|(_, mass)| *mass > 0.0)
        .collect::<Vec<_>>();
    ordered.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    let total: f64 = field.iter().sum();
    let mut mass = 0.0;
    let mut ids = Vec::new();
    for (id, value) in ordered {
        ids.push(id);
        mass += value;
        if mass / total >= ratio.clamp(0.01, 1.0) {
            break;
        }
    }
    ids
}

/// Same source and transport, two independently converging resolvent scales.
pub fn reference_dual_fields(
    graph: &ReferenceTransport,
    source: &[(i64, f64)],
    config: &ReferenceFieldConfig,
) -> Result<ReferenceDualFields> {
    graph.validate()?;
    let mut input = vec![0.0; graph.node_ids.len()];
    for (id, mass) in source {
        if mass.is_finite()
            && *mass > 0.0
            && let Some(index) = graph.node_ids.iter().position(|node| node == id)
        {
            input[index] += mass;
        }
    }
    let total: f64 = input.iter().sum();
    if total > 0.0 {
        for mass in &mut input {
            *mass /= total;
        }
    }
    let mut fields = [input.clone(), input.clone()];
    let alpha = [
        config.local_alpha.clamp(0.0, 0.999999),
        config.transfer_alpha.clamp(0.0, 0.999999),
    ];
    let tolerance = [
        config.local_tolerance.max(1e-15),
        config.transfer_tolerance.max(1e-15),
    ];
    let mut converged = [total <= 0.0; 2];
    let mut residual = [0.0; 2];
    let mut iterations = 0;
    if total > 0.0 {
        for iteration in 1..=config.max_iterations.max(1) {
            iterations = iteration;
            for scale in 0..2 {
                if converged[scale] {
                    continue;
                }
                let transported = graph.transfer(&fields[scale]);
                let next = input
                    .iter()
                    .zip(transported)
                    .map(|(source, flow)| (1.0 - alpha[scale]) * source + alpha[scale] * flow)
                    .collect::<Vec<_>>();
                residual[scale] = next
                    .iter()
                    .zip(&fields[scale])
                    .map(|(a, b)| (a - b).abs())
                    .sum();
                converged[scale] = residual[scale] <= tolerance[scale];
                fields[scale] = next;
            }
            if converged.iter().all(|value| *value) {
                break;
            }
        }
    }
    let entries = |field: &[f64]| {
        graph
            .node_ids
            .iter()
            .copied()
            .zip(field.iter().copied())
            .filter(|(_, mass)| *mass > 0.0)
            .collect()
    };
    Ok(ReferenceDualFields {
        local_field: entries(&fields[0]),
        transfer_field: entries(&fields[1]),
        local_domain: supported_domain(&graph.node_ids, &fields[0], config.local_mass_ratio),
        transfer_domain: supported_domain(&graph.node_ids, &fields[1], config.transfer_mass_ratio),
        iterations,
        local_converged: converged[0],
        transfer_converged: converged[1],
        local_residual: residual[0],
        transfer_residual: residual[1],
    })
}

#[derive(Debug, Deserialize)]
pub struct ReferenceEpaInput {
    pub query: Vec<f32>,
    pub mean: Vec<f32>,
    pub basis: Vec<Vec<f32>>,
}
#[derive(Debug, Serialize)]
pub struct ReferenceEpaAnalysis {
    pub logic_depth: f64,
    pub entropy: f64,
    pub resonance: f64,
    pub axis_probabilities: Vec<f64>,
    pub cache_available: bool,
}

pub fn reference_epa_analysis(input: &ReferenceEpaInput) -> Result<ReferenceEpaAnalysis> {
    let dim = input.query.len();
    if dim != input.mean.len() || input.basis.iter().any(|axis| axis.len() != dim) {
        return Err(Error::new(
            nous_core::ErrorOwner::Resource,
            nous_core::ErrorCode::InvalidRequest,
            "reference EPA dimension mismatch",
        ));
    }
    let centered = input
        .query
        .iter()
        .zip(&input.mean)
        .map(|(q, m)| q - m)
        .collect::<Vec<_>>();
    let energy = input
        .basis
        .iter()
        .map(|axis| {
            let projection: f64 = centered
                .iter()
                .zip(axis)
                .map(|(a, b)| f64::from(*a) * f64::from(*b))
                .sum();
            projection * projection
        })
        .collect::<Vec<_>>();
    let total: f64 = energy.iter().sum();
    if total <= 1e-12 || input.basis.is_empty() {
        return Ok(ReferenceEpaAnalysis {
            logic_depth: 0.0,
            entropy: 1.0,
            resonance: 0.0,
            axis_probabilities: Vec::new(),
            cache_available: false,
        });
    }
    let probabilities = energy.iter().map(|e| e / total).collect::<Vec<_>>();
    let entropy = if probabilities.len() > 1 {
        (probabilities
            .iter()
            .filter(|p| **p > 1e-9)
            .map(|p| -p * p.log2())
            .sum::<f64>()
            / (probabilities.len() as f64).log2())
        .clamp(0.0, 1.0)
    } else {
        0.0
    };
    let mut dominant = probabilities
        .iter()
        .copied()
        .filter(|p| *p > 0.05)
        .collect::<Vec<_>>();
    dominant.sort_by(|a, b| b.total_cmp(a));
    let resonance = dominant.first().map_or(0.0, |primary| {
        dominant
            .iter()
            .skip(1)
            .map(|p| (primary * p).sqrt())
            .filter(|strength| *strength > 0.15)
            .sum()
    });
    Ok(ReferenceEpaAnalysis {
        logic_depth: 1.0 - entropy,
        entropy,
        resonance,
        axis_probabilities: probabilities,
        cache_available: true,
    })
}
