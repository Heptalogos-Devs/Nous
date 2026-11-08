//! Contact, path and shadow scoring for one DTSC candidate.
use super::*;

pub(super) fn contacts(
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
pub(super) fn path(
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
pub(super) fn shadow(
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
