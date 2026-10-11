// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use crate::*;
use nalgebra::DMatrix;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpaBasis {
    pub mean: Vec<f64>,
    pub basis: Vec<Vec<f64>>,
    pub singular_energies: Vec<f64>,
    pub embedding_space: String,
}

/// A published, immutable EPA serving projection for one embedding space.
/// The basis is derived from active Tag vectors and is never Authority.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpaBasisGeneration {
    pub generation_id: ServingGenerationId,
    pub basis: EpaBasis,
}

type RepresentativeSet = (Vec<(u64, Vec<f64>)>, Vec<f64>);

pub fn build_epa_basis(
    vectors: &[(u64, Vec<f64>)],
    embedding_space: &str,
    policy: &EpaPolicy,
) -> Option<EpaBasis> {
    if vectors.len() < policy.minimum_samples || vectors.iter().any(|(_, vector)| vector.is_empty())
    {
        return None;
    }
    let dimension = vectors[0].1.len();
    if vectors.iter().any(|(_, vector)| vector.len() != dimension) {
        return None;
    }
    let vectors = vectors
        .iter()
        .filter_map(|(key, vector)| {
            let norm = vector.iter().map(|value| value * value).sum::<f64>().sqrt();
            (norm > f64::EPSILON).then(|| {
                (
                    *key,
                    vector.iter().map(|value| *value / norm).collect::<Vec<_>>(),
                )
            })
        })
        .collect::<Vec<_>>();
    if vectors.len() < policy.minimum_samples {
        return None;
    }
    let (vectors, weights) = if vectors.len() > policy.max_representatives {
        representative_vectors(&vectors, embedding_space, dimension, policy)
    } else {
        let weights = vec![1.0; vectors.len()];
        (vectors, weights)
    };
    build_weighted_epa_basis(&vectors, &weights, embedding_space, dimension, policy)
}

fn build_weighted_epa_basis(
    vectors: &[(u64, Vec<f64>)],
    weights: &[f64],
    embedding_space: &str,
    dimension: usize,
    policy: &EpaPolicy,
) -> Option<EpaBasis> {
    if vectors.is_empty()
        || vectors.len() != weights.len()
        || vectors.iter().any(|(_, vector)| vector.len() != dimension)
    {
        return None;
    }
    let mut mean = vec![0.0; dimension];
    let total_weight = weights.iter().sum::<f64>().max(f64::EPSILON);
    for ((_, vector), weight) in vectors.iter().zip(weights) {
        for (target, value) in mean.iter_mut().zip(vector.iter()) {
            *target += *weight * *value;
        }
    }
    for value in &mut mean {
        *value /= total_weight;
    }
    let mean_ref = &mean;
    let data = vectors
        .iter()
        .zip(weights)
        .flat_map(|((_, vector), weight)| {
            let scale = weight.sqrt();
            vector
                .iter()
                .enumerate()
                .map(move |(index, value)| scale * (value - mean_ref[index]))
        })
        .collect::<Vec<_>>();
    let matrix = DMatrix::from_row_slice(vectors.len(), dimension, &data);
    let svd = matrix.svd(false, true);
    let singular = svd.singular_values.iter().copied().collect::<Vec<_>>();
    let total_energy = singular
        .iter()
        .map(|value| value * value)
        .sum::<f64>()
        .max(f64::EPSILON);
    let energies = singular
        .iter()
        .map(|value| value * value / total_energy)
        .collect::<Vec<_>>();
    let mut basis = Vec::new();
    if let Some(v_t) = svd.v_t {
        for (row, energy) in v_t.row_iter().zip(&energies) {
            if *energy < policy.minimum_axis_energy || basis.len() >= policy.max_axes {
                break;
            }
            basis.push(row.iter().copied().collect());
        }
    }
    (!basis.is_empty()).then_some(EpaBasis {
        mean,
        basis,
        singular_energies: energies,
        embedding_space: embedding_space.to_owned(),
    })
}

pub(crate) fn representative_vectors(
    vectors: &[(u64, Vec<f64>)],
    _embedding_space: &str,
    _dimension: usize,
    policy: &EpaPolicy,
) -> RepresentativeSet {
    let mut sorted = vectors.to_vec();
    sorted.sort_by(|left, right| {
        stable_vector_key(&left.1)
            .cmp(&stable_vector_key(&right.1))
            .then_with(|| left.0.cmp(&right.0))
    });

    let mut representative = Vec::<RepresentativeEntry>::new();
    for (key, vector) in sorted {
        if let Some((_, count)) = representative.iter_mut().find(|(existing, _)| {
            cosine(&existing.1, &vector) >= 1.0 - policy.duplicate_cosine_epsilon
        }) {
            *count += 1;
        } else {
            representative.push(((key, vector), 1));
        }
    }

    if representative.len() <= policy.max_representatives {
        return (
            representative
                .iter()
                .map(|((key, vector), _)| (*key, vector.clone()))
                .collect(),
            representative
                .iter()
                .map(|(_, count)| *count as f64)
                .collect(),
        );
    }

    let mut min_distance = vec![2.0; representative.len()];
    let mut chosen = Vec::new();
    let mut first = 0;
    for (index, ((_key, _), _)) in representative.iter().enumerate() {
        let key_value = stable_vector_key(&representative[index].0.1);
        if stable_vector_key(&representative[first].0.1) > key_value {
            first = index;
        }
    }
    chosen.push(first);
    update_min_distances(&representative, &chosen, &mut min_distance);

    while chosen.len() < policy.max_representatives {
        let Some((index, distance)) = min_distance
            .iter()
            .enumerate()
            .filter(|(index, _)| !chosen.contains(index))
            .max_by(|left, right| {
                left.1.total_cmp(right.1).then_with(|| {
                    stable_vector_key(&representative[left.0].0.1)
                        .cmp(&stable_vector_key(&representative[right.0].0.1))
                })
            })
        else {
            break;
        };
        if *distance <= policy.representative_distance_epsilon {
            break;
        }
        chosen.push(index);
        update_min_distances(&representative, &chosen, &mut min_distance);
    }

    let mut weights = vec![0.0; chosen.len()];
    let mut representatives = Vec::with_capacity(chosen.len());
    for chosen_index in &chosen {
        representatives.push(representative[*chosen_index].0.clone());
    }
    for ((_, vector), count) in &representative {
        let mut nearest = 0;
        let mut nearest_distance = f64::MAX;
        for (index, (_, selected_vector)) in representatives.iter().enumerate() {
            let distance = 1.0 - cosine(selected_vector, vector).clamp(-1.0, 1.0);
            if distance < nearest_distance {
                nearest_distance = distance;
                nearest = index;
            }
        }
        weights[nearest] += *count as f64;
    }

    let mut ordered = representatives.into_iter().enumerate().collect::<Vec<_>>();
    ordered.sort_by_key(|(_, (key, _))| *key);
    let mut representatives = Vec::with_capacity(ordered.len());
    let mut ordered_weights = Vec::with_capacity(ordered.len());
    for (index, (key, vector)) in ordered {
        representatives.push((key, vector));
        ordered_weights.push(weights[index]);
    }
    (representatives, ordered_weights)
}

type RepresentativeEntry = ((u64, Vec<f64>), u64);

fn update_min_distances(
    representative: &[RepresentativeEntry],
    chosen: &[usize],
    min_distance: &mut [f64],
) {
    let Some(last) = chosen.last().copied() else {
        return;
    };
    let last_vector = &representative[last].0.1;
    for (index, ((_, vector), _)) in representative.iter().enumerate() {
        if chosen.contains(&index) {
            continue;
        }
        let distance = 1.0 - cosine(last_vector, vector).clamp(-1.0, 1.0);
        min_distance[index] = min_distance[index].min(distance);
    }
}

fn stable_vector_key(vector: &[f64]) -> String {
    let bytes = vector
        .iter()
        .flat_map(|value| value.to_bits().to_le_bytes())
        .collect::<Vec<_>>();
    blake3::hash(&bytes).to_hex().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weighted_representatives_match_expanded_sample_pca() {
        let representatives = [
            vec![1.0, 0.0, 0.0],
            vec![0.0, 1.0, 0.0],
            vec![0.0, 0.0, 1.0],
        ];
        let multiplicities = [240usize, 40, 20];
        let mut expanded = Vec::new();
        let mut key = 0;
        for (vector, count) in representatives.iter().zip(multiplicities) {
            for _ in 0..count {
                expanded.push((key, vector.clone()));
                key += 1;
            }
        }
        let compressed = build_epa_basis(&expanded, "test-space", &EpaPolicy::reference())
            .expect("compressed EPA basis");
        let explicit_weights = vec![1.0; expanded.len()];
        let explicit = build_weighted_epa_basis(
            &expanded,
            &explicit_weights,
            "test-space",
            representatives[0].len(),
            &EpaPolicy::reference(),
        )
        .expect("expanded EPA basis");

        assert_eq!(compressed.mean, explicit.mean);
        assert_eq!(compressed.basis.len(), explicit.basis.len());
        for (compressed_axis, explicit_axis) in compressed.basis.iter().zip(&explicit.basis) {
            let alignment = compressed_axis
                .iter()
                .zip(explicit_axis)
                .map(|(left, right)| left * right)
                .sum::<f64>()
                .abs();
            assert!(alignment > 1.0 - 1e-10, "axis alignment={alignment}");
        }
        for (compressed_energy, explicit_energy) in compressed
            .singular_energies
            .iter()
            .zip(&explicit.singular_energies)
        {
            assert!((compressed_energy - explicit_energy).abs() < 1e-10);
        }
    }
}
