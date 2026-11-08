use super::unit;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReferenceDtscNode {
    pub id: i64,
    #[serde(default)]
    pub energy: f64,
    #[serde(default)]
    pub normalized_energy: f64,
    #[serde(default)]
    pub source_type: String,
}
#[derive(Debug, Deserialize)]
pub struct ReferenceDtscFieldInput {
    pub dimension: usize,
    pub nodes: Vec<ReferenceDtscNode>,
    pub source_field: Vec<(i64, f64)>,
    pub tag_vectors: Vec<(i64, Vec<f32>)>,
    pub inbound: Vec<(i64, f64)>,
    pub sample_tags: Vec<(i64, Vec<f32>)>,
    pub config: ReferenceDtscFieldConfig,
}
#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(default, rename_all = "camelCase")]
pub struct ReferenceDtscFieldConfig {
    pub min_field_tags: usize,
    pub min_field_entropy: f64,
    pub field_energy_mass_ratio: f64,
    pub max_field_nodes: usize,
    pub fallback_to_knn_on_low_trust: bool,
    pub field_similarity_threshold: f64,
    pub field_kernel_exponent: f64,
    pub max_field_neighbors: usize,
    pub public_hub_floor: f64,
}
impl Default for ReferenceDtscFieldConfig {
    fn default() -> Self {
        Self {
            min_field_tags: 3,
            min_field_entropy: 0.12,
            field_energy_mass_ratio: 0.95,
            max_field_nodes: 48,
            fallback_to_knn_on_low_trust: true,
            field_similarity_threshold: 0.5,
            field_kernel_exponent: 2.0,
            max_field_neighbors: 4,
            public_hub_floor: 0.35,
        }
    }
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReferenceDtscSample {
    pub id: i64,
    pub potential: f64,
    pub exact: bool,
    pub source_type: String,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReferenceDtscFieldNode {
    pub id: i64,
    pub potential: f64,
    pub vector: Vec<f32>,
    pub source_type: String,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReferenceDtscField {
    pub exact_field: Vec<ReferenceDtscSample>,
    pub field_nodes: Vec<ReferenceDtscFieldNode>,
    pub field_entropy: f64,
    pub field_trusted: bool,
    pub total_energy: f64,
    pub max_energy: f64,
    pub samples: Vec<ReferenceDtscSample>,
}
fn energy(n: &ReferenceDtscNode) -> f64 {
    if n.energy > 0.0 {
        n.energy
    } else {
        n.normalized_energy
    }
}
// Keep the reference's left-to-right f64 reduction. Reassociation, SIMD
// reductions, normalized f32 vectors, and reciprocal multiplication change bits.
fn squared_norm(v: &[f32]) -> f64 {
    v.iter().map(|x| f64::from(*x).powi(2)).sum::<f64>()
}

fn cosine_with_norms(a: &[f32], b: &[f32], aa: f64, bb: f64) -> f64 {
    if a.len() != b.len() || a.is_empty() || !(aa > 1e-12 && bb > 1e-12) {
        return 0.0;
    }
    let mut cross = 0.0;
    for (a, b) in a.iter().zip(b) {
        cross += f64::from(*a) * f64::from(*b);
    }
    // The DTSC guard is 1e-12, stronger than the former 1e-15 reference guard.
    // Do not clamp this value: infinity and NaN retain reference semantics.
    cross / (aa.sqrt() * bb.sqrt())
}

pub(crate) fn dtsc_cosine(a: &[f32], b: &[f32]) -> f64 {
    cosine_with_norms(a, b, squared_norm(a), squared_norm(b))
}

// Request-local derived data only: no changes to public fields or serialization.
// Borrowing prevents stale norms if a caller subsequently mutates the field.
pub(crate) struct DtscFieldSampler<'a> {
    field: &'a ReferenceDtscField,
    config: &'a ReferenceDtscFieldConfig,
    nodes: Vec<(f64, f64)>,
}
impl<'a> DtscFieldSampler<'a> {
    pub(crate) fn new(
        field: &'a ReferenceDtscField,
        inbound: &[(i64, f64)],
        config: &'a ReferenceDtscFieldConfig,
    ) -> Self {
        let maximum = inbound.iter().map(|(_, w)| *w).fold(0.0, f64::max);
        let nodes = field
            .field_nodes
            .iter()
            .map(|n| {
                let specific = if maximum <= 0.0 {
                    1.0
                } else {
                    config.public_hub_floor.clamp(0.05, 1.0).max(
                        1.0 - unit(
                            inbound
                                .iter()
                                .find(|(id, _)| *id == n.id)
                                .map_or(0.0, |(_, w)| *w)
                                / maximum,
                        )
                        .sqrt(),
                    )
                };
                (squared_norm(&n.vector), specific)
            })
            .collect();
        Self {
            field,
            config,
            nodes,
        }
    }

    pub(crate) fn sample(&self, id: i64, vector: &[f32]) -> ReferenceDtscSample {
        let field = self.field;
        let config = self.config;
        let exact = field.exact_field.iter().find(|s| s.id == id);
        let threshold = config.field_similarity_threshold.clamp(-1.0, 1.0);
        let vector_norm = squared_norm(vector);
        let mut neighbors = field
            .field_nodes
            .iter()
            .zip(&self.nodes)
            .filter(|(n, _)| n.id != id)
            .filter_map(|(n, &(node_norm, specific))| {
                let similarity = cosine_with_norms(vector, &n.vector, vector_norm, node_norm);
                if similarity < threshold {
                    return None;
                }
                let local = unit((similarity - threshold) / (1.0 - threshold).max(1e-6));
                Some((
                    n.potential * local.powf(config.field_kernel_exponent.max(0.25)) * specific,
                    n,
                ))
            })
            .collect::<Vec<_>>();
        // Stable ordering matters for equal scores and the selected source type.
        neighbors.sort_by(|a, b| b.0.total_cmp(&a.0));
        neighbors.truncate(config.max_field_neighbors.max(1));
        let interpolated = if neighbors.is_empty() {
            0.0
        } else {
            neighbors.iter().map(|(p, _)| *p).sum::<f64>() / (neighbors.len() as f64).sqrt()
        };
        ReferenceDtscSample {
            id,
            potential: unit(exact.map_or(0.0, |s| s.potential).max(interpolated)),
            exact: exact.is_some(),
            source_type: exact
                .map(|s| s.source_type.clone())
                .or_else(|| neighbors.first().map(|(_, n)| n.source_type.clone()))
                .unwrap_or_else(|| "unknown".into()),
        }
    }
}

pub fn reference_dtsc_field(input: &ReferenceDtscFieldInput) -> ReferenceDtscField {
    let config = &input.config;
    let minimum = config.min_field_tags.max(1);
    let mut nodes = if input.nodes.is_empty() {
        input
            .source_field
            .iter()
            .map(|(id, mass)| ReferenceDtscNode {
                id: *id,
                energy: if mass.is_finite() { mass.max(0.0) } else { 0.0 },
                normalized_energy: 0.0,
                source_type: "seed".into(),
            })
            .collect()
    } else {
        input.nodes.clone()
    };
    nodes.retain(|n| n.id > 0 && energy(n) > 0.0);
    nodes.sort_by(|a, b| {
        energy(b)
            .total_cmp(&energy(a))
            .then_with(|| a.id.cmp(&b.id))
    });
    let total_energy: f64 = nodes.iter().map(energy).sum();
    let max_energy = nodes.iter().map(energy).fold(0.0, f64::max);
    let field_entropy = if nodes.len() > 1 && total_energy > 0.0 {
        nodes
            .iter()
            .map(|n| {
                let p = energy(n) / total_energy;
                if p > 0.0 { -p * p.ln() } else { 0.0 }
            })
            .sum::<f64>()
            / (nodes.len() as f64).ln()
    } else {
        0.0
    };
    let distribution_trusted = nodes.len() >= minimum
        && total_energy > 0.0
        && (!config.fallback_to_knn_on_low_trust
            || field_entropy >= unit(config.min_field_entropy));
    let exact = nodes
        .iter()
        .map(|n| {
            (
                n.id,
                ReferenceDtscSample {
                    id: n.id,
                    potential: if max_energy > 0.0 {
                        unit(energy(n) / max_energy)
                    } else {
                        0.0
                    },
                    exact: true,
                    source_type: n.source_type.clone(),
                },
            )
        })
        .collect::<BTreeMap<_, _>>();
    let mut retained = 0;
    let mut mass = 0.0;
    for n in &nodes {
        if retained >= config.max_field_nodes.max(minimum)
            || (retained >= minimum
                && mass / total_energy.max(1e-12) >= config.field_energy_mass_ratio.clamp(0.5, 1.0))
        {
            break;
        }
        retained += 1;
        mass += energy(n);
    }
    let vectors = input
        .tag_vectors
        .iter()
        .filter(|(_, v)| v.len() == input.dimension)
        .map(|(id, v)| (*id, v))
        .collect::<BTreeMap<_, _>>();
    let field_nodes = nodes
        .iter()
        .take(retained)
        .filter_map(|n| {
            vectors.get(&n.id).map(|v| ReferenceDtscFieldNode {
                id: n.id,
                potential: if max_energy > 0.0 {
                    unit(energy(n) / max_energy)
                } else {
                    0.0
                },
                vector: (*v).clone(),
                source_type: n.source_type.clone(),
            })
        })
        .collect::<Vec<_>>();
    let field_trusted = distribution_trusted && field_nodes.len() >= minimum;
    let exact_field = exact.into_values().collect();
    let mut output = ReferenceDtscField {
        exact_field,
        field_nodes,
        field_entropy,
        field_trusted,
        total_energy,
        max_energy,
        samples: Vec::new(),
    };
    if !input.sample_tags.is_empty() {
        let sampler = DtscFieldSampler::new(&output, &input.inbound, config);
        output.samples = input
            .sample_tags
            .iter()
            .map(|(id, v)| sampler.sample(*id, v))
            .collect();
    }
    output
}

pub fn reference_dtsc_sample(
    id: i64,
    vector: &[f32],
    field: &ReferenceDtscField,
    inbound: &[(i64, f64)],
    config: &ReferenceDtscFieldConfig,
) -> ReferenceDtscSample {
    DtscFieldSampler::new(field, inbound, config).sample(id, vector)
}

#[cfg(test)]
mod sampler_exact_tests;
