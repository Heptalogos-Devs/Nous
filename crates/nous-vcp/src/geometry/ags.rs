//! Anchored greedy Gram-Schmidt and deterministic rank selection.
// Adapted from VCPToolBox d1b40c863273cbbba082fb7657671424768c4a53.
// CC BY-NC-SA 4.0; see LICENSE-VCP and UPSTREAM.md.

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
pub(super) fn dot_exact(a: &[f64], b: &[f64]) -> f64 {
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
pub(super) fn dot_four(vector: &[f64], [a, b, c, d]: [&[f64]; 4]) -> [f64; 4] {
    let mut sums = [0.0; 4];
    for (coordinate, &q) in vector.iter().enumerate() {
        sums[0] += q * a[coordinate];
        sums[1] += q * b[coordinate];
        sums[2] += q * c[coordinate];
        sums[3] += q * d[coordinate];
    }
    sums
}

pub(super) fn normalize(vector: &mut [f64]) -> bool {
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
pub(super) fn sub_four(
    residual: &mut [f64],
    [c0, c1, c2, c3]: [f64; 4],
    [a0, a1, a2, a3]: [&[f64]; 4],
) {
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
pub(super) fn orthogonal_residual(vector: &[f64], basis: &[Vec<f64>]) -> Vec<f64> {
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
pub(super) fn orthogonal_residual_with_projections(
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
pub(super) fn project_active_into(
    axis: &[f64],
    vectors: &[&[f64]],
    active: &[usize],
    row: &mut [f64],
) {
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

pub(super) fn project_active(axis: &[f64], vectors: &[&[f64]], active: &[usize]) -> Vec<f64> {
    let mut row = vec![0.0; vectors.len()];
    project_active_into(axis, vectors, active, &mut row);
    row
}

/// O(K*n*dim + K²*dim + K*n); basis vectors are already in original space.
/// Incremental scalar energy/correlation accumulators avoid O(K²*n) rescans while
/// preserving IEEE-754 left-fold addition order bit-for-bit. Near rank loss,
/// recompute the residual rather than trusting a difference of energies.
pub(super) fn direct_axes_core(
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

pub(super) fn direct_axes(
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
pub(super) fn select_axes(
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

pub(super) fn effective_k(coverage: &[f64], k_sem: usize, epsilon: f64, delta: f64) -> usize {
    let minimum = k_sem.min(coverage.len());
    for k in minimum.max(1)..=coverage.len() {
        let previous = if k > 1 { coverage[k - 2] } else { 0.0 };
        if 1.0 - coverage[k - 1] <= epsilon || coverage[k - 1] - previous < delta {
            return k;
        }
    }
    coverage.len()
}
