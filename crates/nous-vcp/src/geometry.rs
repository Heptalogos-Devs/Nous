//! Adapted from VCPToolBox rust-vexus-lite/src/memo_geometry.rs
//! Upstream commit d1b40c863273cbbba082fb7657671424768c4a53.
//! CC BY-NC-SA 4.0; see LICENSE-VCP and UPSTREAM.md.
//! Host/database access is replaced by immutable typed inputs.
//! Unified anchored greedy Gram-Schmidt geometry.
//! All floating-point reductions use ID order, never Rayon reduction order.

// Retain the upstream numerical decomposition and floating-point accumulation order.
#![allow(clippy::too_many_arguments)]
#[derive(Debug, Clone, Copy)]
pub enum AgsTarget<'a> {
    Explain(&'a [f64]),
    Span,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AgsCandidate {
    pub id: i64,
    pub prior: f64,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AgsTrace {
    pub picked: Vec<(i64, f64)>,
    pub basis: Vec<Vec<f64>>,
    pub residual_norm: Option<f64>,
}

/// Baseline-order sequential dot product used in rank-degenerate fallback paths
/// to preserve exact tie-breaking on symmetric near-collinear test matrices.
#[inline]
fn dot_exact(a: &[f64], b: &[f64]) -> f64 {
    a.iter().zip(b).fold(0.0, |sum, (x, y)| sum + x * y)
}

/// Sequential reduction shared with the baseline; not an exact-arithmetic dot product.
#[inline]
pub fn dot(a: &[f64], b: &[f64]) -> f64 {
    dot_exact(a, b)
}

/// Four independent accumulators sharing one pass over `vector`, preserving
/// each dot product's exact coordinate addition order bit-for-bit.
#[inline]
fn dot_four(vector: &[f64], [a, b, c, d]: [&[f64]; 4]) -> [f64; 4] {
    let mut sums = [0.0; 4];
    for (coordinate, &q) in vector.iter().enumerate() {
        sums[0] += q * a[coordinate];
        sums[1] += q * b[coordinate];
        sums[2] += q * c[coordinate];
        sums[3] += q * d[coordinate];
    }
    sums
}

fn normalize(vector: &mut [f64]) -> bool {
    let norm = dot_exact(vector, vector).sqrt();
    if !norm.is_finite() || norm <= 1e-12 {
        return false;
    }
    for value in vector {
        *value /= norm;
    }
    true
}

/// Subtract 4 basis projections in one pass over `residual`.
#[inline]
fn sub_four(residual: &mut [f64], [c0, c1, c2, c3]: [f64; 4], [a0, a1, a2, a3]: [&[f64]; 4]) {
    for (idx, value) in residual.iter_mut().enumerate() {
        let mut v = *value;
        v -= c0 * a0[idx];
        v -= c1 * a1[idx];
        v -= c2 * a2[idx];
        v -= c3 * a3[idx];
        *value = v;
    }
}

/// Construct a residual in the original space, then reorthogonalize once.
/// Uses `dot_exact` to preserve exact tie-breaking under severe cancellation (`near_1e-4..1e-7`).
fn orthogonal_residual(vector: &[f64], basis: &[Vec<f64>]) -> Vec<f64> {
    let mut residual = vector.to_vec();
    if basis.is_empty() {
        return residual;
    }
    for _ in 0..2 {
        for axis in basis {
            let coefficient = dot_exact(&residual, axis);
            for (value, q) in residual.iter_mut().zip(axis) {
                *value -= coefficient * q;
            }
        }
    }
    residual
}

/// Fused projection-cache + selective CGS/MGS2 reorthogonalization:
/// 1. Reuses cached first-pass inner products `<vector, b_m>` (zero dot products on pass 1!).
/// 2. Runs 4-way blocked CGS pass 2, or falls back to exact 2-pass MGS2 under severe cancellation.
fn orthogonal_residual_with_projections(
    vector: &[f64],
    basis: &[Vec<f64>],
    first_pass_coeffs: Option<&[f64]>,
    initial_norm_sq: f64,
    est_norm_sq: f64,
) -> (Vec<f64>, f64) {
    if basis.is_empty() {
        return (vector.to_vec(), initial_norm_sq.sqrt());
    }
    if first_pass_coeffs.is_none() || est_norm_sq <= 1e-3 * initial_norm_sq {
        let residual = orthogonal_residual(vector, basis);
        let norm = dot_exact(&residual, &residual).sqrt();
        return (residual, norm);
    }
    let coeffs = first_pass_coeffs.unwrap();
    let mut residual = vector.to_vec();
    // Pass 1: 4-way fused subtraction using already-computed `<vector, b_m>` (zero dot products!)
    let mut axis_groups = basis.chunks_exact(4);
    let mut coeff_groups = coeffs.chunks_exact(4);
    for (axes, c) in (&mut axis_groups).zip(&mut coeff_groups) {
        sub_four(
            &mut residual,
            [c[0], c[1], c[2], c[3]],
            [&axes[0], &axes[1], &axes[2], &axes[3]],
        );
    }
    for (axis, &c) in axis_groups.remainder().iter().zip(coeff_groups.remainder()) {
        for (value, q) in residual.iter_mut().zip(axis) {
            *value -= c * q;
        }
    }
    // Pass 2: 4-way blocked CGS reorthogonalization pass
    let mut axis_groups = basis.chunks_exact(4);
    for axes in &mut axis_groups {
        let refs = [
            axes[0].as_slice(),
            axes[1].as_slice(),
            axes[2].as_slice(),
            axes[3].as_slice(),
        ];
        let c = dot_four(&residual, refs);
        sub_four(&mut residual, c, refs);
    }
    for axis in axis_groups.remainder() {
        let c = dot(&residual, axis);
        for (value, q) in residual.iter_mut().zip(axis) {
            *value -= c * q;
        }
    }
    let norm = dot(&residual, &residual).sqrt();
    (residual, norm)
}

#[inline]
fn project_active_into(axis: &[f64], vectors: &[&[f64]], active: &[usize], row: &mut [f64]) {
    let mut groups = active.chunks_exact(4);
    for group in &mut groups {
        let [a, b, c, d] = [group[0], group[1], group[2], group[3]];
        let sums = dot_four(axis, [vectors[a], vectors[b], vectors[c], vectors[d]]);
        row[a] = sums[0];
        row[b] = sums[1];
        row[c] = sums[2];
        row[d] = sums[3];
    }
    for &index in groups.remainder() {
        row[index] = dot(axis, vectors[index]);
    }
}

fn project_active(axis: &[f64], vectors: &[&[f64]], active: &[usize]) -> Vec<f64> {
    let mut row = vec![0.0; vectors.len()];
    project_active_into(axis, vectors, active, &mut row);
    row
}

/// O(K*n*dim + K²*dim + K*n); basis vectors are already in original space.
/// Incremental scalar energy/correlation accumulators avoid O(K²*n) rescans while
/// preserving IEEE-754 left-fold addition order bit-for-bit. Near rank loss,
/// recompute the residual rather than trusting a difference of energies.
fn direct_axes_core(
    target: AgsTarget<'_>,
    candidates: &[AgsCandidate],
    vectors: &[&[f64]],
    precomputed_diagonal: Option<&[f64]>,
    precomputed_correlations: Option<&[f64]>,
    max_basis: usize,
    min_gain: f64,
    keep_basis: bool,
) -> AgsTrace {
    let n = candidates.len();
    assert_eq!(n, vectors.len(), "AGS candidate/vector mismatch");
    let dim = vectors.first().map_or(0, |v| v.len());
    assert!(
        vectors.iter().all(|v| v.len() == dim),
        "AGS dimension mismatch"
    );
    let target = match target {
        AgsTarget::Explain(t) => {
            assert!(
                vectors.is_empty() || t.len() == dim,
                "AGS target dimension mismatch"
            );
            Some(t)
        }
        AgsTarget::Span => None,
    };

    let owned_diagonal: Vec<f64>;
    let diagonal: &[f64] = match precomputed_diagonal {
        Some(d) => {
            assert_eq!(d.len(), n, "AGS diagonal length mismatch");
            d
        }
        None => {
            owned_diagonal = vectors.iter().map(|v| dot(v, v)).collect();
            &owned_diagonal
        }
    };

    let mut used: Vec<bool> = candidates
        .iter()
        .zip(diagonal)
        .map(|(c, norm)| !c.prior.is_finite() || c.prior <= 0.0 || !norm.is_finite())
        .collect();
    let mut active: Vec<usize> = (0..n).filter(|&j| !used[j]).collect();

    let owned_correlations: Vec<f64>;
    let correlations: &[f64] = match (target, precomputed_correlations) {
        (Some(_), Some(c)) => {
            assert_eq!(c.len(), n, "AGS correlations length mismatch");
            c
        }
        (Some(t), None) => {
            owned_correlations = project_active(t, vectors, &active);
            &owned_correlations
        }
        (None, _) => &[],
    };

    let limit = max_basis.min(n);
    let mut basis: Vec<Vec<f64>> = Vec::with_capacity(limit);
    let mut proj_history: Vec<f64> = Vec::new();
    let mut row_buf: Vec<f64> = Vec::new();
    let mut proj_sq_sum = vec![0.0; n];
    let mut proj_target_sum = vec![0.0; n];
    let mut target_residual = target.map(<[f64]>::to_vec);
    let mut picked = Vec::with_capacity(limit);

    while picked.len() < limit {
        let mut best: Option<(usize, f64)> = None;
        let mut best_pos = 0;
        let mut write_pos = 0;
        let current_k = basis.len();
        for read_pos in 0..active.len() {
            let i = active[read_pos];
            let norm_sq = diagonal[i] - proj_sq_sum[i];
            let numerator = target.map(|_| correlations[i] - proj_target_sum[i]);
            let (orth, signed) = if norm_sq <= 1e-8 * diagonal[i] {
                let residual = orthogonal_residual(vectors[i], &basis);
                let orth = dot_exact(&residual, &residual).sqrt();
                let signed = target.map_or(orth, |t| dot_exact(t, &residual) / orth);
                (orth, signed)
            } else {
                let orth = norm_sq.sqrt();
                (orth, numerator.map_or(orth, |value| value / orth))
            };
            if !orth.is_finite() || orth <= 1e-6 || !signed.is_finite() {
                used[i] = true;
                continue;
            }
            active[write_pos] = i;
            let score = signed.abs() * orth * candidates[i].prior;
            if best.as_ref().is_none_or(|(j, old)| {
                score > *old || (score == *old && candidates[i].id < candidates[*j].id)
            }) {
                best = Some((i, score));
                best_pos = write_pos;
            }
            write_pos += 1;
        }
        active.truncate(write_pos);
        let Some((i, _)) = best else { break };
        used[i] = true;
        active.remove(best_pos);

        let est_norm_sq = diagonal[i] - proj_sq_sum[i];
        let coeffs = (current_k > 0 && !proj_history.is_empty())
            .then(|| &proj_history[i * limit..i * limit + current_k]);
        let (mut axis, norm) = orthogonal_residual_with_projections(
            vectors[i],
            &basis,
            coeffs,
            diagonal[i],
            est_norm_sq,
        );
        if !norm.is_finite() || norm <= 1e-6 {
            continue;
        }
        for value in &mut axis {
            *value /= norm;
        }
        let coefficient = target.map(|t| dot(t, &axis));
        let gain = coefficient.map_or(norm, f64::abs);
        if gain < min_gain {
            break;
        }
        if let Some(coefficient) = coefficient {
            for (value, q) in target_residual.as_mut().unwrap().iter_mut().zip(&axis) {
                *value -= coefficient * q;
            }
        }
        picked.push((candidates[i].id, gain));
        if picked.len() < limit && !active.is_empty() {
            if proj_history.is_empty() {
                proj_history.resize(n * limit, 0.0);
                row_buf.resize(n, 0.0);
            }
            project_active_into(&axis, vectors, &active, &mut row_buf);
            if let Some(c) = coefficient {
                for &j in &active {
                    let p = row_buf[j];
                    proj_history[j * limit + current_k] = p;
                    proj_sq_sum[j] += p * p;
                    proj_target_sum[j] += p * c;
                }
            } else {
                for &j in &active {
                    let p = row_buf[j];
                    proj_history[j * limit + current_k] = p;
                    proj_sq_sum[j] += p * p;
                }
            }
        }
        basis.push(axis);
    }
    let residual_norm = target_residual.map(|residual| {
        let energy = dot(&residual, &residual);
        let original = target.map_or(0.0, |t| dot(t, t));
        if energy < 1e-14 * original {
            0.0
        } else {
            energy.sqrt()
        }
    });
    if !keep_basis {
        basis.clear();
    }
    AgsTrace {
        picked,
        basis,
        residual_norm,
    }
}

fn direct_axes(
    target: AgsTarget<'_>,
    candidates: &[AgsCandidate],
    vectors: &[&[f64]],
    max_basis: usize,
    min_gain: f64,
    keep_basis: bool,
) -> AgsTrace {
    direct_axes_core(
        target, candidates, vectors, None, None, max_basis, min_gain, keep_basis,
    )
}

/// Test and isolated-benchmark entry point; production uses precomputed inputs
/// through direct_axes_core. Internal axes still support reorthogonalization.
#[allow(dead_code)]
fn select_axes(
    target: AgsTarget<'_>,
    candidates: &[AgsCandidate],
    vectors: &[&[f64]],
    max_basis: usize,
    min_gain: f64,
) -> AgsTrace {
    direct_axes(target, candidates, vectors, max_basis, min_gain, false)
}

pub fn anchored_gs(
    target: AgsTarget<'_>,
    candidates: &[AgsCandidate],
    vectors: &[&[f64]],
    max_basis: usize,
    min_gain: f64,
) -> AgsTrace {
    direct_axes(target, candidates, vectors, max_basis, min_gain, true)
}

fn effective_k(coverage: &[f64], k_sem: usize, epsilon: f64, delta: f64) -> usize {
    let minimum = k_sem.min(coverage.len());
    for k in minimum.max(1)..=coverage.len() {
        let previous = if k > 1 { coverage[k - 2] } else { 0.0 };
        if 1.0 - coverage[k - 1] <= epsilon || coverage[k - 1] - previous < delta {
            return k;
        }
    }
    coverage.len()
}

// BEGIN TYPED GEOMETRY ASSETS

use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap};
const ALGORITHM: &str = "unified_geometry_ags_t4_cached_reorth";
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeometryTag {
    pub id: i64,
    pub name: String,
    pub vector_version: i64,
    pub vector: Vec<f32>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeometryInput {
    pub dimension: usize,
    pub model_sig: String,
    pub config: Value,
    pub tags: Vec<GeometryTag>,
    pub memberships: Vec<crate::artifact::FileTag>,
    pub recorded_at_ms: i64,
}
#[derive(Debug, Serialize)]
pub struct IntrinsicResidual {
    pub id: i64,
    pub status: &'static str,
    pub count: usize,
    pub residual: f64,
}
type LocalResult = IntrinsicResidual;
#[derive(Debug, Serialize)]
pub struct GeometryOutput {
    pub algorithm_version: &'static str,
    pub signature: String,
    pub config_hash: String,
    pub tag_count: usize,
    pub computed_count: usize,
    pub intrinsic: Vec<IntrinsicResidual>,
    pub anchor_gain: HashMap<i64, f64>,
    pub mean: Vec<f64>,
    pub basis: Vec<Vec<f64>>,
    pub coverage: Vec<f64>,
    pub epa_cache_json: String,
}
struct UnitVectorWorkspace {
    slots: HashMap<i64, usize>,
    vectors: Vec<Vec<f64>>,
    norms: Vec<f64>,
}
impl UnitVectorWorkspace {
    fn build(tags: &[GeometryTag], dimension: usize) -> Result<Self, String> {
        let mut out = Self {
            slots: HashMap::new(),
            vectors: Vec::new(),
            norms: Vec::new(),
        };
        for tag in tags {
            if tag.vector.len() != dimension || tag.vector.iter().any(|v| !v.is_finite()) {
                return Err(format!(
                    "geometry vector {} has invalid dimension or values",
                    tag.id
                ));
            }
            let mut vector: Vec<f64> = tag.vector.iter().map(|value| f64::from(*value)).collect();
            if !normalize(&mut vector) {
                continue;
            }
            out.slots.insert(tag.id, out.vectors.len());
            out.norms.push(dot(&vector, &vector));
            out.vectors.push(vector);
        }
        Ok(out)
    }
    fn len(&self) -> usize {
        self.vectors.len()
    }
    fn is_empty(&self) -> bool {
        self.vectors.is_empty()
    }
    fn contains_key(&self, id: &i64) -> bool {
        self.slots.contains_key(id)
    }
    fn slot(&self, id: &i64) -> Option<usize> {
        self.slots.get(id).copied()
    }
    fn get_by_slot(&self, slot: usize) -> &[f64] {
        &self.vectors[slot]
    }
    fn norm_by_slot(&self, slot: usize) -> f64 {
        self.norms[slot]
    }
    fn get(&self, id: &i64) -> Option<&[f64]> {
        self.slot(id).map(|slot| self.get_by_slot(slot))
    }
}
impl std::ops::Index<&i64> for UnitVectorWorkspace {
    type Output = [f64];
    fn index(&self, id: &i64) -> &[f64] {
        self.get(id).expect("known geometry vector")
    }
}
fn semantic_weight(
    sim: f64,
    enabled: bool,
    peak: f64,
    sigma: f64,
    floor: f64,
    hard_floor: f64,
) -> f64 {
    if !enabled {
        1.0
    } else if sim < hard_floor {
        0.0
    } else if sim <= 0.0 {
        floor
    } else {
        (0.5 + 0.8 * (-(sim - peak).powi(2) / (2.0 * sigma * sigma)).exp()).max(floor)
    }
}

fn star_direction(target: &[f64], mean: &[f64], neighbors: &[(&[f64], f64)], mix: f64) -> Vec<f64> {
    let weight_sum: f64 = neighbors.iter().map(|(_, w)| *w).sum();
    let mix = if weight_sum > 1e-12 { mix } else { 0.0 };
    let mut direction: Vec<f64> = target
        .iter()
        .zip(mean)
        .map(|(v, m)| (1.0 - mix) * (v - m))
        .collect();
    if mix > 0.0 {
        for (vector, weight) in neighbors {
            for ((out, v), m) in direction.iter_mut().zip(*vector).zip(mean) {
                *out += mix * weight / weight_sum * (v - m);
            }
        }
    }
    if !normalize(&mut direction) {
        direction = target.iter().zip(mean).map(|(v, m)| v - m).collect();
        normalize(&mut direction);
    }
    direction
}

fn number(v: &Value, key: &str, default: f64, min: f64, max: f64) -> f64 {
    v.get(key)
        .and_then(Value::as_f64)
        .filter(|x| x.is_finite())
        .unwrap_or(default)
        .clamp(min, max)
}

fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn dot_neighbors(target: &[f64], neighbor_vecs: &[&[f64]]) -> Vec<f64> {
    let mut out = vec![0.0; neighbor_vecs.len()];
    let mut groups = neighbor_vecs.chunks_exact(4).zip(out.chunks_exact_mut(4));
    for (vecs, dst) in &mut groups {
        let sums = dot_four(target, [vecs[0], vecs[1], vecs[2], vecs[3]]);
        dst.copy_from_slice(&sums);
    }
    let start = neighbor_vecs.len() & !3;
    for (dst, &v) in out[start..].iter_mut().zip(&neighbor_vecs[start..]) {
        *dst = dot(target, v);
    }
    out
}

fn accumulate_captured_energy(x: &[f64], basis: &[Vec<f64>], captured: &mut [f64]) {
    let mut axis_groups = basis.chunks_exact(4);
    let mut cap_groups = captured.chunks_exact_mut(4);
    for (axes, cap) in (&mut axis_groups).zip(&mut cap_groups) {
        let sums = dot_four(x, [&axes[0], &axes[1], &axes[2], &axes[3]]);
        cap[0] += sums[0].powi(2);
        cap[1] += sums[1].powi(2);
        cap[2] += sums[2].powi(2);
        cap[3] += sums[3].powi(2);
    }
    for (c, axis) in cap_groups
        .into_remainder()
        .iter_mut()
        .zip(axis_groups.remainder())
    {
        *c += dot(x, axis).powi(2);
    }
}

fn encode_vector(v: &[f64]) -> String {
    use base64::Engine;
    let bytes: Vec<u8> = v.iter().flat_map(|x| (*x as f32).to_le_bytes()).collect();
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// Compute upstream local/global AGS and EPA assets without reading storage.
pub fn build_geometry(mut input: GeometryInput) -> Result<GeometryOutput, String> {
    if input.dimension == 0
        || input.model_sig.trim().is_empty()
        || input.tags.iter().any(|tag| tag.id <= 0)
    {
        return Err("geometry requires dimension, model signature and positive Tag ids".into());
    }
    input.tags.sort_by_key(|tag| tag.id);
    if input.tags.windows(2).any(|pair| pair[0].id == pair[1].id) {
        return Err("duplicate geometry Tag".into());
    }
    let declared: std::collections::HashSet<i64> = input.tags.iter().map(|tag| tag.id).collect();
    if input
        .memberships
        .iter()
        .any(|row| row.file_id <= 0 || !declared.contains(&row.tag_id))
    {
        return Err("MEMO_GEOMETRY_MEMBERSHIP_INVALID: memberships require positive files and declared Tags".into());
    }
    input
        .memberships
        .sort_by_key(|row| (row.file_id, row.position, row.tag_id));
    let config = &input.config;
    let local = config.get("local").unwrap_or(config);
    let global = config.get("global").unwrap_or(&Value::Null);
    let max_neighbors = number(local, "maxNeighbors", 48.0, 1.0, 256.0) as usize;
    let min_neighbors = number(local, "minNeighbors", 3.0, 1.0, 256.0) as usize;
    let max_basis = number(local, "maxBasis", 4.0, 1.0, 32.0) as usize;
    let min_gain = number(local, "minGain", 0.015, 0.0, 1.0);
    let decay = number(local, "positionDecay", 0.15, 0.0, 4.0);
    let k_max = number(global, "kMax", 32.0, 1.0, 128.0) as usize;
    let k_sem = number(global, "kSem", 8.0, 1.0, k_max as f64) as usize;
    let mut signature_config = config.clone();
    if let Some(object) = signature_config.as_object_mut() {
        object.remove("forceRecompute");
    }
    let config_hash = hash(signature_config.to_string().as_bytes());
    let mut digest = Sha256::new();
    let mut adjacency: BTreeMap<i64, BTreeMap<i64, f64>> = BTreeMap::new();
    let mut ids = Vec::new();
    let mut names = HashMap::new();
    for tag in &input.tags {
        digest.update(tag.id.to_le_bytes());
        digest.update(tag.vector_version.to_le_bytes());
        digest.update((tag.name.len() as u64).to_le_bytes());
        digest.update(tag.name.as_bytes());
        ids.push(tag.id);
        names.insert(tag.id, tag.name.clone());
    }
    let flush = |tags: &[(i64, i64)], graph: &mut BTreeMap<i64, BTreeMap<i64, f64>>| {
        if !(2..=100).contains(&tags.len()) {
            return;
        }
        for i in 0..tags.len() {
            for j in i + 1..tags.len() {
                let (a, p) = tags[i];
                let (b, q) = tags[j];
                if a == b {
                    continue;
                }
                let delta = if p > 0 && q > 0 {
                    (p - q).abs().max(1) as f64
                } else {
                    1.0
                };
                let weight = (-decay * (delta - 1.0)).exp();
                *graph.entry(a).or_default().entry(b).or_default() += weight;
                *graph.entry(b).or_default().entry(a).or_default() += weight;
            }
        }
    };

    let mut file = None;
    let mut tags = Vec::new();
    for row in &input.memberships {
        let (fid, id, pos) = (row.file_id, row.tag_id, row.position);
        digest.update(fid.to_le_bytes());
        digest.update(id.to_le_bytes());
        digest.update(pos.to_le_bytes());
        if file != Some(fid) {
            flush(&tags, &mut adjacency);
            tags.clear();
            file = Some(fid);
        }
        if tags.len() <= 100 {
            tags.push((id, pos));
        }
    }
    flush(&tags, &mut adjacency);
    let content = format!("{:x}", digest.finalize());
    let signature = hash(
        format!(
            "{}|{}|{}|{}|{}",
            ALGORITHM, input.dimension, input.model_sig, config_hash, content
        )
        .as_bytes(),
    );
    let vectors = UnitVectorWorkspace::build(&input.tags, input.dimension)?;
    let star_neighbors = number(global, "starNeighbors", max_neighbors as f64, 1.0, 256.0) as usize;
    let max_keep = max_neighbors.max(star_neighbors);
    // Pre-truncated, pre-sorted Slot-CSR: filter invalid vectors and truncate to `max_keep` once!
    let mut offsets = vec![0];
    let mut edges: Vec<(i64, usize, f64)> = Vec::new();
    let mut support = Vec::with_capacity(ids.len());
    let cmp_edge =
        |a: &(i64, usize, f64), b: &(i64, usize, f64)| b.2.total_cmp(&a.2).then(a.0.cmp(&b.0));
    for id in &ids {
        let neighbors = adjacency.remove(id).unwrap_or_default();
        support.push(neighbors.values().sum::<f64>());
        if vectors.contains_key(id) {
            let mut valid_edges: Vec<(i64, usize, f64)> = neighbors
                .into_iter()
                .filter_map(|(nid, w)| vectors.slot(&nid).map(|slot| (nid, slot, w)))
                .collect();
            if valid_edges.len() > max_keep {
                valid_edges.select_nth_unstable_by(max_keep - 1, cmp_edge);
                valid_edges.truncate(max_keep);
            }
            valid_edges.sort_unstable_by(cmp_edge);
            edges.extend(valid_edges);
        }
        offsets.push(edges.len());
    }
    drop(adjacency);
    let gate = local.get("semanticGate").unwrap_or(local);
    let gate_enabled = gate
        .get("enabled")
        .or_else(|| gate.get("semanticEnabled"))
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let peak = number(
        gate,
        "peak",
        number(local, "semanticPeak", 0.65, -1.0, 1.0),
        -1.0,
        1.0,
    );
    let sigma = number(
        gate,
        "sigma",
        number(local, "semanticSigma", 0.25, 0.02, 2.0),
        0.02,
        2.0,
    );
    let floor = number(
        gate,
        "floor",
        number(local, "semanticFloor", 0.35, 0.0, 1.0),
        0.0,
        1.0,
    );
    let hard_floor = number(
        gate,
        "hardFloor",
        number(local, "semanticHardFloor", -1.0, -1.0, 1.0),
        -1.0,
        1.0,
    );
    let results: Vec<LocalResult> = ids
        .par_iter()
        .enumerate()
        .map(|(i, id)| {
            let mut result = LocalResult {
                id: *id,
                status: "failed",
                count: 0,
                residual: 1.0,
            };
            let Some(target_slot) = vectors.slot(id) else {
                return result;
            };
            let target = vectors.get_by_slot(target_slot);
            let end = offsets[i + 1].min(offsets[i] + max_neighbors);
            let neighbors = &edges[offsets[i]..end];
            let neighbor_vecs: Vec<&[f64]> = neighbors
                .iter()
                .map(|&(_, slot, _)| vectors.get_by_slot(slot))
                .collect();
            let raw_dots = dot_neighbors(target, &neighbor_vecs);
            let mut candidates = Vec::with_capacity(neighbors.len());
            let mut refs = Vec::with_capacity(neighbors.len());
            let mut diagonal = Vec::with_capacity(neighbors.len());
            let mut correlations = Vec::with_capacity(neighbors.len());
            for (&(nid, slot, w), (&v, raw_dot)) in
                neighbors.iter().zip(neighbor_vecs.iter().zip(raw_dots))
            {
                let sim = raw_dot.clamp(-1.0, 1.0);
                let semantic = semantic_weight(sim, gate_enabled, peak, sigma, floor, hard_floor);
                if semantic > 0.0 {
                    candidates.push(AgsCandidate {
                        id: nid,
                        prior: w.ln_1p() * semantic,
                    });
                    refs.push(v);
                    diagonal.push(vectors.norm_by_slot(slot));
                    correlations.push(raw_dot);
                }
            }
            result.count = candidates.len();
            if result.count < min_neighbors {
                result.status = "insufficient_neighbors";
                return result;
            }
            let trace = direct_axes_core(
                AgsTarget::Explain(target),
                &candidates,
                &refs,
                Some(&diagonal),
                Some(&correlations),
                max_basis,
                min_gain,
                false,
            );
            result.residual = trace.residual_norm.unwrap_or(1.0).clamp(0.0, 1.0);
            result.status = "computed";
            result
        })
        .collect();

    let mut mean = vec![0.0; input.dimension];
    for slot in 0..vectors.len() {
        let v = vectors.get_by_slot(slot);
        for (m, x) in mean.iter_mut().zip(v) {
            *m += x;
        }
    }
    if !vectors.is_empty() {
        for m in &mut mean {
            *m /= vectors.len() as f64;
        }
    }
    let min_support = number(global, "minSupport", 3.0, 0.0, 1e12);
    let a = number(global, "supportExponent", 0.65, 0.0, 4.0);
    let b = number(global, "residualExponent", 0.35, 0.0, 4.0);
    let prior_mode = global
        .get("priorMode")
        .and_then(Value::as_str)
        .unwrap_or("support");
    if !["support", "explained", "residual"].contains(&prior_mode) {
        return Err("geometry global.priorMode must be support, explained or residual".into());
    }
    let candidate_mode = global
        .get("candidateMode")
        .and_then(Value::as_str)
        .unwrap_or("star");
    if !["star", "anchor"].contains(&candidate_mode) {
        return Err("geometry global.candidateMode must be star or anchor".into());
    }
    let mut pool: Vec<_> = results
        .iter()
        .enumerate()
        .filter(|(i, r)| r.status == "computed" && support[*i] >= min_support)
        .map(|(i, r)| {
            let factor = match prior_mode {
                "explained" => (1.0 - r.residual * r.residual).max(0.0).powf(b),
                "residual" => r.residual.powf(b),
                _ => 1.0,
            };
            AgsCandidate {
                id: r.id,
                prior: support[i].powf(a) * factor,
            }
        })
        .collect();
    pool.sort_by(|a, b| b.prior.total_cmp(&a.prior).then(a.id.cmp(&b.id)));
    pool.truncate(number(global, "candidatePool", 512.0, 1.0, 4096.0) as usize);
    let mix = number(global, "starMix", 0.8, 0.0, 1.0);
    let centered: Vec<Vec<f64>> = pool
        .par_iter()
        .map(|c| {
            let target = &vectors[&c.id];
            let i = ids.binary_search(&c.id).unwrap();
            let end = offsets[i + 1].min(offsets[i] + star_neighbors);
            let neighbors = &edges[offsets[i]..end];
            let neighbor_vecs: Vec<&[f64]> = neighbors
                .iter()
                .map(|&(_, slot, _)| vectors.get_by_slot(slot))
                .collect();
            let raw_dots = dot_neighbors(target, &neighbor_vecs);
            let weighted: Vec<_> = neighbors
                .iter()
                .zip(neighbor_vecs.iter().zip(raw_dots))
                .filter_map(|(&(_, _, w), (&v, raw_dot))| {
                    let weight = w.ln_1p()
                        * semantic_weight(
                            raw_dot.clamp(-1.0, 1.0),
                            gate_enabled,
                            peak,
                            sigma,
                            floor,
                            hard_floor,
                        );
                    (weight > 0.0).then_some((v, weight))
                })
                .collect();
            star_direction(
                target,
                &mean,
                &weighted,
                if candidate_mode == "star" { mix } else { 0.0 },
            )
        })
        .collect();
    let refs: Vec<_> = centered.iter().map(Vec::as_slice).collect();
    let mut trace = anchored_gs(
        AgsTarget::Span,
        &pool,
        &refs,
        k_max,
        number(global, "minGain", 0.02, 0.0, 1.0),
    );
    drop(refs);
    drop(centered);
    drop(pool);
    drop(edges);
    drop(offsets);
    drop(support);

    // Fixed-size chunks + indexed collect + serial reduction = thread-count-independent bits.
    let partial: Vec<_> = ids
        .par_chunks(128)
        .map(|batch| {
            let mut energy = 0.0;
            let mut captured = vec![0.0; trace.basis.len()];
            let mut x = vec![0.0; input.dimension];
            for id in batch {
                if let Some(v) = vectors.get(id) {
                    for ((out, &val), &m) in x.iter_mut().zip(v).zip(&mean) {
                        *out = val - m;
                    }
                    energy += dot(&x, &x);
                    accumulate_captured_energy(&x, &trace.basis, &mut captured);
                }
            }
            (energy, captured)
        })
        .collect();
    drop(vectors);
    let mut total = 0.0;
    let mut energies = vec![0.0; trace.basis.len()];
    for (energy, values) in partial {
        total += energy;
        for (e, v) in energies.iter_mut().zip(values) {
            *e += v;
        }
    }
    let mut cumulative = 0.0;
    let mut coverage: Vec<_> = energies
        .iter()
        .map(|e| {
            cumulative += e;
            if total > 1e-12 {
                (cumulative / total).clamp(0.0, 1.0)
            } else {
                0.0
            }
        })
        .collect();
    let full_coverage = coverage.clone();
    let k = effective_k(
        &coverage,
        k_sem,
        number(global, "epsilon", 0.1, 0.0, 1.0),
        number(global, "kneeDelta", 0.0, 0.0, 1.0),
    );
    trace.basis.truncate(k);
    trace.picked.truncate(k);
    energies.truncate(k);
    coverage.truncate(k);
    let now = input.recorded_at_ms;
    let cache=json!({
            "basis":trace.basis.iter().map(|v|encode_vector(v)).collect::<Vec<_>>(),
            "mean":encode_vector(&mean),"energies":energies,"coverage":coverage,
            "labels":trace.picked.iter().map(|(id,_)|names[id].clone()).collect::<Vec<_>>(),
            "anchorTagIds":trace.picked.iter().map(|(id,_)|*id).collect::<Vec<_>>(),
            "totalEnergy":total,"kSem":k_sem,"kMax":k_max,"epaAlgorithm":"ags-global-star-t4-cached-reorth",
            "candidateMode":candidate_mode,"priorMode":prior_mode,"fullCoverage":full_coverage,
            "artifactSig":signature,"tagCount":ids.len(),"timestamp":now
        }).to_string();

    let computed = results.iter().filter(|r| r.status == "computed").count();
    let anchor = local.get("anchorGain").unwrap_or(local);
    let base = number(
        anchor,
        "base",
        number(local, "v9AnchorBase", 0.75, 0.0, 4.0),
        0.0,
        4.0,
    );
    let scale = number(
        anchor,
        "scale",
        number(local, "v9AnchorScale", 1.25, 0.0, 4.0),
        0.0,
        4.0,
    );
    let gamma = number(
        anchor,
        "gamma",
        number(local, "v9AnchorGamma", 1.0, 0.1, 8.0),
        0.1,
        8.0,
    );
    let low = number(
        anchor,
        "min",
        number(local, "v9AnchorMin", 0.5, 0.0, 4.0),
        0.0,
        4.0,
    );
    let high = number(
        anchor,
        "max",
        number(local, "v9AnchorMax", 2.0, 0.0, 8.0),
        0.0,
        8.0,
    );

    let anchor_gain = results
        .iter()
        .filter(|r| r.status == "computed")
        .map(|r| {
            (
                r.id,
                (base + scale * r.residual.powf(gamma)).clamp(low.min(high), low.max(high)),
            )
        })
        .collect();
    Ok(GeometryOutput {
        algorithm_version: ALGORITHM,
        signature,
        config_hash,
        tag_count: ids.len(),
        computed_count: computed,
        intrinsic: results,
        anchor_gain,
        mean,
        basis: trace.basis,
        coverage,
        epa_cache_json: cache,
    })
}
