use nalgebra::DMatrix;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Deserialize)]
pub struct ReferenceEpaTrainingInput {
    pub dimension: usize,
    pub vectors: Vec<Vec<f32>>,
    pub names: Vec<String>,
    pub requested_anchors: usize,
    pub max_basis: usize,
    pub samples_per_anchor: usize,
    pub candidate_limit: usize,
}
#[derive(Debug, Serialize, Deserialize)]
pub struct ReferenceEpaTraining {
    pub success: bool,
    pub density_mean: Vec<f32>,
    pub bucket_keys: Vec<u16>,
    pub centroids: Vec<Vec<f32>>,
    pub weights: Vec<usize>,
    pub labels: Vec<String>,
    pub representative_count: usize,
    pub bucket_count: usize,
    pub mean: Vec<f32>,
    pub basis: Vec<Vec<f32>>,
    pub energies: Vec<f64>,
}
fn norm_f32(v: &mut [f32]) {
    let m = v.iter().map(|x| f64::from(*x).powi(2)).sum::<f64>().sqrt();
    if m > 1e-9 {
        for x in v {
            *x = (f64::from(*x) / m) as f32;
        }
    }
}
fn key(v: &[f32], mean: &[f32]) -> u16 {
    let mut key = 0;
    for bit in 0..12 {
        let mut state = (bit as u64 + 1).wrapping_mul(0x9E37_79B9_7F4A_7C15);
        let mut sum = 0.0;
        for _ in 0..16 {
            state ^= state >> 12;
            state ^= state << 25;
            state ^= state >> 27;
            let i = state as usize % v.len();
            let sign = if state & 0x8000_0000_0000_0000 == 0 {
                1.0
            } else {
                -1.0
            };
            sum += f64::from(v[i] - mean[i]) * sign;
        }
        if sum >= 0.0 {
            key |= 1 << bit;
        }
    }
    key
}
struct Bucket {
    count: usize,
    sum: Vec<f32>,
    best: usize,
    residual: f64,
    samples: Vec<(usize, f64)>,
}
struct Candidate {
    key: u16,
    density: usize,
    centroid: Vec<f32>,
    label: usize,
    score: f64,
    centered: Vec<f64>,
    max_sim: f64,
}
fn buckets(
    vectors: &[Vec<f32>],
    mean: &[f32],
    sample_count: usize,
) -> (BTreeMap<u16, Bucket>, Vec<u16>) {
    let mut buckets = BTreeMap::<u16, Bucket>::new();
    let mut keys = Vec::new();
    for (i, v) in vectors.iter().enumerate() {
        let k = key(v, mean);
        keys.push(k);
        let residual = v
            .iter()
            .zip(mean)
            .map(|(v, m)| f64::from(v - m).powi(2))
            .sum::<f64>()
            .sqrt();
        let b = buckets.entry(k).or_insert_with(|| Bucket {
            count: 0,
            sum: vec![0.0; v.len()],
            best: i,
            residual,
            samples: Vec::new(),
        });
        b.count += 1;
        for (s, v) in b.sum.iter_mut().zip(v) {
            *s += v;
        }
        if residual > b.residual {
            b.residual = residual;
            b.best = i;
        }
        let index = b
            .samples
            .binary_search_by(|(_, r)| r.total_cmp(&residual).reverse())
            .unwrap_or_else(|i| i);
        if index < sample_count {
            b.samples.insert(index, (i, residual));
            b.samples.truncate(sample_count);
        } else if b.samples.len() < sample_count {
            b.samples.push((i, residual));
        }
    }
    (buckets, keys)
}
fn select(
    buckets: &BTreeMap<u16, Bucket>,
    mean: &[f32],
    anchors: usize,
    limit: usize,
) -> Vec<Candidate> {
    let mut candidates = buckets
        .iter()
        .map(|(key, b)| {
            let mut centroid = b.sum.iter().map(|x| x / b.count as f32).collect::<Vec<_>>();
            norm_f32(&mut centroid);
            let mut centered = centroid
                .iter()
                .zip(mean)
                .map(|(x, m)| f64::from(x - m))
                .collect::<Vec<_>>();
            let norm = centered.iter().map(|x| x * x).sum::<f64>().sqrt();
            if norm > 1e-12 {
                for x in &mut centered {
                    *x /= norm;
                }
            }
            Candidate {
                key: *key,
                density: b.count,
                centroid,
                label: b.best,
                score: (b.count as f64).powf(0.65) * b.residual.max(1e-9).powf(0.35),
                centered,
                max_sim: 0.0,
            }
        })
        .collect::<Vec<_>>();
    candidates.sort_by(|a, b| b.score.total_cmp(&a.score));
    candidates.truncate(limit);
    let mut selected = Vec::new();
    while selected.len() < anchors && !candidates.is_empty() {
        let mut best = 0;
        let mut score = f64::MIN;
        for (i, c) in candidates.iter().enumerate() {
            let s = c.score * (-3.0 * c.max_sim * c.max_sim).exp();
            if s > score {
                score = s;
                best = i;
            }
        }
        let chosen = candidates.swap_remove(best);
        for c in &mut candidates {
            let similarity = c
                .centered
                .iter()
                .zip(&chosen.centered)
                .map(|(a, b)| a * b)
                .sum::<f64>()
                .max(0.0);
            c.max_sim = c.max_sim.max(similarity);
        }
        selected.push(chosen);
    }
    selected
}
fn basis(output: &mut ReferenceEpaTraining, dimension: usize, max_basis: usize) {
    let total = output.weights.iter().sum::<usize>();
    if total == 0 {
        return;
    }
    output.mean = vec![0.0; dimension];
    for (c, w) in output.centroids.iter().zip(&output.weights) {
        for (m, v) in output.mean.iter_mut().zip(c) {
            *m += v * (*w as f32);
        }
    }
    for m in &mut output.mean {
        *m /= total as f32;
    }
    let flat = output
        .centroids
        .iter()
        .zip(&output.weights)
        .flat_map(|(c, w)| {
            c.iter()
                .zip(&output.mean)
                .map(move |(c, m)| (c - m) * (*w as f32).sqrt())
        })
        .collect::<Vec<_>>();
    let decomposition =
        DMatrix::from_row_slice(output.centroids.len(), dimension, &flat).svd(false, true);
    let Some(vt) = decomposition.v_t else {
        return;
    };
    let max = max_basis
        .min(decomposition.singular_values.len())
        .min(output.centroids.len());
    let total_energy = decomposition
        .singular_values
        .iter()
        .take(max)
        .map(|v| f64::from(*v).powi(2))
        .sum::<f64>();
    let mut count = max;
    let mut cumulative = 0.0;
    if total_energy > 1e-12 {
        for (i, v) in decomposition.singular_values.iter().take(max).enumerate() {
            cumulative += f64::from(*v).powi(2);
            if cumulative / total_energy > 0.95 {
                count = (i + 1).max(8.min(max));
                break;
            }
        }
    }
    for i in 0..count {
        let mut axis = (0..dimension).map(|j| vt[(i, j)]).collect::<Vec<_>>();
        norm_f32(&mut axis);
        output.basis.push(axis);
        output
            .energies
            .push(f64::from(decomposition.singular_values[i]).powi(2));
    }
    output.success = true;
}
pub fn reference_train_epa(input: &ReferenceEpaTrainingInput) -> ReferenceEpaTraining {
    let valid = input
        .vectors
        .iter()
        .enumerate()
        .filter(|(_, v)| v.len() == input.dimension)
        .collect::<Vec<_>>();
    let names = valid
        .iter()
        .map(|(i, _)| {
            input
                .names
                .get(*i)
                .cloned()
                .unwrap_or_else(|| "Unknown".into())
        })
        .collect::<Vec<_>>();
    let mut vectors = valid
        .into_iter()
        .map(|(_, v)| v.clone())
        .collect::<Vec<_>>();
    for v in &mut vectors {
        norm_f32(v);
    }
    let mut output = ReferenceEpaTraining {
        success: false,
        density_mean: Vec::new(),
        bucket_keys: Vec::new(),
        centroids: Vec::new(),
        weights: Vec::new(),
        labels: Vec::new(),
        representative_count: 0,
        bucket_count: 0,
        mean: Vec::new(),
        basis: Vec::new(),
        energies: Vec::new(),
    };
    if input.dimension == 0 || vectors.len() < 8 {
        return output;
    }
    let mut mean = vec![0.0; input.dimension];
    for v in &vectors {
        for (m, v) in mean.iter_mut().zip(v) {
            *m += v;
        }
    }
    for m in &mut mean {
        *m /= vectors.len() as f32;
    }
    let anchors = input
        .requested_anchors
        .min(vectors.len())
        .clamp(8, 128)
        .min(vectors.len());
    let (buckets, keys) = buckets(&vectors, &mean, input.samples_per_anchor.clamp(4, 128));
    let selected = select(
        &buckets,
        &mean,
        anchors,
        input.candidate_limit.clamp(anchors, 4096),
    );
    let mut representatives = BTreeSet::new();
    for c in selected {
        representatives.insert(c.label);
        for (i, _) in &buckets[&c.key].samples {
            representatives.insert(*i);
        }
        output.labels.push(
            names
                .get(c.label)
                .cloned()
                .unwrap_or_else(|| "Unknown".into()),
        );
        output.weights.push(c.density.max(1));
        output.centroids.push(c.centroid);
    }
    output.representative_count = representatives.len();
    output.bucket_count = buckets.len();
    output.density_mean = mean;
    output.bucket_keys = keys;
    basis(&mut output, input.dimension, input.max_basis);
    output
}
