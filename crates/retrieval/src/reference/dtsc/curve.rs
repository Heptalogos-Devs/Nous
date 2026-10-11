// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::field::dtsc_cosine;
use crate::reference::*;
use crate::reference::{positive, unit};
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
    pub field: &'a ReferenceDtscField,
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
                field: reference_dtsc_sample(
                    tag.id,
                    &tag.vector,
                    ctx.field,
                    ctx.inbound,
                    &ctx.config.field,
                ),
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
fn contacts(
    samples: &[Sample<'_>],
    ctx: &DtscCurveContext<'_>,
    score: &mut ReferenceDtscScore,
) -> f64 {
    let mut contacted_mass = 0.0;
    let mut potential_mass = 0.0;
    let mut closure_mass = 0.0;
    let mut semantic_mass = 0.0;
    for s in samples {
        closure_mass += s.closure * s.mass;
        if s.field.potential >= ctx.config.curve.weak_contact_threshold {
            score.geo_hit_count += 1;
            contacted_mass += s.mass;
            potential_mass += s.mass * s.field.potential;
        }
        score.geo_strong_hits +=
            usize::from(s.field.potential >= ctx.config.curve.strong_contact_threshold);
        if s.field.exact {
            score.geo_exact_hits += 1;
            if direct(s) {
                score.geo_direct_exact_hits += 1;
            } else {
                score.geo_emergent_exact_hits += 1;
            }
        } else if direct(s) && s.field.potential >= ctx.config.reward.direct_semantic_min_potential
        {
            score.geo_direct_semantic_hits += 1;
            semantic_mass += s.field.potential;
        }
        score.geo_max_potential = score.geo_max_potential.max(s.field.potential);
    }
    let total: f64 = samples.iter().map(|s| s.mass).sum();
    score.geo_weighted_coverage = unit(contacted_mass / total);
    score.geo_mean_potential = unit(potential_mass / contacted_mass.max(1e-12));
    score.geo_closure_quality = unit(closure_mass / total);
    let r = &ctx.config.reward;
    if score.geo_direct_semantic_hits >= r.direct_semantic_min_contacts {
        score.geo_direct_semantic_strength = unit(
            (semantic_mass / score.geo_direct_semantic_hits as f64
                - r.direct_semantic_min_potential)
                / (r.direct_semantic_saturation - r.direct_semantic_min_potential),
        );
    }
    potential_mass
}
fn edge(ctx: &DtscCurveContext<'_>, a: i64, b: i64) -> f64 {
    ctx.edges.get(&(a, b)).copied().unwrap_or(0.0)
}
fn sparse(
    samples: &[Sample<'_>],
    isolated: &[usize],
    isolated_mass: f64,
    ctx: &DtscCurveContext<'_>,
) -> (usize, f64) {
    let cfg = &ctx.config.sparse;
    if !cfg.enabled || isolated.len() < cfg.min_contacts {
        return (0, 0.0);
    }
    let mut connected = BTreeSet::new();
    let mut pairs = 0;
    let mut quality_sum = 0.0;
    for (offset, &a) in isolated.iter().enumerate() {
        let left = &samples[a];
        if left.field.potential < cfg.min_potential || left.closure < cfg.min_closure {
            continue;
        }
        for &b in isolated.iter().skip(offset + 1) {
            let right = &samples[b];
            if a.abs_diff(b) <= 1
                || right.field.potential < cfg.min_potential
                || right.closure < cfg.min_closure
            {
                continue;
            }
            let conductance =
                edge(ctx, left.tag.id, right.tag.id).max(edge(ctx, right.tag.id, left.tag.id));
            let similarity = dtsc_cosine(&left.tag.vector, &right.tag.vector);
            if conductance < cfg.min_conductance || similarity < cfg.min_similarity {
                continue;
            }
            let tq =
                unit((conductance - cfg.min_conductance) / (1.0 - cfg.min_conductance).max(1e-6));
            let sq = unit((similarity - cfg.min_similarity) / (1.0 - cfg.min_similarity).max(1e-6));
            let quality = positive(
                tq * sq
                    * (left.field.potential * right.field.potential).sqrt()
                    * (left.closure * right.closure).sqrt(),
            )
            .powf(0.25);
            if quality > 0.0 {
                pairs += 1;
                quality_sum += quality;
                connected.insert(a);
                connected.insert(b);
            }
        }
    }
    let connected_mass: f64 = connected
        .into_iter()
        .map(|i| samples[i].mass * samples[i].field.potential)
        .sum();
    let confidence = if pairs > 0 && isolated_mass > 0.0 {
        unit(connected_mass / isolated_mass)
            * unit(pairs as f64 / cfg.pair_saturation as f64)
            * unit(quality_sum / pairs as f64)
    } else {
        0.0
    };
    (pairs, confidence)
}
fn path(
    samples: &[Sample<'_>],
    weighted_potential: f64,
    ctx: &DtscCurveContext<'_>,
    score: &mut ReferenceDtscScore,
) {
    let weak = ctx.config.curve.weak_contact_threshold;
    let isolated = samples
        .iter()
        .enumerate()
        .filter(|(i, s)| {
            s.field.potential >= weak
                && (*i == 0 || samples[*i - 1].field.potential < weak)
                && (*i + 1 == samples.len() || samples[*i + 1].field.potential < weak)
        })
        .map(|(i, _)| i)
        .collect::<Vec<_>>();
    let isolated_mass: f64 = isolated
        .iter()
        .map(|i| samples[*i].mass * samples[*i].field.potential)
        .sum();
    let mut sums = [0.0; 6];
    for pair in samples.windows(2) {
        let a = &pair[0];
        let b = &pair[1];
        let arc = dtsc_cosine(&a.tag.vector, &b.tag.vector)
            .clamp(-1.0, 1.0)
            .acos()
            / std::f64::consts::PI;
        let forward = edge(ctx, a.tag.id, b.tag.id);
        let reverse = edge(ctx, b.tag.id, a.tag.id);
        let topology = unit(forward.max(reverse).sqrt());
        let potential = (a.field.potential * b.field.potential).sqrt();
        let mass = (a.mass * b.mass).sqrt();
        sums[0] += mass;
        sums[1] += mass * potential * (0.65 + 0.35 * topology);
        sums[2] += arc * mass;
        sums[3] += arc * mass / (potential + 0.25 * topology).max(0.08);
        if forward + reverse > 0.0 {
            let w = mass * potential.max(0.05);
            sums[4] += w;
            sums[5] += w * forward / (forward + reverse);
        }
    }
    score.geo_continuity = if sums[0] > 0.0 {
        unit(sums[1] / sums[0])
    } else {
        unit(score.geo_max_potential * 0.35)
    };
    score.geo_raw_isolated_ratio = unit(isolated_mass / weighted_potential.max(1e-12));
    let (pairs, confidence) = sparse(samples, &isolated, isolated_mass, ctx);
    score.geo_sparse_association_pairs = pairs;
    score.geo_sparse_association_confidence = confidence;
    score.geo_isolated_ratio =
        unit(score.geo_raw_isolated_ratio * (1.0 - ctx.config.sparse.max_relief * confidence));
    score.geo_action_quality = if sums[2] > 0.0 {
        unit((-sums[3] / sums[2].max(0.15)).exp())
    } else {
        unit(score.geo_max_potential * 0.5)
    };
    score.geo_direction_consistency = if sums[4] > 0.0 {
        unit(sums[5] / sums[4])
    } else {
        0.0
    };
}
fn shadow(
    curve: &ReferenceCurve,
    target: usize,
    ctx: &DtscCurveContext<'_>,
    score: &mut ReferenceDtscScore,
) {
    let a = query_closure(ctx.original, &curve.chunk_vector);
    let b = query_closure(ctx.enhanced, &curve.chunk_vector);
    let lift = b - a;
    score.geo_vector_lift = lift;
    score.geo_closure_score = unit(
        0.25 * a + 0.35 * b + 0.30 * score.geo_closure_quality + 0.10 * unit(0.5 + lift * 5.0),
    );
    let saturation = unit(
        (score.geo_direct_exact_hits + score.geo_direct_semantic_hits) as f64
            / (ctx.config.reward.direct_semantic_min_contacts + 1) as f64,
    );
    score.geo_direct_score = unit(
        0.45 * score
            .geo_direct_semantic_strength
            .max(if score.geo_direct_exact_hits > 0 {
                1.0
            } else {
                0.0
            })
            + 0.25 * score.geo_max_potential
            + 0.20 * score.geo_mean_potential
            + 0.1 * saturation,
    );
    let contact = unit(
        (score.geo_emergent_exact_hits + score.geo_strong_hits.min(target)) as f64
            / (target + 1) as f64,
    );
    score.geo_structural_score = unit(
        0.25 * score.geo_continuity
            + 0.2 * score.geo_action_quality
            + 0.15 * score.geo_closure_quality
            + 0.15 * (1.0 - score.geo_isolated_ratio)
            + 0.15 * score.geo_direction_consistency
            + 0.1 * contact,
    );
    score.geo_thematic_score = unit(
        0.35 * score.geo_weighted_coverage
            + 0.3 * score.geo_mean_potential
            + 0.2 * (1.0 - score.geo_isolated_ratio)
            + 0.15 * score.geo_closure_quality,
    );
    let e = &ctx.geometry.epa;
    let p = &ctx.geometry.pyramid;
    let weights = [
        0.45 + 0.35 * unit(e.logic_depth) + 0.2 * unit(p.coverage),
        0.35 + 0.3 * unit(p.depth) + 0.25 * unit(p.novelty) + 0.1 * unit(e.resonance),
        0.2 + 0.35 * unit(e.entropy) + 0.3 * unit(e.resonance) + 0.15 * (1.0 - unit(e.logic_depth)),
    ];
    let total = weights.iter().sum::<f64>().max(1e-12);
    score.geo_fused_shadow_score = unit(
        (1.0 - (1.0 - weights[0] / total * score.geo_direct_score)
            * (1.0 - weights[1] / total * score.geo_structural_score)
            * (1.0 - weights[2] / total * score.geo_thematic_score))
            * score.geo_closure_score,
    );
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
