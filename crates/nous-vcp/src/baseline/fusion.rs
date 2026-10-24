use super::{ReferenceGatedTag, anchors::reference_cosine};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Deserialize)]
pub struct ReferenceFusionNode {
    pub id: i64,
    pub energy: f64,
}
#[derive(Debug, Clone, Deserialize)]
pub struct ReferenceFusionVector {
    pub id: i64,
    pub name: String,
    pub vector: Vec<f32>,
}
#[derive(Debug, Clone, Deserialize)]
pub struct ReferenceFusionGhost {
    pub name: String,
    pub vector: Vec<f32>,
    pub is_core: bool,
}
#[derive(Debug, Deserialize)]
pub struct ReferenceFusionInput {
    pub query: Vec<f32>,
    pub nodes: Vec<ReferenceFusionNode>,
    pub gated_tags: Vec<ReferenceGatedTag>,
    pub core_tags: Vec<String>,
    pub tag_vectors: Vec<ReferenceFusionVector>,
    pub ghosts: Vec<ReferenceFusionGhost>,
    pub dynamic_core_boost: f64,
    pub alpha: f64,
    pub config: ReferenceFusionConfig,
}
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(default, rename_all = "camelCase")]
pub struct ReferenceFusionConfig {
    pub core_boost_factor: f64,
    pub max_emergent_nodes: usize,
    pub deduplication_threshold: f64,
}
impl Default for ReferenceFusionConfig {
    fn default() -> Self {
        Self {
            core_boost_factor: 1.33,
            max_emergent_nodes: 50,
            deduplication_threshold: 0.88,
        }
    }
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReferenceFusionDiagnostics {
    pub requested_count: usize,
    pub found_count: usize,
    pub deduplicated_count: usize,
    pub total_weight: f64,
    pub selected_tag_ids: Vec<i64>,
    pub emergent_count: usize,
    pub supplemented_core_count: usize,
    pub ghost_count: usize,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReferenceFusionTag {
    pub id: i64,
    pub name: String,
    pub weight: f64,
    pub is_core: bool,
    pub vector: Vec<f32>,
}
#[derive(Debug, Serialize)]
pub struct ReferenceFusion {
    pub vector: Vec<f64>,
    pub diagnostics: ReferenceFusionDiagnostics,
    pub tags: Vec<ReferenceFusionTag>,
}
fn magnitude(v: &[f32]) -> f64 {
    v.iter().map(|x| f64::from(*x).powi(2)).sum::<f64>().sqrt()
}
fn normalize(v: &mut [f64]) {
    let mag = v.iter().map(|x| x * x).sum::<f64>().sqrt();
    if mag > 1e-12 {
        for x in v {
            *x /= mag;
        }
    }
}
fn merge(input: &ReferenceFusionInput) -> (Vec<ReferenceGatedTag>, usize, usize, f64) {
    let seeds = input
        .gated_tags
        .iter()
        .map(|t| (t.id, t))
        .collect::<BTreeMap<_, _>>();
    let mut merged = Vec::new();
    let mut emergent = Vec::new();
    for node in &input.nodes {
        if node.id <= 0 || node.energy <= 0.0 {
            continue;
        }
        if let Some(seed) = seeds.get(&node.id) {
            let mut tag = (*seed).clone();
            tag.weight = tag.weight.max(node.energy);
            merged.push(tag);
        } else {
            emergent.push(ReferenceGatedTag {
                id: node.id,
                name: String::new(),
                weight: node.energy,
                is_core: false,
            });
        }
    }
    emergent.sort_by(|a, b| b.weight.total_cmp(&a.weight).then_with(|| a.id.cmp(&b.id)));
    if input.config.max_emergent_nodes > 0 {
        emergent.truncate(input.config.max_emergent_nodes);
    }
    let emergent_count = emergent.len();
    merged.extend(emergent);
    let divisor = if input.config.core_boost_factor.is_finite()
        && input.config.core_boost_factor.abs() > 1e-12
    {
        input.config.core_boost_factor
    } else {
        1.33
    };
    let base = merged
        .iter()
        .map(|t| t.weight / divisor)
        .fold(0.0, f64::max)
        .max(if merged.is_empty() { 1.0 } else { 0.0 });
    let mut supplement = 0;
    for name in &input.core_tags {
        let lower = name.to_lowercase();
        if lower.is_empty()
            || merged
                .iter()
                .any(|t| !t.name.is_empty() && t.name.to_lowercase() == lower)
        {
            continue;
        }
        if let Some(source) = input
            .tag_vectors
            .iter()
            .find(|t| t.name.to_lowercase() == lower)
        {
            if merged.iter().any(|t| t.id == source.id) {
                continue;
            }
            merged.push(ReferenceGatedTag {
                id: source.id,
                name: source.name.clone(),
                weight: base * input.dynamic_core_boost,
                is_core: true,
            });
            supplement += 1;
        }
    }
    (merged, emergent_count, supplement, base)
}
fn vector_tags(
    input: &ReferenceFusionInput,
    merged: Vec<ReferenceGatedTag>,
    base: f64,
) -> (Vec<ReferenceFusionTag>, usize) {
    let vectors = input
        .tag_vectors
        .iter()
        .map(|v| (v.id, v))
        .collect::<BTreeMap<_, _>>();
    let mut output = Vec::new();
    for t in merged {
        if let Some(v) = vectors
            .get(&t.id)
            .filter(|v| v.vector.len() == input.query.len() && magnitude(&v.vector) > 1e-12)
        {
            output.push(ReferenceFusionTag {
                id: t.id,
                name: if t.name.is_empty() {
                    format!("Tag#{}", t.id)
                } else {
                    t.name
                },
                weight: t.weight,
                is_core: t.is_core,
                vector: v.vector.clone(),
            });
        }
    }
    let mut count = 0;
    for g in &input.ghosts {
        if g.vector.len() != input.query.len() || magnitude(&g.vector) <= 1e-12 {
            continue;
        }
        count += 1;
        output.push(ReferenceFusionTag {
            id: -(count as i64),
            name: g.name.clone(),
            weight: base
                * if g.is_core {
                    input.dynamic_core_boost
                } else {
                    1.0
                },
            is_core: g.is_core,
            vector: g.vector.clone(),
        });
    }
    (output, count)
}
pub fn reference_fuse_observation(input: &ReferenceFusionInput) -> ReferenceFusion {
    let (merged, emergent_count, supplemented_core_count, base) = merge(input);
    let requested_count = merged.len() + input.ghosts.len();
    let (mut vectors, ghost_count) = vector_tags(input, merged, base);
    let found_count = vectors.len();
    vectors.sort_by(|a, b| b.weight.total_cmp(&a.weight).then_with(|| a.id.cmp(&b.id)));
    let mut selected = Vec::<ReferenceFusionTag>::new();
    let threshold = input.config.deduplication_threshold.clamp(-1.0, 1.0);
    for candidate in vectors {
        let duplicate = selected.iter().position(|other| {
            let denom = magnitude(&candidate.vector) * magnitude(&other.vector);
            let similarity = if denom > 1e-12 {
                reference_cosine(&candidate.vector, &other.vector)
            } else {
                0.0
            };
            similarity > threshold
        });
        if let Some(i) = duplicate {
            selected[i].weight += 0.2 * candidate.weight;
            selected[i].is_core |= candidate.is_core;
        } else {
            selected.push(candidate);
        }
    }
    let total = selected.iter().map(|t| t.weight).sum::<f64>();
    let vector = if total <= 0.0 {
        selected.clear();
        input.query.iter().map(|v| f64::from(*v)).collect()
    } else {
        let mut context = vec![0.0; input.query.len()];
        for tag in &selected {
            for (x, v) in context.iter_mut().zip(&tag.vector) {
                *x += f64::from(*v) * tag.weight;
            }
        }
        for x in &mut context {
            *x /= total;
        }
        normalize(&mut context);
        let mut vector = input
            .query
            .iter()
            .zip(context)
            .map(|(q, c)| (1.0 - input.alpha) * f64::from(*q) + input.alpha * c)
            .collect::<Vec<_>>();
        normalize(&mut vector);
        vector
    };
    ReferenceFusion {
        vector,
        diagnostics: ReferenceFusionDiagnostics {
            requested_count,
            found_count,
            deduplicated_count: selected.len(),
            total_weight: if total <= 0.0 { 0.0 } else { total },
            selected_tag_ids: selected.iter().map(|t| t.id).collect(),
            emergent_count,
            supplemented_core_count,
            ghost_count,
        },
        tags: selected,
    }
}
