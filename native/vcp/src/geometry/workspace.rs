//! Unit vector workspace and geometric reductions.
use super::*;

pub(super) struct UnitVectorWorkspace {
    pub(super) slots: HashMap<i64, usize>,
    pub(super) vectors: Vec<Vec<f64>>,
    pub(super) norms: Vec<f64>,
}
impl UnitVectorWorkspace {
    pub(super) fn build(tags: &[GeometryTag], dimension: usize) -> Result<Self, String> {
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
    pub(super) fn len(&self) -> usize {
        self.vectors.len()
    }
    pub(super) fn is_empty(&self) -> bool {
        self.vectors.is_empty()
    }
    pub(super) fn contains_key(&self, id: &i64) -> bool {
        self.slots.contains_key(id)
    }
    pub(super) fn slot(&self, id: &i64) -> Option<usize> {
        self.slots.get(id).copied()
    }
    pub(super) fn get_by_slot(&self, slot: usize) -> &[f64] {
        &self.vectors[slot]
    }
    pub(super) fn norm_by_slot(&self, slot: usize) -> f64 {
        self.norms[slot]
    }
    pub(super) fn get(&self, id: &i64) -> Option<&[f64]> {
        self.slot(id).map(|slot| self.get_by_slot(slot))
    }
}
impl std::ops::Index<&i64> for UnitVectorWorkspace {
    type Output = [f64];
    fn index(&self, id: &i64) -> &[f64] {
        self.get(id).expect("known geometry vector")
    }
}
pub(super) fn semantic_weight(
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

pub(super) fn star_direction(
    target: &[f64],
    mean: &[f64],
    neighbors: &[(&[f64], f64)],
    mix: f64,
) -> Vec<f64> {
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

pub(super) fn number(v: &Value, key: &str, default: f64, min: f64, max: f64) -> f64 {
    v.get(key)
        .and_then(Value::as_f64)
        .filter(|x| x.is_finite())
        .unwrap_or(default)
        .clamp(min, max)
}

pub(super) fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

pub(super) fn dot_neighbors(target: &[f64], neighbor_vecs: &[&[f64]]) -> Vec<f64> {
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

pub(super) fn accumulate_captured_energy(x: &[f64], basis: &[Vec<f64>], captured: &mut [f64]) {
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

pub(super) fn encode_vector(v: &[f64]) -> String {
    use base64::Engine;
    let bytes: Vec<u8> = v.iter().flat_map(|x| (*x as f32).to_le_bytes()).collect();
    base64::engine::general_purpose::STANDARD.encode(bytes)
}
