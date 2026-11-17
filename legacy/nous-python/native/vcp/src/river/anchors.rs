//! Observable support and direct independent anchors.
use super::*;

pub(super) fn evaluate_observables(
    curve: &Curve,
    geometry: &GeometryOutput,
    input: &NativeInput,
    workspace: &FieldWorkspace,
    visible: bool,
    cosines: &CosineCache,
) -> ObservableOutput {
    let mut seen_ids = HashSet::new();
    let tags: Vec<&TagData> = curve
        .tags
        .iter()
        .filter(|t| seen_ids.insert(t.id))
        .collect();
    let chain_size = tags.len().max(1) as f64;
    let mut exact_seed_hits = 0usize;
    let mut local_contacts = 0usize;
    let mut transfer_contacts = 0usize;
    let mut local_potential = 0.0;
    let mut transfer_potential = 0.0;
    let mut tail_only = 0usize;
    let mut boundary: f64 = 0.0;

    for tag in &tags {
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
        field_support: scalar_field_support(local_coverage, local_mean)
            .max(scalar_field_support(transfer_coverage, transfer_mean)),
    }
}

pub(super) fn anchor_contacts(
    curve: &Curve,
    seeds: &[(i64, f64)],
    seed_vectors: &TagVectorPool,
    config: &NativeConfig,
    cosines: &CosineCache,
) -> Vec<(i64, i64, bool, f64, f64, f64)> {
    // Enumerate edges before assignment: exact contacts reserve both endpoints.
    let mut edges = Vec::new();
    for (seed_id, mass) in seeds {
        for tag in &curve.tags {
            let exact = tag.id == *seed_id;
            let similarity = if exact {
                1.0
            } else {
                seed_vectors
                    .get(seed_id)
                    .map(|vector| cosines.tag_pair(*seed_id, vector, tag))
                    .unwrap_or(0.0)
            };
            if exact || similarity >= config.semantic_anchor_threshold.max(0.88) {
                edges.push((*seed_id, tag.id, exact, similarity, *mass, tag.chunk_cosine));
            }
        }
    }
    edges.sort_by(|a, b| {
        b.2.cmp(&a.2)
            .then_with(|| (b.3 * b.4 * b.5).total_cmp(&(a.3 * a.4 * a.5)))
            .then_with(|| a.0.cmp(&b.0))
            .then_with(|| a.1.cmp(&b.1))
    });
    let mut used_seeds = HashSet::new();
    let mut used_tags = HashSet::new();
    let mut contacts = Vec::new();
    for edge in edges {
        if used_seeds.contains(&edge.0) || used_tags.contains(&edge.1) {
            continue;
        }
        used_seeds.insert(edge.0);
        used_tags.insert(edge.1);
        contacts.push(edge);
        if contacts.len() == 3 {
            break;
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
    explicit_ids: &HashSet<i64>,
) -> Vec<AnchorOutput> {
    let contacts: Vec<Vec<(i64, i64, bool, f64, f64, f64)>> = curves
        .par_iter()
        .map(|curve| anchor_contacts(curve, seeds, seed_vectors, config, cosines))
        .collect();
    let explicit_max = seeds
        .iter()
        .filter(|s| explicit_ids.contains(&s.0))
        .map(|s| positive(s.1))
        .fold(0.0, f64::max);
    let max_seed_mass = if explicit_max > 0.0 {
        explicit_max
    } else {
        seeds
            .iter()
            .map(|seed| positive(seed.1))
            .fold(0.0, f64::max)
    };
    contacts
        .into_par_iter()
        .map(|candidate_contacts| {
            let mut no_contact_probability = 1.0;
            let mut closure_sum = 0.0;
            let mut exact = 0usize;
            let implicit_only = !candidate_contacts.is_empty()
                && !candidate_contacts
                    .iter()
                    .any(|c| explicit_ids.contains(&c.0));
            for contact in &candidate_contacts {
                let inbound = artifact.inbound.get(&contact.1).copied().unwrap_or(0.0);
                let specificity = if artifact.max_inbound > 0.0 {
                    config
                        .specificity_floor
                        .max(1.0 - clamp01(inbound / artifact.max_inbound).sqrt())
                } else {
                    1.0
                };
                // Global specificity, not the accidental composition of this pool.
                let rarity = config.rarity_floor.max(specificity);
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
                        * clamp01(
                            (contact.3 - config.semantic_anchor_threshold.max(0.88))
                                / (1.0 - config.semantic_anchor_threshold.max(0.88)).max(1e-9),
                        )
                };
                let grounding_gain = if !explicit_ids.contains(&contact.0) {
                    0.35
                } else {
                    1.0
                };
                let contribution = clamp01(
                    normalized_mass
                        * specificity
                        * contact.5
                        * rarity
                        * match_weight
                        * grounding_gain,
                );
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
            // A single exact focus is sufficient identity evidence. Quantity is
            // never a substitute for closure; semantic-only contacts stay discounted.
            let mut reliability = clamp01(mean_closure).sqrt()
                * if exact > 0 {
                    1.0
                } else {
                    config.semantic_anchor_discount
                };
            if fallback {
                reliability = reliability.min(config.fallback_reliability_cap);
            }
            if implicit_only {
                reliability = reliability.min(0.35);
            }
            AnchorOutput {
                score,
                reliability,
                strength: if implicit_only {
                    clamp01(score * reliability).min(0.035)
                } else {
                    clamp01(score * reliability)
                },
                implicit_only,
                explicit_exact_contacts: candidate_contacts
                    .iter()
                    .filter(|c| c.2 && explicit_ids.contains(&c.0))
                    .count(),
                explicit_semantic_contacts: candidate_contacts
                    .iter()
                    .filter(|c| !c.2 && explicit_ids.contains(&c.0))
                    .count(),
                has_explicit_focus: explicit_max > 0.0,
                contacted_seeds: contacted,
                exact_contacts: exact,
                semantic_contacts: contacted.saturating_sub(exact),
                mean_closure,
            }
        })
        .collect()
}
