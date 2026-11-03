use super::ReferenceFileTags;
use nalgebra::DMatrix;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Deserialize)]
pub struct ReferenceIntrinsicInput {
    pub dimension: usize,
    pub tag_vectors: Vec<(i64, Vec<f32>)>,
    pub files: Vec<ReferenceFileTags>,
    pub pairwise: Vec<(i64, i64, f64)>,
    pub config: ReferenceIntrinsicConfig,
}
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(default, rename_all = "camelCase")]
pub struct ReferenceIntrinsicConfig {
    pub method: String,
    pub max_neighbors: usize,
    pub max_basis: usize,
    pub min_neighbors: usize,
    pub semantic_enabled: bool,
    pub semantic_peak: f64,
    pub semantic_sigma: f64,
    pub semantic_floor: f64,
    pub semantic_hard_floor: f64,
    pub min_gain: f64,
    pub position_decay: f64,
    pub v9_anchor_base: f64,
    pub v9_anchor_scale: f64,
    pub v9_anchor_gamma: f64,
    pub v9_anchor_min: f64,
    pub v9_anchor_max: f64,
}
impl Default for ReferenceIntrinsicConfig {
    fn default() -> Self {
        Self {
            method: "anchored_gs".into(),
            max_neighbors: 48,
            max_basis: 4,
            min_neighbors: 3,
            semantic_enabled: true,
            semantic_peak: 0.65,
            semantic_sigma: 0.25,
            semantic_floor: 0.35,
            semantic_hard_floor: -1.0,
            min_gain: 0.015,
            position_decay: 0.15,
            v9_anchor_base: 0.75,
            v9_anchor_scale: 1.25,
            v9_anchor_gamma: 1.0,
            v9_anchor_min: 0.5,
            v9_anchor_max: 2.0,
        }
    }
}
impl ReferenceIntrinsicConfig {
    fn normalized(&self) -> Self {
        let mut c = self.clone();
        c.method = match c.method.trim().to_ascii_lowercase().as_str() {
            "centroid" => "centroid",
            "svd" => "svd",
            _ => "anchored_gs",
        }
        .into();
        c.max_neighbors = c.max_neighbors.clamp(4, 256);
        c.max_basis = c.max_basis.clamp(1, 32);
        c.min_neighbors = c.min_neighbors.clamp(1, 64);
        c.semantic_peak = c.semantic_peak.clamp(-1.0, 1.0);
        c.semantic_sigma = c.semantic_sigma.clamp(0.02, 2.0);
        c.semantic_floor = c.semantic_floor.clamp(0.0, 1.0);
        c.semantic_hard_floor = c.semantic_hard_floor.clamp(-1.0, 1.0);
        c.min_gain = c.min_gain.clamp(0.0, 1.0);
        c.position_decay = c.position_decay.clamp(0.0, 4.0);
        c.v9_anchor_base = c.v9_anchor_base.clamp(0.0, 4.0);
        c.v9_anchor_scale = c.v9_anchor_scale.clamp(0.0, 4.0);
        c.v9_anchor_gamma = c.v9_anchor_gamma.clamp(0.1, 8.0);
        let a = c.v9_anchor_min.clamp(0.0, 4.0);
        let b = c.v9_anchor_max.clamp(0.0, 8.0);
        c.v9_anchor_min = a.min(b);
        c.v9_anchor_max = a.max(b);
        c
    }
}
#[derive(Debug, Serialize, Deserialize)]
pub struct ReferenceIntrinsicResult {
    pub id: i64,
    pub raw_residual_ratio: Option<f64>,
    pub anchor_gain: Option<f64>,
    pub neighbor_count: usize,
    pub status: String,
}
fn dot(a: &[f64], b: &[f64]) -> f64 {
    a.iter().zip(b).map(|(a, b)| a * b).sum()
}
fn norm(v: &[f64]) -> f64 {
    dot(v, v).sqrt()
}
fn normalized_vectors(input: &ReferenceIntrinsicInput) -> BTreeMap<i64, Vec<f32>> {
    input
        .tag_vectors
        .iter()
        .filter(|(_, v)| v.len() == input.dimension)
        .map(|(id, v)| {
            let magnitude = v.iter().map(|x| f64::from(*x).powi(2)).sum::<f64>().sqrt();
            (
                *id,
                v.iter()
                    .map(|x| {
                        if magnitude > 1e-9 {
                            (f64::from(*x) / magnitude) as f32
                        } else {
                            *x
                        }
                    })
                    .collect(),
            )
        })
        .collect()
}
fn adjacency(
    input: &ReferenceIntrinsicInput,
    c: &ReferenceIntrinsicConfig,
) -> BTreeMap<i64, BTreeMap<i64, f64>> {
    let mut graph = BTreeMap::<i64, BTreeMap<i64, f64>>::new();
    for file in &input.files {
        if file.tags.len() < 2 || file.tags.len() > 100 {
            continue;
        }
        for (i, (a, ap)) in file.tags.iter().enumerate() {
            for (j, (b, bp)) in file.tags.iter().enumerate() {
                if i == j || a == b {
                    continue;
                }
                let distance = if *ap > 0 && *bp > 0 {
                    (*ap - *bp).abs().max(1) as f64
                } else {
                    1.0
                };
                let weight = if c.position_decay > 0.0 {
                    (-c.position_decay * (distance - 1.0)).exp()
                } else {
                    1.0
                };
                *graph.entry(*a).or_default().entry(*b).or_default() += weight;
            }
        }
    }
    graph
}
fn semantic(sim: f64, c: &ReferenceIntrinsicConfig) -> f64 {
    if !c.semantic_enabled {
        1.0
    } else if !sim.is_finite() || sim <= 0.0 {
        c.semantic_floor
    } else if sim < c.semantic_hard_floor {
        0.0
    } else {
        (0.5 + 0.8
            * (-(sim - c.semantic_peak).powi(2) / (2.0 * c.semantic_sigma * c.semantic_sigma))
                .exp())
        .max(c.semantic_floor)
    }
}
struct Neighbor<'a> {
    vector: &'a [f32],
    weight: f64,
    semantic: f64,
}
fn centroid(tag: &[f64], neighbors: &[Neighbor<'_>]) -> Option<f64> {
    let mut mean = vec![0.0; tag.len()];
    let mut total = 0.0;
    for n in neighbors {
        let weight = n.weight * n.semantic;
        if weight <= 0.0 {
            continue;
        }
        total += weight;
        for (m, v) in mean.iter_mut().zip(n.vector) {
            *m += f64::from(*v) * weight;
        }
    }
    if total <= 1e-12 {
        return None;
    }
    for x in &mut mean {
        *x /= total;
    }
    let magnitude = norm(&mean);
    if magnitude <= 1e-9 {
        return None;
    }
    for x in &mut mean {
        *x /= magnitude;
    }
    Some(project_residual(tag, &[mean]))
}
fn project_residual(tag: &[f64], basis: &[Vec<f64>]) -> f64 {
    let coefficients = basis.iter().map(|b| dot(tag, b)).collect::<Vec<_>>();
    tag.iter()
        .enumerate()
        .map(|(i, v)| {
            let projection = basis
                .iter()
                .zip(&coefficients)
                .map(|(b, c)| b[i] * c)
                .sum::<f64>();
            (v - projection).powi(2)
        })
        .sum::<f64>()
        .sqrt()
}
fn anchored(tag: &[f64], neighbors: &[Neighbor<'_>], c: &ReferenceIntrinsicConfig) -> Option<f64> {
    let mut residual = tag.to_vec();
    let mut basis = Vec::<Vec<f64>>::new();
    let mut used = vec![false; neighbors.len()];
    for _ in 0..c.max_basis {
        let mut best = None;
        let mut best_score = 0.0;
        for (i, n) in neighbors.iter().enumerate() {
            if used[i] || n.semantic <= 0.0 {
                continue;
            }
            let mut axis = n.vector.iter().map(|x| f64::from(*x)).collect::<Vec<_>>();
            for b in &basis {
                let coefficient = dot(&axis, b);
                for (x, u) in axis.iter_mut().zip(b) {
                    *x -= coefficient * u;
                }
            }
            let magnitude = norm(&axis);
            if magnitude <= 1e-6 {
                continue;
            }
            for x in &mut axis {
                *x /= magnitude;
            }
            let gain = dot(&residual, &axis).abs();
            let score = gain * magnitude * (1.0 + n.weight).ln().max(1e-6) * n.semantic;
            if score > best_score {
                best_score = score;
                best = Some((i, gain, axis));
            }
        }
        let Some((i, gain, axis)) = best else {
            break;
        };
        if gain < c.min_gain {
            break;
        }
        used[i] = true;
        let coefficient = dot(&residual, &axis);
        for (x, u) in residual.iter_mut().zip(&axis) {
            *x -= coefficient * u;
        }
        basis.push(axis);
    }
    if basis.is_empty() {
        None
    } else {
        Some(norm(&residual))
    }
}
fn svd(tag: &[f64], neighbors: &[Neighbor<'_>], max_basis: usize) -> Option<f64> {
    let values = neighbors
        .iter()
        .flat_map(|n| n.vector.iter().copied())
        .collect::<Vec<_>>();
    let matrix = DMatrix::from_row_slice(neighbors.len(), tag.len(), &values);
    let decomposition = matrix.svd(false, true);
    let vt = decomposition.v_t?;
    let basis = (0..max_basis.min(neighbors.len()).min(tag.len()))
        .map(|i| (0..tag.len()).map(|j| f64::from(vt[(i, j)])).collect())
        .collect::<Vec<_>>();
    Some(project_residual(tag, &basis))
}

pub fn reference_intrinsic_residual(
    input: &ReferenceIntrinsicInput,
) -> Vec<ReferenceIntrinsicResult> {
    let config = input.config.normalized();
    let vectors = normalized_vectors(input);
    let graph = adjacency(input, &config);
    let pairwise = input
        .pairwise
        .iter()
        .map(|(a, b, s)| (((*a).min(*b), (*a).max(*b)), *s))
        .collect::<BTreeMap<_, _>>();
    vectors
        .iter()
        .map(|(id, vector)| {
            let mut neighbors = graph
                .get(id)
                .into_iter()
                .flat_map(|row| row.iter())
                .filter_map(|(other, weight)| {
                    let vector = vectors.get(other)?;
                    let sim = pairwise
                        .get(&((*id).min(*other), (*id).max(*other)))
                        .copied()
                        .unwrap_or(0.0);
                    let gate = semantic(sim, &config);
                    if gate <= 0.0 {
                        None
                    } else {
                        Some(Neighbor {
                            vector,
                            weight: *weight,
                            semantic: gate,
                        })
                    }
                })
                .collect::<Vec<_>>();
            // Stable tag-ID order resolves equal neighbor weights in this independent builder.
            // Frozen VCP's HashMap iteration does not define a stable tie order.
            neighbors.sort_by(|a, b| (b.weight * b.semantic).total_cmp(&(a.weight * a.semantic)));
            neighbors.truncate(config.max_neighbors);
            let count = neighbors.len();
            let ratio = if count < config.min_neighbors {
                None
            } else {
                let tag = vector.iter().map(|x| f64::from(*x)).collect::<Vec<_>>();
                match config.method.as_str() {
                    "centroid" => centroid(&tag, &neighbors),
                    "svd" => svd(&tag, &neighbors, config.max_basis),
                    _ => anchored(&tag, &neighbors, &config),
                }
            };
            let raw = ratio.map(|r| r.clamp(0.0, 1.0));
            let gain = raw.map(|r| {
                (config.v9_anchor_base + config.v9_anchor_scale * r.powf(config.v9_anchor_gamma))
                    .clamp(config.v9_anchor_min, config.v9_anchor_max)
            });
            ReferenceIntrinsicResult {
                id: *id,
                raw_residual_ratio: raw,
                anchor_gain: gain,
                neighbor_count: count,
                status: if count < config.min_neighbors {
                    "insufficient_neighbors"
                } else if raw.is_some() {
                    "computed"
                } else {
                    "failed"
                }
                .into(),
            }
        })
        .collect()
}
