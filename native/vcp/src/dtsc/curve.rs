mod path_scoring;

use super::field::{DtscFieldSampler, dtsc_cosine};
use super::*;
use super::{positive, unit};
use path_scoring::{contacts, path, shadow};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Deserialize)]
pub struct ReferenceDtscCandidate {
    pub curve: ReferenceCurve,
    pub score: f64,
}
#[derive(Debug, Clone, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ReferenceDtscEpaState {
    pub logic_depth: f64,
    pub entropy: f64,
    pub resonance: f64,
}
impl Default for ReferenceDtscEpaState {
    fn default() -> Self {
        Self {
            logic_depth: 0.5,
            entropy: 0.5,
            resonance: 0.0,
        }
    }
}
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ReferenceDtscPyramidState {
    pub coverage: f64,
    pub novelty: f64,
    pub depth: f64,
}
impl Default for ReferenceDtscPyramidState {
    fn default() -> Self {
        Self {
            coverage: 0.5,
            novelty: 0.5,
            depth: 0.5,
        }
    }
}
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct ReferenceDtscGeometryState {
    pub epa: ReferenceDtscEpaState,
    pub pyramid: ReferenceDtscPyramidState,
}
#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReferenceDtscScore {
    pub id: i64,
    pub score: f64,
    pub original_knn_score: f64,
    pub geo_score: f64,
    pub normalized_geo: f64,
    pub geo_bonus: f64,
    pub geo_base_bonus: f64,
    pub geo_aux_bonus: f64,
    pub geo_effect: String,
    pub geo_evidence_class: String,
    pub geo_reward_eligible: bool,
    pub geo_confidence: f64,
    pub geo_exact_hits: usize,
    pub geo_direct_exact_hits: usize,
    pub geo_emergent_exact_hits: usize,
    pub geo_direct_semantic_hits: usize,
    pub geo_direct_semantic_strength: f64,
    pub geo_strong_hits: usize,
    pub geo_hit_count: usize,
    pub geo_weighted_coverage: f64,
    pub geo_mean_potential: f64,
    pub geo_max_potential: f64,
    pub geo_continuity: f64,
    pub geo_isolated_ratio: f64,
    pub geo_raw_isolated_ratio: f64,
    pub geo_sparse_association_confidence: f64,
    pub geo_sparse_association_pairs: usize,
    pub geo_action_quality: f64,
    pub geo_closure_quality: f64,
    pub geo_direction_consistency: f64,
    pub geo_vector_lift: f64,
    pub geo_direct_score: f64,
    pub geo_structural_score: f64,
    pub geo_thematic_score: f64,
    pub geo_closure_score: f64,
    pub geo_fused_shadow_score: f64,
}
pub(crate) struct DtscCurveContext<'a> {
    pub config: &'a ReferenceDtscConfig,
    pub sampler: &'a DtscFieldSampler<'a>,
    pub inbound: &'a [(i64, f64)],
    pub edges: BTreeMap<(i64, i64), f64>,
    pub anchor: BTreeMap<i64, f64>,
    pub original: &'a [f32],
    pub enhanced: &'a [f32],
    pub geometry: &'a ReferenceDtscGeometryState,
}
fn query_closure(q: &[f32], c: &[f32]) -> f64 {
    if q.len() == c.len() && !c.is_empty() {
        unit((dtsc_cosine(q, c) + 1.0) / 2.0)
    } else {
        0.0
    }
}
fn empty(candidate: &ReferenceDtscCandidate, ctx: &DtscCurveContext<'_>) -> ReferenceDtscScore {
    let a = query_closure(ctx.original, &candidate.curve.chunk_vector);
    let b = query_closure(ctx.enhanced, &candidate.curve.chunk_vector);
    let lift = b - a;
    ReferenceDtscScore {
        id: candidate.curve.id,
        score: candidate.score,
        original_knn_score: candidate.score,
        geo_effect: "neutral".into(),
        geo_evidence_class: "neutral".into(),
        geo_isolated_ratio: 1.0,
        geo_raw_isolated_ratio: 1.0,
        geo_vector_lift: lift,
        geo_closure_score: unit(0.35 * a + 0.45 * b + 0.2 * unit(0.5 + lift * 5.0)),
        ..Default::default()
    }
}
struct Sample<'a> {
    tag: &'a ReferenceCurveTag,
    field: ReferenceDtscSample,
    mass: f64,
    closure: f64,
    specificity: f64,
}
fn samples<'a>(curve: &'a ReferenceCurve, ctx: &DtscCurveContext<'_>) -> Vec<Sample<'a>> {
    let cfg = &ctx.config.curve;
    let max_inbound = ctx.inbound.iter().map(|(_, w)| *w).fold(0.0, f64::max);
    curve
        .tags
        .iter()
        .map(|tag| {
            let closure = unit(
                (dtsc_cosine(&tag.vector, &curve.chunk_vector) - cfg.min_closure_similarity)
                    / (1.0 - cfg.min_closure_similarity).max(1e-6),
            );
            let specificity = if max_inbound <= 0.0 {
                1.0
            } else {
                ctx.config.field.public_hub_floor.clamp(0.05, 1.0).max(
                    1.0 - unit(
                        ctx.inbound
                            .iter()
                            .find(|(id, _)| *id == tag.id)
                            .map_or(0.0, |(_, w)| *w)
                            / max_inbound,
                    )
                    .sqrt(),
                )
            };
            let anchor = ctx
                .anchor
                .get(&tag.id)
                .copied()
                .unwrap_or(1.0)
                .clamp(0.5, 2.0);
            let positional = (-cfg.position_decay * positive(tag.position as f64 - 1.0)).exp();
            Sample {
                tag,
                field: ctx.sampler.sample(tag.id, &tag.vector),
                mass: (closure * anchor * positional * specificity).max(0.02),
                closure,
                specificity,
            }
        })
        .collect()
}
fn direct(s: &Sample<'_>) -> bool {
    matches!(s.field.source_type.as_str(), "seed" | "core")
}
fn reward(samples: &[Sample<'_>], ctx: &DtscCurveContext<'_>, score: &mut ReferenceDtscScore) {
    let r = &ctx.config.reward;
    let a = &ctx.config.auxiliary;
    let (class, eligible, cap, channel, floor_cap) = if score.geo_direct_exact_hits > 0
        || score.geo_direct_semantic_hits >= r.direct_semantic_min_contacts
    {
        (
            "direct",
            true,
            r.direct_bonus_cap,
            score.geo_direct_score,
            a.direct_floor_cap,
        )
    } else if score.geo_exact_hits > 0
        || (score.geo_strong_hits >= 2
            && score.geo_continuity >= r.structural_continuity_min
            && score.geo_isolated_ratio <= r.thematic_max_isolated_ratio)
    {
        (
            "structural",
            true,
            r.structural_bonus_cap,
            score.geo_structural_score,
            a.structural_floor_cap,
        )
    } else {
        (
            "thematic",
            score.geo_max_potential >= r.thematic_min_potential
                && score.geo_isolated_ratio <= r.thematic_max_isolated_ratio,
            r.thematic_bonus_cap,
            score.geo_thematic_score,
            a.thematic_floor_cap,
        )
    };
    score.geo_evidence_class = class.into();
    score.geo_reward_eligible = eligible;
    let qualified =
        class == "direct" && score.geo_direct_semantic_hits >= r.direct_semantic_min_contacts;
    let strength = if qualified {
        score.normalized_geo.max(score.geo_direct_semantic_strength)
    } else {
        score.normalized_geo
    };
    let confidence = if qualified {
        score.geo_confidence.max(r.direct_confidence_floor)
    } else {
        score.geo_confidence
    };
    score.geo_base_bonus = if eligible {
        (r.alpha * confidence * strength).min(cap.max(0.0))
    } else {
        0.0
    };
    let geometry_floor = if a.enabled
        && eligible
        && channel >= a.min_class_evidence
        && score.geo_fused_shadow_score >= a.min_fused_score
        && score.geo_closure_score >= a.min_closure_score
    {
        let values = [
            (score.geo_fused_shadow_score, a.min_fused_score),
            (channel, a.min_class_evidence),
            (score.geo_closure_score, a.min_closure_score),
        ];
        floor_cap
            * values
                .iter()
                .map(|(v, min)| unit((v - min) / (1.0 - min).max(1e-6)))
                .product::<f64>()
                .cbrt()
                .powf(a.floor_exponent)
    } else {
        0.0
    };
    let identity_floor = identity_floor(samples, ctx, eligible);
    let target = cap.min(geometry_floor.max(identity_floor));
    score.geo_aux_bonus = if a.enabled {
        a.max_aux_bonus.min(positive(target - score.geo_base_bonus))
    } else {
        0.0
    };
    score.geo_bonus = cap.min(score.geo_base_bonus + score.geo_aux_bonus);
    score.score = score.original_knn_score + score.geo_bonus;
    score.geo_effect = if score.geo_bonus > 0.0 {
        "boost"
    } else {
        "neutral"
    }
    .into();
}
fn identity_floor(samples: &[Sample<'_>], ctx: &DtscCurveContext<'_>, eligible: bool) -> f64 {
    let a = &ctx.config.auxiliary;
    let cfg = &a.identity_anchor;
    if !a.enabled || !cfg.enabled || !eligible {
        return 0.0;
    }
    let direct = samples
        .iter()
        .filter(|s| s.field.exact && direct(s))
        .collect::<Vec<_>>();
    let maximum = direct.iter().map(|s| s.field.potential).fold(0.0, f64::max);
    let strength = |s: &&Sample<'_>| unit(s.field.potential * s.specificity * s.closure.sqrt());
    let Some(best) = direct
        .into_iter()
        .max_by(|a, b| strength(a).total_cmp(&strength(b)))
    else {
        return 0.0;
    };
    let value = unit(best.field.potential * best.specificity * best.closure.sqrt());
    if maximum < cfg.min_potential
        || best.specificity < cfg.min_specificity
        || best.closure < cfg.min_tag_chunk_closure
        || value < cfg.min_strength
    {
        0.0
    } else {
        cfg.floor_cap
            * unit((value - cfg.min_strength) / (1.0 - cfg.min_strength).max(1e-6))
                .powf(cfg.floor_exponent)
    }
}
pub(crate) fn dtsc_curve(
    candidate: &ReferenceDtscCandidate,
    ctx: &DtscCurveContext<'_>,
) -> ReferenceDtscScore {
    let mut score = empty(candidate, ctx);
    let samples = samples(&candidate.curve, ctx);
    if samples.is_empty() {
        return score;
    }
    let weighted = contacts(&samples, ctx, &mut score);
    if score.geo_hit_count == 0 {
        return empty(candidate, ctx);
    }
    path(&samples, weighted, ctx, &mut score);
    let target = ctx
        .config
        .curve
        .min_geo_samples
        .min((samples.len() as f64).sqrt().ceil() as usize);
    let evidence =
        unit((score.geo_strong_hits as f64 + 0.75 * score.geo_exact_hits as f64) / target as f64);
    score.geo_confidence = unit(
        score.geo_weighted_coverage
            * (0.55 + 0.45 * evidence)
            * (1.0 - 0.65 * score.geo_isolated_ratio),
    );
    score.geo_score = score.geo_confidence
        * unit(
            0.3 * score.geo_mean_potential
                + 0.2 * score.geo_max_potential
                + 0.2 * score.geo_continuity
                + 0.15 * score.geo_action_quality
                + 0.15 * score.geo_closure_quality,
        );
    let r = &ctx.config.reward;
    score.normalized_geo = unit(
        (score.geo_score - r.geo_reward_floor) / (r.geo_reward_saturation - r.geo_reward_floor),
    );
    shadow(&candidate.curve, target, ctx, &mut score);
    reward(&samples, ctx, &mut score);
    score
}
