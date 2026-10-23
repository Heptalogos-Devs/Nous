//! Complete query pipeline adapted from VCPToolBox memo_pipeline.rs.
//! Commit d1b40c863273cbbba082fb7657671424768c4a53, CC BY-NC-SA 4.0.
//! N-API, SQLite loads and host observation handles are replaced by typed snapshots.
// Retain the upstream numerical decomposition and floating-point accumulation order.
#![allow(
    clippy::type_complexity,
    clippy::too_many_arguments,
    clippy::needless_range_loop
)]

use crate::assets::{MemoAssets, TagSearch, TagVectorPool, ensure_tag_vectors_loaded};
use crate::sensing::{
    DynamicWormholeContext, SenseConfig, SenseDiagnostics, SenseInput, SenseOutput, SenseSeed,
    sense_prevalidated,
};
use base64::Engine;
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Instant;
const PIPELINE_SCHEMA: &str = "vexus-unified-memo-pipeline-v1";
const PIPELINE_ALGORITHM: &str = "tagmemo.epa-pyramid-spike-fusion-v9.4-calibrated-rust";

fn positive(value: f64) -> f64 {
    if value.is_finite() {
        value.max(0.0)
    } else {
        0.0
    }
}

fn clamp01(value: f64) -> f64 {
    if value.is_finite() {
        value.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

fn magnitude(vector: &[f32]) -> f64 {
    vector
        .iter()
        .map(|value| (*value as f64) * (*value as f64))
        .sum::<f64>()
        .sqrt()
}

fn dot(left: &[f32], right: &[f32]) -> f64 {
    left.iter()
        .zip(right.iter())
        .map(|(a, b)| (*a as f64) * (*b as f64))
        .sum()
}

fn cosine(left: &[f32], right: &[f32]) -> f64 {
    if left.len() != right.len() || left.is_empty() {
        return 0.0;
    }
    let denominator = magnitude(left) * magnitude(right);
    if denominator > 1e-12 {
        dot(left, right) / denominator
    } else {
        0.0
    }
}

fn normalize(vector: &mut [f64]) -> bool {
    let norm = vector.iter().map(|value| value * value).sum::<f64>().sqrt();
    if norm <= 1e-12 {
        return false;
    }
    for value in vector {
        *value /= norm;
    }
    true
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PipelineInput {
    #[serde(default)]
    pub query_id: Option<String>,
    #[serde(default, rename = "queryText")]
    pub _query_text: String,
    #[serde(default)]
    pub query_vector: Vec<f32>,
    #[serde(default)]
    pub core_tags: Vec<String>,
    #[serde(default)]
    pub ghost_tags: Vec<GhostTagInput>,
    #[serde(default)]
    pub config: PipelineConfig,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GhostTagInput {
    pub name: String,
    #[serde(default)]
    pub vector: Vec<f32>,
    #[serde(default)]
    pub is_core: bool,
}

#[derive(Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct PipelineConfig {
    pub base_tag_boost: f64,
    pub core_boost_factor: f64,
    pub local_alpha: f64,
    pub transfer_alpha: f64,
    pub field_max_iterations: usize,
    pub local_tolerance: f64,
    pub transfer_tolerance: f64,
    pub local_mass_ratio: f64,
    pub transfer_mass_ratio: f64,
    pub max_field_projection_nodes: usize,
    pub max_levels: usize,
    pub pyramid_top_k: usize,
    pub min_energy_ratio: f64,
    pub layer_decay: f64,
    pub activation_multiplier: Vec<f64>,
    pub dynamic_boost_range: Vec<f64>,
    pub core_boost_range: Vec<f64>,
    pub lang_confidence_enabled: bool,
    pub lang_penalty_unknown: f64,
    pub lang_penalty_cross_domain: f64,
    pub deduplication_threshold: f64,
    pub max_fusion_tags: usize,
    pub max_emergent_nodes: usize,
    pub tech_tag_threshold: f64,
    pub normal_tag_threshold: f64,
    pub epa_gate_mode: String,
    #[serde(default)]
    pub spike_routing: SenseConfig,
}

impl Default for PipelineConfig {
    fn default() -> Self {
        Self {
            base_tag_boost: 0.6,
            core_boost_factor: 1.33,
            local_alpha: 0.15,
            transfer_alpha: 0.55,
            field_max_iterations: 80,
            local_tolerance: 1e-9,
            transfer_tolerance: 1e-9,
            local_mass_ratio: 0.8,
            transfer_mass_ratio: 0.9,
            max_field_projection_nodes: 256,
            max_levels: 3,
            pyramid_top_k: 10,
            min_energy_ratio: 0.1,
            layer_decay: 0.7,
            activation_multiplier: vec![0.5, 1.5],
            dynamic_boost_range: vec![0.3, 2.0],
            core_boost_range: vec![1.2, 1.4],
            lang_confidence_enabled: true,
            lang_penalty_unknown: 0.05,
            lang_penalty_cross_domain: 0.2,
            deduplication_threshold: 0.88,
            max_fusion_tags: 128,
            max_emergent_nodes: 50,
            tech_tag_threshold: 0.08,
            normal_tag_threshold: 0.015,
            epa_gate_mode: "backgroundRelative".into(),
            spike_routing: SenseConfig::default(),
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct EpaCache {
    basis: Vec<String>,
    mean: String,
    #[serde(default, rename = "energies")]
    _energies: Vec<f64>,
    #[serde(default)]
    labels: Vec<String>,
    #[serde(default = "default_k_sem")]
    k_sem: usize,
    #[serde(default)]
    coverage: Vec<f64>,
}

fn default_k_sem() -> usize {
    8
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct EpaAxis {
    index: usize,
    label: String,
    energy: f64,
    projection: f64,
    score: f64,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct EpaBridge {
    from: String,
    to: String,
    strength: f64,
    balance: f64,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct EpaOutput {
    logic_depth: f64,
    entropy: f64,
    resonance: f64,
    dominant_axes: Vec<EpaAxis>,
    bridges: Vec<EpaBridge>,
    basis_count: usize,
    cache_available: bool,
    out_of_domain: f64,
    semantic_explained_ratio: f64,
    in_domain_gate: f64,
    baseline_coverage: f64,
    background_ratio: f64,
}

fn decode_f32_base64(encoded: &str, dimension: usize) -> Option<Vec<f32>> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .ok()?;
    if bytes.len() != dimension * std::mem::size_of::<f32>() {
        return None;
    }
    Some(
        bytes
            .chunks_exact(4)
            .map(|chunk| f32::from_ne_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
            .collect(),
    )
}

/// Background is the ratio of expected energies for an isotropic unit query after
/// subtracting the same mean. It is a calibration reference, not an OOD probability.
fn calibrated_gate(ratio: f64, baseline: f64, background: f64, mode: &str) -> f64 {
    match mode {
        "relativeLinear" => clamp01(ratio / baseline),
        "relativeSqrt" => clamp01((ratio / baseline).max(0.0).sqrt()),
        "backgroundRelative" => {
            if baseline <= background + 1e-8 {
                0.0
            } else {
                clamp01((ratio - background) / (baseline - background))
            }
        }
        _ => clamp01(ratio),
    }
}

/// Query-independent part of the EPA cache row: decoded basis and mean, and the
/// values derived from them alone.
pub struct EpaBasis {
    pub raw: String,
    pub dimension: usize,
    pub k_sem: usize,
    pub labels: Vec<String>,
    pub mean: Vec<f32>,
    pub bases: Vec<Vec<f32>>,
    pub semantic_count: usize,
    pub baseline_coverage: f64,
    pub background_ratio: f64,
}

/// Decode the immutable EPA publication supplied by the asset owner.
pub fn decode_epa_basis(raw: String, dimension: usize) -> Option<EpaBasis> {
    let cache: EpaCache = serde_json::from_str(&raw).ok()?;
    let mean = decode_f32_base64(&cache.mean, dimension)?;
    let bases: Vec<Vec<f32>> = cache
        .basis
        .iter()
        .filter_map(|encoded| decode_f32_base64(encoded, dimension))
        .collect();
    let semantic_count = cache.k_sem.max(1).min(bases.len());
    let baseline_coverage = cache
        .coverage
        .get(semantic_count.saturating_sub(1))
        .copied()
        .filter(|v| v.is_finite() && *v > 0.0 && *v <= 1.0)
        .unwrap_or(semantic_count as f64 / dimension.max(1) as f64)
        .max(1e-4);
    let mean_captured = bases
        .iter()
        .take(semantic_count)
        .map(|b| dot(&mean, b).powi(2))
        .sum::<f64>();
    let background_ratio = clamp01(
        (semantic_count as f64 / dimension.max(1) as f64 + mean_captured)
            / (1.0 + dot(&mean, &mean)),
    );
    Some(EpaBasis {
        raw,
        dimension,
        k_sem: cache.k_sem,
        labels: cache.labels,
        mean,
        bases,
        semantic_count,
        baseline_coverage,
        background_ratio,
    })
}

fn analyze_epa_with_mode(
    connection: &MemoAssets,
    query: &[f32],
    dimension: usize,
    mode: &str,
) -> EpaOutput {
    let empty = || EpaOutput {
        logic_depth: 0.0,
        entropy: 1.0,
        resonance: 0.0,
        dominant_axes: Vec::new(),
        bridges: Vec::new(),
        basis_count: 0,
        cache_available: false,
        out_of_domain: 1.0,
        semantic_explained_ratio: 0.0,
        in_domain_gate: 0.0,
        baseline_coverage: 0.0,
        background_ratio: 0.0,
    };
    let Some(basis) = connection.epa.as_ref() else {
        return empty();
    };
    if basis.dimension != dimension {
        return empty();
    }
    let mean = &basis.mean;
    let bases = &basis.bases;
    let centered: Vec<f32> = query
        .iter()
        .zip(mean.iter())
        .map(|(value, average)| value - average)
        .collect();
    let projections: Vec<f64> = bases.iter().map(|basis| dot(&centered, basis)).collect();
    let centered_energy = dot(&centered, &centered);
    let captured = projections.iter().map(|value| value * value).sum::<f64>();
    let out_of_domain = if centered_energy > 1e-12 {
        ((centered_energy - captured).max(0.0) / centered_energy)
            .sqrt()
            .clamp(0.0, 1.0)
    } else {
        0.0
    };
    let semantic_count = basis.semantic_count;
    let total_energy = projections
        .iter()
        .take(semantic_count)
        .map(|value| value * value)
        .sum::<f64>();
    let semantic_explained_ratio = if centered_energy > 1e-12 {
        clamp01(total_energy / centered_energy)
    } else {
        0.0
    };
    let baseline_coverage = basis.baseline_coverage;
    let background_ratio = basis.background_ratio;
    let in_domain_gate = if semantic_count == 0 || centered_energy <= 1e-12 {
        0.0
    } else {
        calibrated_gate(
            semantic_explained_ratio,
            baseline_coverage,
            background_ratio,
            mode,
        )
    };
    let probabilities: Vec<f64> = projections
        .iter()
        .take(semantic_count)
        .map(|value| {
            if total_energy > 1e-12 {
                value * value / total_energy
            } else {
                0.0
            }
        })
        .collect();
    let raw_entropy = probabilities
        .iter()
        .filter(|value| **value > 1e-9)
        .map(|value| -value * value.log2())
        .sum::<f64>();
    let normalized_entropy = if total_energy <= 1e-12 {
        1.0
    } else if basis.k_sem > 1 {
        clamp01(raw_entropy / (basis.k_sem as f64).log2())
    } else {
        0.0
    };
    let mut dominant_axes: Vec<EpaAxis> = probabilities
        .iter()
        .enumerate()
        .filter(|(_, energy)| **energy > 0.05)
        .map(|(index, energy)| EpaAxis {
            index,
            label: basis
                .labels
                .get(index)
                .cloned()
                .unwrap_or_else(|| "Unknown".to_string()),
            energy: *energy,
            projection: projections[index],
            score: *energy,
        })
        .collect();
    dominant_axes.sort_by(|left, right| {
        right
            .energy
            .partial_cmp(&left.energy)
            .unwrap_or(Ordering::Equal)
    });

    let mut bridges = Vec::new();
    if let Some(primary) = dominant_axes.first() {
        for secondary in dominant_axes.iter().skip(1) {
            let strength = (primary.energy * secondary.energy).sqrt();
            if strength > 0.15 {
                bridges.push(EpaBridge {
                    from: primary.label.clone(),
                    to: secondary.label.clone(),
                    strength,
                    balance: if primary.energy.max(secondary.energy) > 0.0 {
                        primary.energy.min(secondary.energy) / primary.energy.max(secondary.energy)
                    } else {
                        0.0
                    },
                });
            }
        }
    }
    let resonance = bridges.iter().map(|bridge| bridge.strength).sum();

    EpaOutput {
        logic_depth: (1.0 - normalized_entropy) * in_domain_gate,
        entropy: normalized_entropy,
        resonance,
        dominant_axes,
        bridges,
        basis_count: bases.len(),
        cache_available: true,
        out_of_domain,
        semantic_explained_ratio,
        in_domain_gate,
        baseline_coverage,
        background_ratio,
    }
}

#[derive(Clone)]
struct PyramidTagWork {
    id: i64,
    name: String,
    vector: Arc<[f32]>,
    similarity: f64,
    contribution: f64,
    handshake_magnitude: f64,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct PyramidTagOutput {
    id: i64,
    name: String,
    similarity: f64,
    contribution: f64,
    handshake_magnitude: f64,
}

#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct HandshakeFeatures {
    direction_coherence: f64,
    pattern_strength: f64,
    novelty_signal: f64,
    noise_signal: f64,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct PyramidLevelOutput {
    level: usize,
    tags: Vec<PyramidTagOutput>,
    projection_magnitude: f64,
    residual_magnitude: f64,
    residual_energy_ratio: f64,
    energy_explained: f64,
    handshake_features: HandshakeFeatures,
}

#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct PyramidFeatures {
    depth: usize,
    coverage: f64,
    novelty: f64,
    coherence: f64,
    activation: f64,
    expansion_signal: f64,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct PyramidOutput {
    levels: Vec<PyramidLevelOutput>,
    total_explained_energy: f64,
    features: PyramidFeatures,
}

fn orthogonal_projection(
    query: &[f32],
    tags: &[PyramidTagWork],
    dimension: usize,
) -> (Vec<f32>, Vec<f32>, Vec<f64>) {
    let mut basis: Vec<Vec<f64>> = Vec::with_capacity(tags.len());
    let mut coefficients = vec![0.0; tags.len()];
    for (index, tag) in tags.iter().enumerate() {
        let mut vector: Vec<f64> = tag.vector.iter().map(|value| *value as f64).collect();
        for unit in &basis {
            let projection = vector
                .iter()
                .zip(unit.iter())
                .map(|(left, right)| left * right)
                .sum::<f64>();
            for offset in 0..dimension {
                vector[offset] -= projection * unit[offset];
            }
        }
        if normalize(&mut vector) {
            coefficients[index] = query
                .iter()
                .zip(vector.iter())
                .map(|(left, right)| (*left as f64) * right)
                .sum::<f64>()
                .abs();
            basis.push(vector);
        }
    }

    let mut projection = vec![0.0f32; dimension];
    for unit in &basis {
        let coefficient = query
            .iter()
            .zip(unit.iter())
            .map(|(left, right)| (*left as f64) * right)
            .sum::<f64>();
        for offset in 0..dimension {
            projection[offset] += (coefficient * unit[offset]) as f32;
        }
    }
    let residual = query
        .iter()
        .zip(projection.iter())
        .map(|(source, explained)| source - explained)
        .collect();
    (projection, residual, coefficients)
}

fn analyze_handshakes(
    query: &[f32],
    tags: &mut [PyramidTagWork],
    dimension: usize,
) -> HandshakeFeatures {
    if tags.is_empty() {
        return HandshakeFeatures::default();
    }
    let mut directions = Vec::with_capacity(tags.len());
    let mut average = vec![0.0f64; dimension];
    for tag in tags.iter_mut() {
        let mut direction = vec![0.0f64; dimension];
        let mut magnitude_squared = 0.0;
        for offset in 0..dimension {
            let delta = (query[offset] - tag.vector[offset]) as f64;
            direction[offset] = delta;
            magnitude_squared += delta * delta;
        }
        let distance = magnitude_squared.sqrt();
        tag.handshake_magnitude = distance;
        if distance > 1e-9 {
            for offset in 0..dimension {
                direction[offset] /= distance;
                average[offset] += direction[offset];
            }
        }
        directions.push(direction);
    }
    for value in &mut average {
        *value /= tags.len() as f64;
    }
    let direction_coherence = clamp01(
        average
            .iter()
            .map(|value| value * value)
            .sum::<f64>()
            .sqrt(),
    );
    let limit = directions.len().min(5);
    let mut pairwise_sum = 0.0;
    let mut pair_count = 0usize;
    for left in 0..limit {
        for right in (left + 1)..limit {
            pairwise_sum += directions[left]
                .iter()
                .zip(directions[right].iter())
                .map(|(a, b)| a * b)
                .sum::<f64>()
                .abs();
            pair_count += 1;
        }
    }
    let pattern_strength = if pair_count > 0 {
        clamp01(pairwise_sum / pair_count as f64)
    } else {
        0.0
    };
    HandshakeFeatures {
        direction_coherence,
        pattern_strength,
        novelty_signal: direction_coherence,
        noise_signal: (1.0 - direction_coherence) * (1.0 - pattern_strength),
    }
}

fn load_tag_names(connection: &MemoAssets, ids: &[i64]) -> Result<HashMap<i64, String>, String> {
    Ok(ids
        .iter()
        .filter_map(|id| connection.tag_names.get(id).map(|name| (*id, name.clone())))
        .collect())
}

fn analyze_pyramid_with_pool(
    index: &dyn TagSearch,
    connection: &MemoAssets,
    query: &[f32],
    dimension: usize,
    config: &PipelineConfig,
    tag_vectors: &mut TagVectorPool,
) -> std::result::Result<(PyramidOutput, Vec<Vec<PyramidTagWork>>, Vec<Vec<f32>>), String> {
    let original_energy = magnitude(query).powi(2);
    if original_energy <= 1e-12 {
        return Ok((
            PyramidOutput {
                levels: Vec::new(),
                total_explained_energy: 0.0,
                features: PyramidFeatures {
                    novelty: 1.0,
                    expansion_signal: 1.0,
                    ..PyramidFeatures::default()
                },
            },
            Vec::new(),
            Vec::new(),
        ));
    }

    let mut residual = query.to_vec();
    let mut levels_output = Vec::new();
    let mut levels_work = Vec::new();
    let mut residuals = Vec::new();
    let mut total_explained = 0.0;
    for level in 0..config.max_levels.clamp(1, 8) {
        let matches = index.search(&residual, config.pyramid_top_k.clamp(1, 128))?;
        if matches
            .iter()
            .any(|(id, score)| !connection.tag_names.contains_key(id) || !score.is_finite())
        {
            return Err(
                "MEMO_TAG_SEARCH_INVALID: search returned an undeclared ID or non-finite score"
                    .into(),
            );
        }
        if matches.is_empty() {
            break;
        }
        let ids: Vec<i64> = matches.iter().map(|(id, _)| *id).collect();

        ensure_tag_vectors_loaded(&connection.tag_vectors, tag_vectors, &ids, dimension)?;
        let mut tags = Vec::new();
        for id in &ids {
            let Some(vector) = tag_vectors.get(id).cloned() else {
                continue;
            };
            let similarity = cosine(&residual, &vector);
            tags.push(PyramidTagWork {
                id: *id,
                name: String::new(),
                vector,
                similarity,
                contribution: 0.0,
                handshake_magnitude: 0.0,
            });
        }
        let names = load_tag_names(connection, &ids)?;
        for tag in &mut tags {
            tag.name = names
                .get(&tag.id)
                .cloned()
                .unwrap_or_else(|| format!("Tag#{}", tag.id));
        }
        if tags.is_empty() {
            break;
        }

        let current_energy = magnitude(&residual).powi(2);
        let (projection, next_residual, coefficients) =
            orthogonal_projection(&residual, &tags, dimension);
        for (tag, coefficient) in tags.iter_mut().zip(coefficients.iter()) {
            tag.contribution = *coefficient;
        }
        let handshake = analyze_handshakes(&residual, &mut tags, dimension);
        let residual_magnitude = magnitude(&next_residual);
        let residual_energy = residual_magnitude * residual_magnitude;
        let energy_explained = positive(current_energy - residual_energy) / original_energy;
        total_explained += energy_explained;
        levels_output.push(PyramidLevelOutput {
            level,
            tags: tags
                .iter()
                .map(|tag| PyramidTagOutput {
                    id: tag.id,
                    name: tag.name.clone(),
                    similarity: tag.similarity,
                    contribution: tag.contribution,
                    handshake_magnitude: tag.handshake_magnitude,
                })
                .collect(),
            projection_magnitude: magnitude(&projection),
            residual_magnitude,
            residual_energy_ratio: residual_energy / original_energy,
            energy_explained,
            handshake_features: handshake,
        });
        if config.spike_routing.dynamic_wormholes {
            residuals.push(residual.clone());
        }
        levels_work.push(tags);
        residual = next_residual;
        if residual_energy / original_energy < config.min_energy_ratio.clamp(0.0, 1.0) {
            break;
        }
    }

    let coverage = clamp01(total_explained);
    let first_handshake = levels_output
        .first()
        .map(|level| level.handshake_features.clone())
        .unwrap_or_default();
    let novelty = clamp01((1.0 - coverage) * 0.7 + first_handshake.novelty_signal * 0.3);
    let coherence = first_handshake.pattern_strength;
    let activation = clamp01(coverage * coherence * (1.0 - first_handshake.noise_signal));
    let features = PyramidFeatures {
        depth: levels_output.len(),
        coverage,
        novelty,
        coherence,
        activation,
        expansion_signal: novelty,
    };
    Ok((
        PyramidOutput {
            levels: levels_output,
            total_explained_energy: total_explained,
            features,
        },
        levels_work,
        residuals,
    ))
}

#[derive(Clone)]
struct GatedTag {
    id: i64,
    name: String,
    weight: f64,
    is_core: bool,
}

fn range_value(values: &[f64], index: usize, fallback: f64) -> f64 {
    values
        .get(index)
        .copied()
        .filter(|value| value.is_finite())
        .unwrap_or(fallback)
}

fn contains_cjk(value: &str) -> bool {
    value
        .chars()
        .any(|character| ('\u{4e00}'..='\u{9fff}').contains(&character))
}

fn technical_name(value: &str, allow_space: bool) -> bool {
    !contains_cjk(value)
        && value.chars().count() > 3
        && value.chars().all(|character| {
            character.is_ascii_alphanumeric()
                || matches!(character, '-' | '_' | '.')
                || (allow_space && character.is_ascii_whitespace())
        })
}

fn gate_tags(
    levels: &[Vec<PyramidTagWork>],
    epa: &EpaOutput,
    pyramid: &PyramidOutput,
    core_tags: &[String],
    config: &PipelineConfig,
) -> (Vec<GatedTag>, f64, f64) {
    let activation_min = range_value(&config.activation_multiplier, 0, 0.5);
    let activation_max = range_value(&config.activation_multiplier, 1, 1.5);
    let activation_multiplier =
        activation_min + pyramid.features.activation * (activation_max - activation_min);
    let dynamic_factor = epa.logic_depth * (1.0 + epa.resonance.ln_1p())
        / (1.0 + epa.entropy * 0.5)
        * activation_multiplier;
    let boost_min = range_value(&config.dynamic_boost_range, 0, 0.3);
    let boost_max = range_value(&config.dynamic_boost_range, 1, 2.0);
    let effective_boost =
        positive(config.base_tag_boost) * dynamic_factor.clamp(boost_min, boost_max);

    let core_metric = epa.logic_depth * 0.5 + (1.0 - pyramid.features.coverage) * 0.5;
    let core_min = range_value(&config.core_boost_range, 0, 1.2);
    let core_max = range_value(&config.core_boost_range, 1, 1.4);
    let dynamic_core_boost = core_min + core_metric * (core_max - core_min);
    let core_names: HashSet<String> = core_tags.iter().map(|name| name.to_lowercase()).collect();
    let world = epa
        .dominant_axes
        .first()
        .map(|axis| axis.label.as_str())
        .unwrap_or("Unknown");
    let world_lower = world.to_ascii_lowercase();
    let social_world = ["politics", "society", "history", "economics", "culture"]
        .iter()
        .any(|token| world_lower.contains(token));
    let technical_world = world != "Unknown" && technical_name(world, false);

    let mut seen = HashSet::new();
    let mut gated = Vec::new();
    for (level, tags) in levels.iter().enumerate() {
        for tag in tags {
            if tag.id <= 0 || !seen.insert(tag.id) {
                continue;
            }
            let is_core = core_names.contains(&tag.name.to_lowercase());
            // Cosine zero is a measured orthogonal relationship, not a
            // missing-score sentinel. Every pyramid candidate is measured
            // against its SQL original before entering this gate.
            let relevance = tag.similarity;
            let core_gain = if is_core {
                dynamic_core_boost * (0.95 + relevance * 0.1)
            } else {
                1.0
            };
            let mut language_gain = 1.0;
            if config.lang_confidence_enabled && technical_name(&tag.name, true) && !technical_world
            {
                let penalty = if world == "Unknown" {
                    config.lang_penalty_unknown
                } else {
                    config.lang_penalty_cross_domain
                }
                .clamp(0.0, 1.0);
                language_gain = if social_world {
                    penalty.sqrt()
                } else {
                    penalty
                };
            }
            let layer_gain = config.layer_decay.clamp(0.0, 1.0).powi(level as i32);
            let weight = positive(tag.contribution) * layer_gain * language_gain * core_gain;
            if weight > 0.0 {
                gated.push(GatedTag {
                    id: tag.id,
                    name: tag.name.clone(),
                    weight,
                    is_core,
                });
            }
        }
    }
    (gated, effective_boost.clamp(0.0, 1.0), dynamic_core_boost)
}

#[derive(Clone)]
struct FusionTag {
    id: i64,
    name: String,
    weight: f64,
    is_core: bool,
    vector: Arc<[f32]>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct FusionDiagnostics {
    requested_count: usize,
    found_count: usize,
    deduplicated_count: usize,
    total_weight: f64,
    selected_tag_ids: Vec<i64>,
    emergent_count: usize,
    supplemented_core_count: usize,
    ghost_count: usize,
}

fn fuse_observation_with_pool(
    connection: &MemoAssets,
    query: &[f32],
    observation: &SenseOutput,
    gated_tags: &[GatedTag],
    core_tags: &[String],
    ghost_tags: &[GhostTagInput],
    dynamic_core_boost: f64,
    dimension: usize,
    alpha: f64,
    config: &PipelineConfig,
    tag_vectors: &mut TagVectorPool,
) -> std::result::Result<(Vec<f64>, FusionDiagnostics, Vec<FusionTag>), String> {
    // 与 JS SOTA 完全相同：传播后的原始种子取 max，防止循环共现膨胀；
    // 纯涌现节点按能量排序后只保留 Top-N。
    let gated_by_id: HashMap<i64, &GatedTag> = gated_tags.iter().map(|tag| (tag.id, tag)).collect();
    let mut merged = Vec::new();
    let mut emergent = Vec::new();
    for node in &observation.nodes {
        if node.id <= 0 || node.energy <= 0.0 {
            continue;
        }
        if let Some(seed) = gated_by_id.get(&node.id) {
            merged.push(GatedTag {
                id: seed.id,
                name: seed.name.clone(),
                weight: seed.weight.max(node.energy),
                is_core: seed.is_core,
            });
        } else {
            emergent.push(GatedTag {
                id: node.id,
                name: String::new(),
                weight: node.energy,
                is_core: false,
            });
        }
    }
    emergent.sort_by(|left, right| {
        right
            .weight
            .partial_cmp(&left.weight)
            .unwrap_or(Ordering::Equal)
            .then_with(|| left.id.cmp(&right.id))
    });
    if config.max_emergent_nodes > 0 {
        emergent.truncate(config.max_emergent_nodes);
    }
    let emergent_count = emergent.len();
    merged.extend(emergent);

    // JS 使用传播后 allTags 的最大基准权重；coreBoostFactor 仅用于还原基准，
    // 实际补全 Core 与 Hard Ghost 使用动态 Core 增益。
    let core_divisor =
        if config.core_boost_factor.is_finite() && config.core_boost_factor.abs() > 1e-12 {
            config.core_boost_factor
        } else {
            1.33
        };
    let max_base_weight = merged
        .iter()
        .map(|tag| tag.weight / core_divisor)
        .fold(0.0, f64::max)
        .max(if merged.is_empty() { 1.0 } else { 0.0 });

    // 字符串 Core 不参与本轮 Spike 扩散；传播完成后按名称从数据库补入，
    // 保持 JS 的“最终融合锚点”语义。
    let mut present_names: HashSet<String> = merged
        .iter()
        .filter(|tag| !tag.name.is_empty())
        .map(|tag| tag.name.to_lowercase())
        .collect();
    let mut supplemented_core_count = 0usize;
    for core_name in core_tags {
        let normalized = core_name.to_lowercase();
        if normalized.is_empty() || present_names.contains(&normalized) {
            continue;
        }
        if let Some((id, name)) = connection
            .tag_names
            .iter()
            .find(|(_, name)| name.eq_ignore_ascii_case(core_name))
            .map(|(id, name)| (*id, name.clone()))
        {
            if merged.iter().any(|tag| tag.id == id) {
                continue;
            }
            present_names.insert(name.to_lowercase());
            merged.push(GatedTag {
                id,
                name,
                weight: max_base_weight * dynamic_core_boost,
                is_core: true,
            });
            supplemented_core_count += 1;
        }
    }

    let requested_count = merged.len() + ghost_tags.len();
    let ids = merged.iter().map(|tag| tag.id).collect::<Vec<_>>();
    ensure_tag_vectors_loaded(&connection.tag_vectors, tag_vectors, &ids, dimension)?;
    let mut vectors = Vec::with_capacity(requested_count);
    for tag in merged {
        let Some(vector) = tag_vectors.get(&tag.id).cloned() else {
            continue;
        };
        if magnitude(&vector) > 1e-12 {
            vectors.push(FusionTag {
                id: tag.id,
                name: if tag.name.is_empty() {
                    format!("Tag#{}", tag.id)
                } else {
                    tag.name
                },
                weight: tag.weight,
                is_core: tag.is_core,
                vector,
            });
        }
    }

    // 幽灵节点使用专属负 ID，仅参与去重与向量融合，绝不进入 Artifact/Spike。
    let mut ghost_id = -1i64;
    let mut accepted_ghosts = 0usize;
    for ghost in ghost_tags {
        if ghost.vector.len() != dimension || magnitude(&ghost.vector) <= 1e-12 {
            continue;
        }
        vectors.push(FusionTag {
            id: ghost_id,
            name: ghost.name.clone(),
            weight: max_base_weight
                * if ghost.is_core {
                    dynamic_core_boost
                } else {
                    1.0
                },
            is_core: ghost.is_core,
            vector: Arc::from(ghost.vector.clone()),
        });
        ghost_id -= 1;
        accepted_ghosts += 1;
    }
    let found_count = vectors.len();

    // JS SOTA：按权重降序贪心去重；冗余标签把 20% 能量转移给代表标签，
    // 同时保留 Core 属性。不能简化为“直接丢弃冗余标签”。
    vectors.sort_by(|left, right| {
        right
            .weight
            .partial_cmp(&left.weight)
            .unwrap_or(Ordering::Equal)
            .then_with(|| left.id.cmp(&right.id))
    });
    vectors.truncate(config.max_fusion_tags.clamp(1, 1024));
    let threshold = config.deduplication_threshold.clamp(-1.0, 1.0);
    let mut selected: Vec<FusionTag> = Vec::new();
    for candidate in vectors {
        let redundant_index = selected
            .iter()
            .position(|existing| cosine(&candidate.vector, &existing.vector) > threshold);
        if let Some(index) = redundant_index {
            selected[index].weight += candidate.weight * 0.2;
            selected[index].is_core |= candidate.is_core;
        } else {
            selected.push(candidate);
        }
    }

    let total_weight = selected.iter().map(|entry| entry.weight).sum::<f64>();
    if total_weight <= 0.0 {
        // JS applyTagBoost() 的兼容契约：无法构造 Tag 上下文时原样返回查询向量，
        // 而不是抛错。空 selected 也确保 matched/coreTagsMatched 保持为空。
        let diagnostics = FusionDiagnostics {
            requested_count,
            found_count,
            deduplicated_count: 0,
            total_weight: 0.0,
            selected_tag_ids: Vec::new(),
            emergent_count,
            supplemented_core_count,
            ghost_count: accepted_ghosts,
        };
        return Ok((
            query.iter().map(|value| *value as f64).collect(),
            diagnostics,
            Vec::new(),
        ));
    }
    let mut context = vec![0.0f64; dimension];
    for tag in &selected {
        for offset in 0..dimension {
            context[offset] += tag.vector[offset] as f64 * tag.weight;
        }
    }
    for value in &mut context {
        *value /= total_weight;
    }
    normalize(&mut context);

    let mut fused: Vec<f64> = query
        .iter()
        .zip(context.iter())
        .map(|(source, memo)| (1.0 - alpha) * (*source as f64) + alpha * *memo)
        .collect();
    normalize(&mut fused);
    let diagnostics = FusionDiagnostics {
        requested_count,
        found_count,
        deduplicated_count: selected.len(),
        total_weight,
        selected_tag_ids: selected.iter().map(|entry| entry.id).collect(),
        emergent_count,
        supplemented_core_count,
        ghost_count: accepted_ghosts,
    };
    Ok((fused, diagnostics, selected))
}

#[derive(Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct DualFieldDiagnostics {
    retained_local_field_nodes: usize,
    retained_transfer_field_nodes: usize,
    local_domain_nodes: usize,
    transfer_domain_nodes: usize,
    projected_nodes: usize,
    selected_projection_nodes: usize,
    source_vectors_loaded: usize,
    source_requested_nodes: usize,
    source_batches: usize,
    growth: crate::projection::GrowthDiagnostics,
    local_support: ProjectionSupportDiagnostics,
    transfer_support: ProjectionSupportDiagnostics,
    local_selected_mass_ratio: f64,
    transfer_selected_mass_ratio: f64,
    local_projected_mass_ratio: f64,
    transfer_projected_mass_ratio: f64,
    solve_ms: f64,
    domain_ms: f64,
    project_ms: f64,
    iterations: usize,
    local_converged: bool,
    transfer_converged: bool,
    local_residual: f64,
    transfer_residual: f64,
    local_mass: f64,
    transfer_mass: f64,
}

struct DualFieldOutput {
    local_field: Vec<(i64, f64)>,
    transfer_field: Vec<(i64, f64)>,
    local_domain_ids: Vec<i64>,
    transfer_domain_ids: Vec<i64>,
    local_vector: Vec<f32>,
    transfer_vector: Vec<f32>,
    diagnostics: DualFieldDiagnostics,
}

fn apply_transport(artifact: &crate::graph::NativeArtifact, input: &[f64], output: &mut [f64]) {
    output.fill(0.0);
    for source in 0..artifact.node_ids.len() {
        let mass = input[source];
        if mass == 0.0 {
            continue;
        }
        for cursor in artifact.row_offsets[source]..artifact.row_offsets[source + 1] {
            output[artifact.targets[cursor]] += mass * positive(artifact.weights[cursor]);
        }
    }
}

/// `apply_transport` for both fields in one pass over the CSR. Every output
/// position receives the same additions in the same order as two separate
/// passes, and each field keeps its own zero-mass skip.
fn apply_transport_pair(
    artifact: &crate::graph::NativeArtifact,
    local: &[f64],
    transfer: &[f64],
    local_output: &mut [f64],
    transfer_output: &mut [f64],
) {
    local_output.fill(0.0);
    transfer_output.fill(0.0);
    for source in 0..artifact.node_ids.len() {
        let local_mass = local[source];
        let transfer_mass = transfer[source];
        let local_active = local_mass != 0.0;
        let transfer_active = transfer_mass != 0.0;
        if !local_active && !transfer_active {
            continue;
        }
        for cursor in artifact.row_offsets[source]..artifact.row_offsets[source + 1] {
            let weight = positive(artifact.weights[cursor]);
            let target = artifact.targets[cursor];
            if local_active {
                local_output[target] += local_mass * weight;
            }
            if transfer_active {
                transfer_output[target] += transfer_mass * weight;
            }
        }
    }
}

/// `next = (1 - alpha) * source + alpha * propagated` and the L1 change from
/// `current`, in one pass with the same arithmetic and index order as a
/// separate update followed by an L1 sum of non-negative terms.
fn relax_field(
    source: &[f64],
    propagated: &[f64],
    alpha: f64,
    current: &[f64],
    next: &mut [f64],
) -> f64 {
    let mut residual = 0.0;
    for index in 0..source.len() {
        next[index] = (1.0 - alpha) * source[index] + alpha * propagated[index];
        residual += (next[index] - current[index]).abs();
    }
    residual
}

fn field_mass(field: &[f64]) -> f64 {
    field.iter().map(|value| positive(*value)).sum()
}

fn effective_domain(
    artifact: &crate::graph::NativeArtifact,
    field: &[f64],
    ratio: f64,
) -> Vec<i64> {
    let total = field_mass(field);
    if total <= 0.0 {
        return Vec::new();
    }
    let mut ranked: Vec<(i64, f64)> = artifact
        .node_ids
        .iter()
        .copied()
        .zip(field.iter().copied())
        .filter(|(_, mass)| *mass > 0.0)
        .collect();
    ranked.sort_by(|left, right| {
        right
            .1
            .partial_cmp(&left.1)
            .unwrap_or(Ordering::Equal)
            .then_with(|| left.0.cmp(&right.0))
    });
    let mut retained = Vec::new();
    let mut mass = 0.0;
    for (id, value) in ranked {
        retained.push(id);
        mass += value;
        if mass / total >= ratio.clamp(0.01, 1.0) {
            break;
        }
    }
    retained
}

/// Legacy full-support projection retained only as a regression reference.

#[derive(Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProjectionSupportDiagnostics {
    core_nodes: usize,
    selected_nodes: usize,
    outside_domain_nodes: usize,
    missing_vector_nodes: usize,
    stop_reason: String,
}

fn project_basin_fields(
    connection: &MemoAssets,
    artifact: &crate::graph::NativeArtifact,
    fields: [&[f64]; 2],
    domains: [&[i64]; 2],
    dimension: usize,
    limit: usize,
    river_ids: &[i64],
    basins: &[crate::projection::Basin],
    pool: &mut TagVectorPool,
    diagnostics: &mut DualFieldDiagnostics,
) -> std::result::Result<(Vec<f32>, Vec<f32>), String> {
    let supports = crate::projection::grow(artifact, fields, domains, river_ids, basins, limit)?;
    let selected = supports.ids;
    let support_diagnostics = |f: usize| {
        let domain: HashSet<i64> = domains[f].iter().copied().collect();
        ProjectionSupportDiagnostics {
            core_nodes: supports.diagnostics.core_nodes,
            selected_nodes: selected[f].len(),
            outside_domain_nodes: selected[f].iter().filter(|id| !domain.contains(id)).count(),
            stop_reason: supports.diagnostics.stop_reason.clone(),
            ..ProjectionSupportDiagnostics::default()
        }
    };
    diagnostics.local_support = support_diagnostics(0);
    diagnostics.transfer_support = support_diagnostics(1);
    diagnostics.growth = supports.diagnostics;
    let mut union: Vec<i64> = selected.iter().flatten().copied().collect();
    union.sort_unstable();
    union.dedup();
    diagnostics.selected_projection_nodes = union.len();
    let stats = ensure_tag_vectors_loaded(&connection.tag_vectors, pool, &union, dimension)?;
    diagnostics.source_vectors_loaded = stats.fetched_nodes;
    diagnostics.source_requested_nodes = stats.requested_nodes;
    diagnostics.source_batches = stats.source_batches;
    diagnostics.projected_nodes = union.iter().filter(|id| pool.contains_key(id)).count();
    diagnostics.local_support.missing_vector_nodes = selected[0]
        .iter()
        .filter(|id| !pool.contains_key(id))
        .count();
    diagnostics.transfer_support.missing_vector_nodes = selected[1]
        .iter()
        .filter(|id| !pool.contains_key(id))
        .count();
    let mut outputs = [vec![0.0f64; dimension], vec![0.0f64; dimension]];
    let mut selected_ratios = [0.0; 2];
    let mut projected_ratios = [0.0; 2];
    for field in 0..2 {
        let total_mass = field_mass(fields[field]);
        let mut selected_mass = 0.0;
        let mut projected_mass = 0.0;
        for id in &selected[field] {
            let Some(&index) = artifact.node_index.get(id) else {
                continue;
            };
            let mass = positive(fields[field][index]);
            selected_mass += mass;
            let Some(vector) = pool.get(id) else { continue };
            projected_mass += mass;
            for (out, value) in outputs[field].iter_mut().zip(vector.iter()) {
                *out += *value as f64 * mass;
            }
        }
        if projected_mass > 0.0 {
            for out in &mut outputs[field] {
                *out /= projected_mass;
            }
            normalize(&mut outputs[field]);
        }
        if total_mass > 0.0 {
            selected_ratios[field] = selected_mass / total_mass;
            projected_ratios[field] = projected_mass / total_mass;
        }
    }
    [
        diagnostics.local_selected_mass_ratio,
        diagnostics.transfer_selected_mass_ratio,
    ] = selected_ratios;
    [
        diagnostics.local_projected_mass_ratio,
        diagnostics.transfer_projected_mass_ratio,
    ] = projected_ratios;
    let [local, transfer] = outputs;
    Ok((
        local.into_iter().map(|v| v as f32).collect(),
        transfer.into_iter().map(|v| v as f32).collect(),
    ))
}

fn solve_dual_fields_with_pool(
    artifact: &crate::graph::NativeArtifact,
    connection: &MemoAssets,
    source_entries: &[(i64, f64)],
    dimension: usize,
    config: &PipelineConfig,
    pool: &mut TagVectorPool,
    river_ids: &[i64],
    basins: &[crate::projection::Basin],
) -> std::result::Result<DualFieldOutput, String> {
    let solve_started = Instant::now();
    let mut source = vec![0.0f64; artifact.node_ids.len()];
    for (id, mass) in source_entries {
        if let Some(index) = artifact.node_index.get(id) {
            source[*index] += positive(*mass);
        }
    }
    let source_mass = field_mass(&source);
    if source_mass <= 0.0 {
        return Ok(DualFieldOutput {
            local_field: Vec::new(),
            transfer_field: Vec::new(),
            local_domain_ids: Vec::new(),
            transfer_domain_ids: Vec::new(),
            local_vector: vec![0.0; dimension],
            transfer_vector: vec![0.0; dimension],
            diagnostics: DualFieldDiagnostics {
                iterations: 0,
                local_converged: true,
                transfer_converged: true,
                local_residual: 0.0,
                transfer_residual: 0.0,
                local_mass: 0.0,
                transfer_mass: 0.0,
                ..DualFieldDiagnostics::default()
            },
        });
    }
    for value in &mut source {
        *value /= source_mass;
    }

    let local_alpha = config.local_alpha.clamp(0.0, 0.999999);
    let transfer_alpha = config.transfer_alpha.clamp(0.0, 0.999999);
    let mut local = source.clone();
    let mut transfer = source.clone();
    let mut next_local = vec![0.0; source.len()];
    let mut next_transfer = vec![0.0; source.len()];
    let mut propagated_local = vec![0.0; source.len()];
    let mut propagated_transfer = vec![0.0; source.len()];
    let mut local_converged = false;
    let mut transfer_converged = false;
    let mut local_residual = f64::INFINITY;
    let mut transfer_residual = f64::INFINITY;
    let mut iterations = 0;

    for iteration in 1..=config.field_max_iterations.max(1) {
        iterations = iteration;
        // The fields are independent; while both move, one CSR pass serves both.
        if !local_converged && !transfer_converged {
            apply_transport_pair(
                artifact,
                &local,
                &transfer,
                &mut propagated_local,
                &mut propagated_transfer,
            );
        } else if !local_converged {
            apply_transport(artifact, &local, &mut propagated_local);
        } else if !transfer_converged {
            apply_transport(artifact, &transfer, &mut propagated_transfer);
        }
        if !local_converged {
            local_residual = relax_field(
                &source,
                &propagated_local,
                local_alpha,
                &local,
                &mut next_local,
            );
            local_converged = local_residual <= config.local_tolerance.max(1e-15);
            std::mem::swap(&mut local, &mut next_local);
        }
        if !transfer_converged {
            transfer_residual = relax_field(
                &source,
                &propagated_transfer,
                transfer_alpha,
                &transfer,
                &mut next_transfer,
            );
            transfer_converged = transfer_residual <= config.transfer_tolerance.max(1e-15);
            std::mem::swap(&mut transfer, &mut next_transfer);
        }
        if local_converged && transfer_converged {
            break;
        }
    }

    let solve_ms = solve_started.elapsed().as_secs_f64() * 1000.0;
    let domain_started = Instant::now();
    let local_domain_ids = effective_domain(artifact, &local, config.local_mass_ratio);
    let transfer_domain_ids = effective_domain(artifact, &transfer, config.transfer_mass_ratio);
    // Scalar retention is independent of projection K. Keep meaningful tails for
    // path potentials/tail penalties, and always preserve support/river nodes.
    let mut retained: Vec<i64> = local_domain_ids
        .iter()
        .chain(&transfer_domain_ids)
        .chain(river_ids)
        .copied()
        .collect();
    retained.sort_unstable();
    retained.dedup();
    let required: HashSet<i64> = retained.into_iter().collect();
    let scalar = |field: &[f64]| -> Vec<(i64, f64)> {
        let maximum = field.iter().copied().fold(0.0f64, f64::max);
        let cutoff = maximum * 1e-12;
        artifact
            .node_ids
            .iter()
            .copied()
            .zip(field.iter().copied())
            .filter(|(id, mass)| {
                mass.is_finite() && *mass > 0.0 && (*mass >= cutoff || required.contains(id))
            })
            .collect()
    };
    let local_field = scalar(&local);
    let transfer_field = scalar(&transfer);
    let local_mass = field_mass(&local);
    let transfer_mass = field_mass(&transfer);
    let mut projection = DualFieldDiagnostics {
        solve_ms,
        retained_local_field_nodes: local_field.len(),
        retained_transfer_field_nodes: transfer_field.len(),
        local_domain_nodes: local_domain_ids.len(),
        transfer_domain_nodes: transfer_domain_ids.len(),
        domain_ms: domain_started.elapsed().as_secs_f64() * 1000.0,
        ..DualFieldDiagnostics::default()
    };
    let project_started = Instant::now();
    let (local_vector, transfer_vector) = project_basin_fields(
        connection,
        artifact,
        [&local, &transfer],
        [&local_domain_ids, &transfer_domain_ids],
        dimension,
        config.max_field_projection_nodes,
        river_ids,
        basins,
        pool,
        &mut projection,
    )?;
    projection.project_ms = project_started.elapsed().as_secs_f64() * 1000.0;

    Ok(DualFieldOutput {
        local_field,
        transfer_field,
        local_domain_ids,
        transfer_domain_ids,
        local_vector,
        transfer_vector,
        diagnostics: DualFieldDiagnostics {
            iterations,
            local_converged,
            transfer_converged,
            local_residual,
            transfer_residual,
            local_mass,
            transfer_mass,
            ..projection
        },
    })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PipelineDiagnostics {
    backend: String,
    runtime_ownership: String,
    epa_ms: f64,
    pyramid_ms: f64,
    gating_ms: f64,
    sensing_ms: f64,
    fusion_ms: f64,
    dual_field_solve_ms: f64,
    dual_field_project_ms: f64,
    dual_field_domain_ms: f64,
    total_ms: f64,
    seed_nodes: usize,
    observation_cached: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    observation_cache_error: Option<String>,
    dual_field: DualFieldDiagnostics,
    fusion: FusionDiagnostics,
    sensing: SenseDiagnostics,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PipelineMetadataOutput {
    schema: String,
    algorithm_version: String,
    artifact_sig: String,
    query_id: Option<String>,
    epa: EpaOutput,
    pyramid: PyramidOutput,
    matched_tags: Vec<String>,
    core_tags_matched: Vec<String>,
    effective_tag_boost: f64,
    diagnostics: PipelineDiagnostics,
}

pub struct PipelineOutput {
    pub generation: String,
    pub artifact_sig: String,
    pub original_query_vector: Vec<f32>,
    pub enhanced_vector: Vec<f32>,
    pub local_vector: Vec<f32>,
    pub transfer_vector: Vec<f32>,
    pub local_field: Vec<(i64, f64)>,
    pub transfer_field: Vec<(i64, f64)>,
    pub local_domain_ids: Vec<i64>,
    pub transfer_domain_ids: Vec<i64>,
    pub observation: SenseOutput,
    pub tag_vectors: TagVectorPool,
    pub metadata: serde_json::Value,
}
impl PipelineOutput {
    /// Validate the generation and request-local numerical state consumed by readout.
    pub fn validate_for(&self, assets: &MemoAssets) -> Result<(), String> {
        if self.generation != assets.generation
            || self.artifact_sig != assets.signature
            || self.observation.artifact_sig != assets.signature
        {
            return Err("MEMO_OBSERVATION_STALE: asset generation or signature changed".into());
        }
        let vectors = [
            &self.original_query_vector,
            &self.enhanced_vector,
            &self.local_vector,
            &self.transfer_vector,
        ];
        if vectors.iter().any(|vector| {
            vector.len() != assets.dimension || vector.iter().any(|value| !value.is_finite())
        }) || self.tag_vectors.iter().any(|(id, vector)| {
            *id <= 0
                || vector.len() != assets.dimension
                || vector.iter().any(|value| !value.is_finite())
        }) {
            return Err(
                "MEMO_QUERY_VECTOR_INVALID: dimension or finite-value validation failed".into(),
            );
        }
        let valid_id = |id: i64| assets.graph.node_index.contains_key(&id);
        if self
            .local_field
            .iter()
            .chain(&self.transfer_field)
            .chain(&self.observation.source_field)
            .any(|(id, value)| !valid_id(*id) || !value.is_finite())
            || self
                .local_domain_ids
                .iter()
                .chain(&self.transfer_domain_ids)
                .any(|id| !valid_id(*id))
            || self.observation.nodes.iter().any(|node| {
                !valid_id(node.id)
                    || !node.energy.is_finite()
                    || !node.normalized_energy.is_finite()
            })
            || self.observation.edges.iter().any(|edge| {
                !valid_id(edge.source_id)
                    || !valid_id(edge.target_id)
                    || [
                        edge.flow,
                        edge.max_flow,
                        edge.normalized_flow,
                        edge.conductance,
                    ]
                    .iter()
                    .any(|value| !value.is_finite())
            })
            || self.observation.transitions.iter().any(|edge| {
                !valid_id(edge.source_id)
                    || !valid_id(edge.target_id)
                    || edge.previous_id.is_some_and(|id| !valid_id(id))
                    || !edge.flow.is_finite()
            })
        {
            return Err(
                "MEMO_QUERY_STATE_INVALID: observation IDs or finite values are invalid".into(),
            );
        }
        Ok(())
    }
}
pub fn run_pipeline(
    connection: &MemoAssets,
    index: &dyn TagSearch,
    input: PipelineInput,
) -> Result<PipelineOutput, String> {
    crate::assets::validate_finite_config(&input.config)?;
    let total_started = Instant::now();
    let dimensions = connection.dimension;
    let artifact = connection.graph.clone();
    let artifact_sig = &connection.signature;
    if input.query_vector.len() != dimensions
        || input.query_vector.iter().any(|v| !v.is_finite())
        || input
            .ghost_tags
            .iter()
            .any(|g| g.vector.len() != dimensions || g.vector.iter().any(|v| !v.is_finite()))
    {
        return Err("Memo pipeline vector dimensions or values are invalid".into());
    }
    let epa_started = Instant::now();
    if ![
        "absolute",
        "relativeLinear",
        "relativeSqrt",
        "backgroundRelative",
    ]
    .contains(&input.config.epa_gate_mode.as_str())
    {
        return Err("Invalid epaGateMode".into());
    }
    let epa = analyze_epa_with_mode(
        connection,
        &input.query_vector,
        dimensions,
        &input.config.epa_gate_mode,
    );
    let epa_ms = epa_started.elapsed().as_secs_f64() * 1000.0;

    let mut tag_vectors = TagVectorPool::new();
    let pyramid_started = Instant::now();
    let (pyramid, level_tags, residuals) = analyze_pyramid_with_pool(
        index,
        connection,
        &input.query_vector,
        dimensions,
        &input.config,
        &mut tag_vectors,
    )?;
    let pyramid_ms = pyramid_started.elapsed().as_secs_f64() * 1000.0;

    let gating_started = Instant::now();
    let (gated_tags, effective_tag_boost, dynamic_core_boost) =
        gate_tags(&level_tags, &epa, &pyramid, &input.core_tags, &input.config);
    // 空 Pyramid 门控不再中止：Spike 生成合法空观测，随后字符串 Core/幽灵
    // 仍按 JS SOTA 在传播后注入；若最终仍为空，融合层返回原查询向量。
    let seeds: Vec<SenseSeed> = gated_tags
        .iter()
        .map(|tag| SenseSeed {
            id: tag.id,
            energy: tag.weight,
            source_type: if tag.is_core {
                "core".to_string()
            } else {
                "seed".to_string()
            },
        })
        .collect();
    let gating_ms = gating_started.elapsed().as_secs_f64() * 1000.0;

    let sensing_started = Instant::now();
    let mut seed_levels = HashMap::new();
    for (level, tags) in level_tags.iter().enumerate() {
        for tag in tags {
            seed_levels.entry(tag.id).or_insert(level);
        }
    }
    let mut wormholes = DynamicWormholeContext::try_new(
        &connection.tag_vectors,
        &mut tag_vectors,
        residuals,
        seed_levels,
    )?;
    for (layer, tags) in level_tags.iter().enumerate() {
        for tag in tags {
            wormholes.reuse_score(layer, tag.id, tag.similarity);
        }
    }
    let observation = sense_prevalidated(
        &artifact,
        artifact_sig,
        SenseInput {
            query_id: input.query_id.clone(),
            seeds,
            config: input.config.spike_routing.clone(),
        },
        Some(&mut wormholes),
    )?;
    let sensing_diagnostics = observation.diagnostics.clone();
    drop(wormholes);
    let sensing_ms = sensing_started.elapsed().as_secs_f64() * 1000.0;

    let fusion_started = Instant::now();
    let (enhanced_vector, fusion, selected_tags) = fuse_observation_with_pool(
        connection,
        &input.query_vector,
        &observation,
        &gated_tags,
        &input.core_tags,
        &input.ghost_tags,
        dynamic_core_boost,
        dimensions,
        effective_tag_boost,
        &input.config,
        &mut tag_vectors,
    )?;
    let fusion_ms = fusion_started.elapsed().as_secs_f64() * 1000.0;
    let dual_fields = solve_dual_fields_with_pool(
        &artifact,
        connection,
        &observation.source_field,
        dimensions,
        &input.config,
        &mut tag_vectors,
        &observation
            .nodes
            .iter()
            .map(|node| node.id)
            .collect::<Vec<_>>(),
        &crate::projection::seed_basins(&observation, &artifact),
    )?;

    let enhanced_vector_f32: Vec<f32> = enhanced_vector.iter().map(|value| *value as f32).collect();
    let observation_cached = false;
    let maximum_weight = selected_tags
        .iter()
        .map(|tag| tag.weight)
        .fold(0.0, f64::max);
    let matched_tags = selected_tags
        .iter()
        .filter(|tag| {
            tag.is_core
                || tag.weight
                    > maximum_weight
                        * if technical_name(&tag.name, true) {
                            input.config.tech_tag_threshold
                        } else {
                            input.config.normal_tag_threshold
                        }
        })
        .map(|tag| tag.name.clone())
        .collect::<Vec<_>>();
    let core_tags_matched = selected_tags
        .iter()
        .filter(|tag| tag.is_core)
        .map(|tag| tag.name.clone())
        .collect::<Vec<_>>();
    let seed_nodes = gated_tags.len();

    let dual_field_diagnostics = dual_fields.diagnostics;
    let metadata_json = serde_json::to_string(&PipelineMetadataOutput {
        schema: PIPELINE_SCHEMA.to_string(),
        algorithm_version: PIPELINE_ALGORITHM.to_string(),
        artifact_sig: artifact_sig.to_string(),
        query_id: input.query_id.clone(),
        epa,
        pyramid,
        matched_tags,
        core_tags_matched,
        effective_tag_boost,
        diagnostics: PipelineDiagnostics {
            backend: "rust-unified-memo-pipeline".to_string(),
            runtime_ownership: "typed-query-output".to_string(),
            epa_ms,
            pyramid_ms,
            gating_ms,
            sensing_ms,
            fusion_ms,
            dual_field_solve_ms: dual_field_diagnostics.solve_ms,
            dual_field_project_ms: dual_field_diagnostics.project_ms,
            dual_field_domain_ms: dual_field_diagnostics.domain_ms,
            total_ms: total_started.elapsed().as_secs_f64() * 1000.0,
            seed_nodes,
            observation_cached,
            observation_cache_error: None,
            dual_field: dual_field_diagnostics,
            fusion,
            sensing: sensing_diagnostics,
        },
    })
    .map_err(|error| format!("encode unified Memo pipeline metadata failed: {}", error))?;

    Ok(PipelineOutput {
        generation: connection.generation.clone(),
        artifact_sig: connection.signature.clone(),
        original_query_vector: input.query_vector,
        enhanced_vector: enhanced_vector_f32,
        local_vector: dual_fields.local_vector,
        transfer_vector: dual_fields.transfer_vector,
        local_field: dual_fields.local_field,
        transfer_field: dual_fields.transfer_field,
        local_domain_ids: dual_fields.local_domain_ids,
        transfer_domain_ids: dual_fields.transfer_domain_ids,
        observation,
        tag_vectors,
        metadata: serde_json::from_str(&metadata_json).map_err(|error| error.to_string())?,
    })
}
