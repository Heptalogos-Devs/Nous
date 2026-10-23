//! Observable support and direct independent anchors.
// Adapted from VCPToolBox d1b40c863273cbbba082fb7657671424768c4a53.
// CC BY-NC-SA 4.0; see LICENSE-VCP and UPSTREAM.md.
use super::*;

pub(super) fn evaluate_observables(
    curve: &Curve,
    geometry: &GeometryOutput,
    input: &NativeInput,
    workspace: &FieldWorkspace,
    visible: bool,
    cosines: &CosineCache,
) -> ObservableOutput {
    let chain_size = curve.tags.len().max(1) as f64;
    let mut exact_seed_hits = 0usize;
    let mut local_contacts = 0usize;
    let mut transfer_contacts = 0usize;
    let mut local_potential = 0.0;
    let mut transfer_potential = 0.0;
    let mut tail_only = 0usize;
    let mut boundary: f64 = 0.0;

    for tag in &curve.tags {
        let local = workspace.local.get(&tag.id).copied().unwrap_or(0.0);
        let transfer = workspace.transfer.get(&tag.id).copied().unwrap_or(0.0);
        exact_seed_hits += usize::from(workspace.source_ids.contains(&tag.id));
        local_contacts += usize::from(workspace.local_domain.contains(&tag.id));
        transfer_contacts += usize::from(workspace.transfer_domain.contains(&tag.id));
        local_potential += local;
        transfer_potential += transfer;
        tail_only += usize::from(
            (local > 0.0 || transfer > 0.0)
                && !workspace.local_domain.contains(&tag.id)
                && !workspace.transfer_domain.contains(&tag.id),
        );
        boundary = boundary
            .max((clamp01(cosines.query_tag(&input.query.vector, tag)) * tag.chunk_cosine).sqrt());
    }

    let local_coverage = local_contacts as f64 / chain_size;
    let transfer_coverage = transfer_contacts as f64 / chain_size;
    let local_mean = local_potential / chain_size;
    let transfer_mean = transfer_potential / chain_size;
    let tail_ratio = tail_only as f64 / chain_size;
    let dual_agreement = 1.0 - (clamp01(local_mean) - clamp01(transfer_mean)).abs();
    let thematic = clamp01(
        0.25 * local_coverage
            + 0.2 * transfer_coverage
            + 0.2 * clamp01(local_mean)
            + 0.15 * clamp01(transfer_mean)
            + 0.2 * dual_agreement,
    ) * (1.0 - 0.5 * clamp01(tail_ratio));
    let query_chunk = clamp01(cosine(&input.query.vector, &curve.chunk_vector));
    let direct_contact = clamp01(exact_seed_hits as f64 / workspace.source_ids.len().max(1) as f64);
    let semantic_boundary_score = if boundary >= 0.55 { boundary } else { 0.0 };
    let direct = if visible {
        clamp01((0.75 * direct_contact).max(semantic_boundary_score))
    } else {
        0.0
    };
    let closure = clamp01(0.65 * query_chunk + 0.35 * geometry.tag_closure);
    ObservableOutput {
        direct,
        structural: geometry.path_quality,
        thematic,
        closure,
        query_chunk_score: query_chunk,
        semantic_boundary_score,
        local_coverage,
        transfer_coverage,
        local_potential: local_mean,
        transfer_potential: transfer_mean,
        tail_only_ratio: tail_ratio,
    }
}

pub(super) fn anchor_contacts(
    curve: &Curve,
    seeds: &[(i64, f64)],
    seed_vectors: &TagVectorPool,
    config: &NativeConfig,
    cosines: &CosineCache,
) -> Vec<(i64, i64, bool, f64, f64, f64)> {
    let mut contacts = Vec::new();
    for (seed_id, mass) in seeds {
        if let Some(tag) = curve.tags.iter().find(|tag| tag.id == *seed_id) {
            contacts.push((*seed_id, tag.id, true, 1.0, *mass, tag.chunk_cosine));
            continue;
        }
        let Some(seed_vector) = seed_vectors.get(seed_id) else {
            continue;
        };
        if let Some((tag, similarity)) = curve
            .tags
            .iter()
            .map(|tag| (tag, cosines.tag_pair(*seed_id, seed_vector, tag)))
            .filter(|item| item.1 >= config.semantic_anchor_threshold)
            .max_by(|left, right| left.1.partial_cmp(&right.1).unwrap_or(Ordering::Equal))
        {
            contacts.push((*seed_id, tag.id, false, similarity, *mass, tag.chunk_cosine));
        }
    }
    contacts
}

pub(super) fn compute_anchors(
    curves: &[Curve],
    seeds: &[(i64, f64)],
    seed_vectors: &TagVectorPool,
    artifact: &NativeArtifact,
    config: &NativeConfig,
    fallback: bool,
    cosines: &CosineCache,
) -> Vec<AnchorOutput> {
    let contacts: Vec<Vec<(i64, i64, bool, f64, f64, f64)>> = curves
        .par_iter()
        .map(|curve| anchor_contacts(curve, seeds, seed_vectors, config, cosines))
        .collect();
    let mut pool_counts: HashMap<i64, usize> = seeds.iter().map(|seed| (seed.0, 0)).collect();
    for candidate_contacts in &contacts {
        for contact in candidate_contacts {
            *pool_counts.entry(contact.0).or_default() += 1;
        }
    }
    let max_seed_mass = seeds
        .iter()
        .map(|seed| positive(seed.1))
        .fold(0.0, f64::max);
    contacts
        .into_par_iter()
        .map(|candidate_contacts| {
            let mut no_contact_probability = 1.0;
            let mut closure_sum = 0.0;
            let mut exact = 0usize;
            for contact in &candidate_contacts {
                let inbound = artifact.inbound.get(&contact.1).copied().unwrap_or(0.0);
                let specificity = if artifact.max_inbound > 0.0 {
                    config
                        .specificity_floor
                        .max(1.0 - clamp01(inbound / artifact.max_inbound).sqrt())
                } else {
                    1.0
                };
                let rarity = config.rarity_floor.max(
                    1.0 - pool_counts.get(&contact.0).copied().unwrap_or(0) as f64
                        / curves.len().max(1) as f64,
                );
                let normalized_mass = if max_seed_mass > 0.0 {
                    clamp01(contact.4 / max_seed_mass)
                } else {
                    0.0
                };
                let match_weight = if contact.2 {
                    exact += 1;
                    1.0
                } else {
                    config.semantic_anchor_discount
                };
                let contribution =
                    clamp01(normalized_mass * specificity * contact.5 * rarity * match_weight);
                no_contact_probability *= 1.0 - contribution;
                closure_sum += contact.5;
            }
            let contacted = candidate_contacts.len();
            let mean_closure = if contacted > 0 {
                closure_sum / contacted as f64
            } else {
                0.0
            };
            let score = clamp01(1.0 - no_contact_probability);
            let mut reliability = clamp01(
                (mean_closure
                    * (contacted as f64 / config.reliability_seed_saturation.max(1.0)).min(1.0))
                .sqrt(),
            );
            if fallback {
                reliability = reliability.min(config.fallback_reliability_cap);
            }
            AnchorOutput {
                score,
                reliability,
                strength: clamp01(score * reliability),
                contacted_seeds: contacted,
                exact_contacts: exact,
                semantic_contacts: contacted.saturating_sub(exact),
                mean_closure,
            }
        })
        .collect()
}
