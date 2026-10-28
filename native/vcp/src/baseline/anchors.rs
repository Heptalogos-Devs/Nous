use super::unit;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReferenceCurveTag {
    pub id: i64,
    pub vector: Vec<f32>,
    pub position: i64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReferenceCurve {
    pub id: i64,
    pub chunk_vector: Vec<f32>,
    pub tags: Vec<ReferenceCurveTag>,
}
#[derive(Debug, Deserialize)]
pub struct ReferenceAnchorInput {
    pub curves: Vec<ReferenceCurve>,
    pub seeds: Vec<(i64, f64)>,
    pub seed_vectors: Vec<(i64, Vec<f32>)>,
    pub inbound: Vec<(i64, f64)>,
    pub fallback: bool,
    pub config: ReferenceAnchorConfig,
}
#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(default, rename_all = "camelCase")]
pub struct ReferenceAnchorConfig {
    pub semantic_anchor_threshold: f64,
    pub semantic_anchor_discount: f64,
    pub specificity_floor: f64,
    pub rarity_floor: f64,
    pub reliability_seed_saturation: f64,
    pub fallback_reliability_cap: f64,
}
impl Default for ReferenceAnchorConfig {
    fn default() -> Self {
        Self {
            semantic_anchor_threshold: 0.8,
            semantic_anchor_discount: 0.7,
            specificity_floor: 0.35,
            rarity_floor: 0.15,
            reliability_seed_saturation: 2.0,
            fallback_reliability_cap: 0.5,
        }
    }
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReferenceAnchor {
    pub id: i64,
    pub score: f64,
    pub reliability: f64,
    pub strength: f64,
    pub contacted_seeds: usize,
    pub exact_contacts: usize,
    pub semantic_contacts: usize,
    pub mean_closure: f64,
}
/// Positive and signed cosine conventions are selected by the readout caller.
pub(crate) fn reference_cosine(a: &[f32], b: &[f32]) -> f64 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let mut cross = 0.0;
    let mut aa = 0.0;
    let mut bb = 0.0;
    for (a, b) in a.iter().zip(b) {
        let a = f64::from(*a);
        let b = f64::from(*b);
        cross += a * b;
        aa += a * a;
        bb += b * b;
    }
    if aa > 1e-15 && bb > 1e-15 {
        cross / (aa.sqrt() * bb.sqrt())
    } else {
        0.0
    }
}
struct Contact {
    seed: i64,
    tag: i64,
    exact: bool,
    mass: f64,
    closure: f64,
}
fn contacts(
    curve: &ReferenceCurve,
    input: &ReferenceAnchorInput,
    vectors: &BTreeMap<i64, &[f32]>,
) -> Vec<Contact> {
    let mut contacts = Vec::new();
    for (id, mass) in &input.seeds {
        if let Some(tag) = curve.tags.iter().find(|tag| tag.id == *id) {
            contacts.push(Contact {
                seed: *id,
                tag: tag.id,
                exact: true,
                mass: *mass,
                closure: unit(reference_cosine(&tag.vector, &curve.chunk_vector)),
            });
            continue;
        }
        let Some(vector) = vectors.get(id) else {
            continue;
        };
        let matched = curve
            .tags
            .iter()
            .map(|tag| (tag, reference_cosine(vector, &tag.vector)))
            .filter(|(_, similarity)| *similarity >= input.config.semantic_anchor_threshold)
            .max_by(|a, b| a.1.total_cmp(&b.1));
        if let Some((tag, _)) = matched {
            contacts.push(Contact {
                seed: *id,
                tag: tag.id,
                exact: false,
                mass: *mass,
                closure: unit(reference_cosine(&tag.vector, &curve.chunk_vector)),
            });
        }
    }
    contacts
}

/// Pool rarity is computed over the complete selected curve set before any
/// per-candidate readout; no candidate can change the source observation.
pub fn reference_anchors(input: &ReferenceAnchorInput) -> Vec<ReferenceAnchor> {
    let vectors = input
        .seed_vectors
        .iter()
        .map(|(id, v)| (*id, v.as_slice()))
        .collect::<BTreeMap<_, _>>();
    let inbound = input.inbound.iter().copied().collect::<BTreeMap<_, _>>();
    let max_inbound = input.inbound.iter().map(|(_, v)| *v).fold(0.0, f64::max);
    let max_mass = input
        .seeds
        .iter()
        .map(|(_, mass)| if mass.is_finite() { mass.max(0.0) } else { 0.0 })
        .fold(0.0, f64::max);
    let found = input
        .curves
        .iter()
        .map(|curve| contacts(curve, input, &vectors))
        .collect::<Vec<_>>();
    let mut count = BTreeMap::<i64, usize>::new();
    for curve in &found {
        for contact in curve {
            *count.entry(contact.seed).or_default() += 1;
        }
    }
    input
        .curves
        .iter()
        .zip(found)
        .map(|(curve, found)| {
            let mut none = 1.0;
            let mut closure = 0.0;
            let mut exact = 0;
            for c in &found {
                let specificity = if max_inbound > 0.0 {
                    input.config.specificity_floor.max(
                        1.0 - unit(inbound.get(&c.tag).copied().unwrap_or(0.0) / max_inbound)
                            .sqrt(),
                    )
                } else {
                    1.0
                };
                let rarity = input
                    .config
                    .rarity_floor
                    .max(1.0 - count[&c.seed] as f64 / input.curves.len().max(1) as f64);
                let mass = if max_mass > 0.0 {
                    unit(c.mass / max_mass)
                } else {
                    0.0
                };
                let matching = if c.exact {
                    exact += 1;
                    1.0
                } else {
                    input.config.semantic_anchor_discount
                };
                none *= 1.0 - unit(mass * specificity * c.closure * rarity * matching);
                closure += c.closure;
            }
            let mean = if found.is_empty() {
                0.0
            } else {
                closure / found.len() as f64
            };
            let score = unit(1.0 - none);
            let mut reliability = unit(
                (mean
                    * (found.len() as f64 / input.config.reliability_seed_saturation.max(1.0))
                        .min(1.0))
                .sqrt(),
            );
            if input.fallback {
                reliability = reliability.min(input.config.fallback_reliability_cap);
            }
            ReferenceAnchor {
                id: curve.id,
                score,
                reliability,
                strength: unit(score * reliability),
                contacted_seeds: found.len(),
                exact_contacts: exact,
                semantic_contacts: found.len().saturating_sub(exact),
                mean_closure: mean,
            }
        })
        .collect()
}
