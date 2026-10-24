//! EPA basis decoding, calibrated gates and semantic projection.
use super::*;

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct EpaCache {
    pub(super) basis: Vec<String>,
    pub(super) mean: String,
    #[serde(default, rename = "energies")]
    pub(super) _energies: Vec<f64>,
    #[serde(default)]
    pub(super) labels: Vec<String>,
    #[serde(default = "default_k_sem")]
    pub(super) k_sem: usize,
    #[serde(default)]
    pub(super) coverage: Vec<f64>,
}

pub(super) fn default_k_sem() -> usize {
    8
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct EpaAxis {
    pub(super) index: usize,
    pub(super) label: String,
    pub(super) energy: f64,
    pub(super) projection: f64,
    pub(super) score: f64,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct EpaBridge {
    pub(super) from: String,
    pub(super) to: String,
    pub(super) strength: f64,
    pub(super) balance: f64,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct EpaOutput {
    pub(super) logic_depth: f64,
    pub(super) entropy: f64,
    pub(super) resonance: f64,
    pub(super) dominant_axes: Vec<EpaAxis>,
    pub(super) bridges: Vec<EpaBridge>,
    pub(super) basis_count: usize,
    pub(super) cache_available: bool,
    pub(super) out_of_domain: f64,
    pub(super) semantic_explained_ratio: f64,
    pub(super) in_domain_gate: f64,
    pub(super) baseline_coverage: f64,
    pub(super) background_ratio: f64,
}

pub(super) fn decode_f32_base64(encoded: &str, dimension: usize) -> Option<Vec<f32>> {
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
pub(super) fn calibrated_gate(ratio: f64, baseline: f64, background: f64, mode: &str) -> f64 {
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

pub(super) fn analyze_epa_with_mode(
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
