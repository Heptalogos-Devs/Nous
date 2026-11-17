//! Curve path geometry and query river topology.
use super::*;

pub(super) fn evaluate_path(
    curve: &Curve,
    workspace: &FieldWorkspace,
    artifact: &NativeArtifact,
    config: &NativeConfig,
) -> GeometryOutput {
    let mut output = GeometryOutput::default();
    let mut quality_mass = 0.0;
    let mut used_relations = HashSet::new();
    for start in 0..curve.tags.len().saturating_sub(1) {
        let mut best = GeometryOutput::default();
        let mut best_quality = 0.0;
        let mut best_relation = None;
        for span in 1..=2 {
            let Some(next) = curve.tags.get(start + span) else {
                continue;
            };
            let current = &curve.tags[start];
            if current.position <= 0 || next.position <= 0 {
                continue;
            }
            let relation = (current.id.min(next.id), current.id.max(next.id));
            if current.id == next.id || used_relations.contains(&relation) {
                continue;
            }
            let local_potential = (workspace.local.get(&current.id).copied().unwrap_or(0.0)
                * workspace.local.get(&next.id).copied().unwrap_or(0.0))
            .sqrt();
            let transfer_potential = (workspace.transfer.get(&current.id).copied().unwrap_or(0.0)
                * workspace.transfer.get(&next.id).copied().unwrap_or(0.0))
            .sqrt();
            let forward = artifact.edge_weight(current.id, next.id);
            let reverse = artifact.edge_weight(next.id, current.id);
            let direction = if forward + reverse > 0.0 {
                clamp01(forward / (forward + reverse))
            } else {
                clamp01(config.direction_floor)
            };
            let semantic_continuity = clamp01((cosine(&current.vector, &next.vector) + 1.0) / 2.0);
            let field_continuity = (local_potential.max(transfer_potential)
                * workspace
                    .local
                    .get(&next.id)
                    .copied()
                    .unwrap_or(0.0)
                    .max(workspace.transfer.get(&next.id).copied().unwrap_or(0.0)))
            .sqrt();
            let continuity = clamp01(0.5 * semantic_continuity + 0.5 * field_continuity);
            let local_supported = workspace.local_domain.contains(&current.id)
                && workspace.local_domain.contains(&next.id);
            let transfer_supported = workspace.transfer_domain.contains(&current.id)
                && workspace.transfer_domain.contains(&next.id);
            let supported =
                (local_supported || transfer_supported) && (forward > 0.0 || reverse > 0.0);
            let weight_total = (config.local_weight + config.transfer_weight).max(1e-12);
            let potential = (config.local_weight * local_potential
                + config.transfer_weight * transfer_potential)
                / weight_total;
            let active_forward = workspace
                .active_edges
                .get(&(current.id, next.id))
                .copied()
                .unwrap_or(0.0);
            let active_reverse = workspace
                .active_edges
                .get(&(next.id, current.id))
                .copied()
                .unwrap_or(0.0);
            let active = active_forward.max(active_reverse);
            // Global-only support remains a weak fallback. Active flow is evidence,
            // not an independent additive reward for the same relation.
            let activity_gain = 0.5 + 0.5 * active.sqrt();
            let span_gain = if span == 2 { 0.7 } else { 1.0 };
            let quality = if supported {
                clamp01(
                    potential
                        * (if active > 0.0 {
                            clamp01(active_forward / (active_forward + active_reverse).max(1e-12))
                        } else {
                            direction
                        })
                        .max(config.direction_floor)
                        .sqrt()
                        * continuity.max(0.0).sqrt()
                        * activity_gain
                        * span_gain,
                )
            } else {
                0.0
            };
            if quality > best_quality {
                best_quality = quality;
                best_relation = Some(relation);
                best.supported_segments = usize::from(supported);
                best.active_segments = usize::from(active > 0.0);
                best.skip_segments = usize::from(span == 2);
                best.transfer_segments = usize::from(
                    supported
                        && transfer_supported
                        && (!local_supported || transfer_potential > local_potential),
                );
                best.mean_direction = direction;
                best.mean_continuity = continuity;
                best.mean_local_potential = local_potential;
                best.mean_transfer_potential = transfer_potential;
            }
        }
        if let Some(relation) = best_relation {
            used_relations.insert(relation);
        }
        output.segment_count += 1;
        output.supported_segments += best.supported_segments;
        output.active_segments += best.active_segments;
        output.skip_segments += best.skip_segments;
        output.transfer_segments += best.transfer_segments;
        output.mean_direction += best.mean_direction;
        output.mean_continuity += best.mean_continuity;
        output.mean_local_potential += best.mean_local_potential;
        output.mean_transfer_potential += best.mean_transfer_potential;
        quality_mass += best_quality;
    }

    if output.segment_count > 0 {
        let count = output.segment_count as f64;
        output.path_core = clamp01(quality_mass / count);
        output.mean_direction /= count;
        output.mean_continuity /= count;
        output.mean_local_potential /= count;
        output.mean_transfer_potential /= count;
        output.support_coverage = output.supported_segments as f64 / count;
    } else if let Some(tag) = curve.tags.first() {
        output.path_core = clamp01(
            workspace
                .local
                .get(&tag.id)
                .copied()
                .unwrap_or(0.0)
                .max(workspace.transfer.get(&tag.id).copied().unwrap_or(0.0))
                * 0.5,
        );
    }

    output.tag_closure = if curve.tags.is_empty() {
        0.0
    } else {
        curve
            .tags
            .iter()
            .map(|tag| {
                clamp01(
                    (tag.chunk_cosine - config.closure_floor)
                        / (1.0 - config.closure_floor).max(1e-9),
                )
            })
            .sum::<f64>()
            / curve.tags.len() as f64
    };
    output.path_quality = clamp01(
        output.path_core * (0.5 + 0.25 * output.support_coverage + 0.25 * output.tag_closure),
    );
    output
}

pub(super) fn evaluate_topology(
    curve: &Curve,
    input: &NativeInput,
    artifact: &NativeArtifact,
    query_tag_vectors: &TagVectorPool,
    cosines: &CosineCache,
) -> TopologyOutput {
    let mut output = TopologyOutput {
        query_nodes: input.query_state.river_nodes.len(),
        ..TopologyOutput::default()
    };
    // Ordered by Tag ID so that `mean_closure` does not sum in hash order.
    let mut alignments = BTreeMap::new();
    let mut total_node_weight = 0.0;
    let mut matched_node_weight = 0.0;
    let mut node_quality_mass = 0.0;

    let mut bindings = Vec::new();
    let mut node_weights = BTreeMap::new();
    for node in &input.query_state.river_nodes {
        let weight = positive(if node.normalized_energy != 0.0 {
            node.normalized_energy
        } else {
            node.energy
        })
        .max(1e-9);
        if node_weights.insert(node.id, weight).is_some() {
            continue;
        }
        total_node_weight += weight;
        for (index, tag) in curve.tags.iter().enumerate() {
            let exact = node.id == tag.id;
            let similarity = if exact {
                1.0
            } else {
                query_tag_vectors
                    .get(&node.id)
                    .map(|v| clamp01(cosines.tag_pair(node.id, v, tag)))
                    .unwrap_or(0.0)
            };
            if !exact && similarity <= input.config.semantic_node_threshold {
                continue;
            }
            let normalized = if exact {
                1.0
            } else {
                clamp01(
                    (similarity - input.config.semantic_node_threshold)
                        / (1.0 - input.config.semantic_node_threshold).max(1e-9),
                )
            };
            let weak_gain = if !exact && similarity < 0.65 {
                (similarity / 0.65).powi(2)
            } else {
                1.0
            };
            let quality = (normalized * tag.chunk_cosine).sqrt() * weak_gain;
            bindings.push((
                exact,
                node.id,
                tag.id,
                weight,
                Alignment {
                    candidate_index: index,
                    candidate_position: tag.position,
                    quality,
                    closure: tag.chunk_cosine,
                },
            ));
        }
    }
    bindings.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| (b.3 * b.4.quality).total_cmp(&(a.3 * a.4.quality)))
            .then_with(|| a.1.cmp(&b.1))
            .then_with(|| a.2.cmp(&b.2))
            .then_with(|| a.4.candidate_index.cmp(&b.4.candidate_index))
    });
    let mut used_tags = HashSet::new();
    for (exact, node_id, tag_id, weight, alignment) in bindings {
        if alignments.contains_key(&node_id) || used_tags.contains(&tag_id) {
            continue;
        }
        used_tags.insert(tag_id);
        output.exact_matched_nodes += usize::from(exact);
        output.semantic_matched_nodes += usize::from(!exact);
        output.strongest_node_match_quality =
            output.strongest_node_match_quality.max(alignment.quality);
        matched_node_weight += weight * alignment.quality;
        node_quality_mass += weight * alignment.quality * alignment.quality;
        alignments.insert(node_id, alignment);
    }
    let density = support_density(
        alignments.values().map(|a| a.quality).sum(),
        curve.tags.len(),
    );
    output.candidate_support_density = density;
    matched_node_weight *= density;
    node_quality_mass *= density;

    output.matched_nodes = alignments.len();
    output.matched_node_coverage = if total_node_weight > 0.0 {
        clamp01(matched_node_weight / total_node_weight)
    } else {
        0.0
    };
    output.node_alignment_score = if matched_node_weight > 0.0 {
        clamp01(node_quality_mass / matched_node_weight)
    } else {
        0.0
    };
    output.mean_closure = if alignments.is_empty() {
        0.0
    } else {
        alignments.values().map(|item| item.closure).sum::<f64>() / alignments.len() as f64
    };

    let maximum_hop = input
        .query_state
        .river_nodes
        .iter()
        .map(|node| node.hop.max(0))
        .max()
        .unwrap_or(1)
        .max(1) as f64;
    let river_node_by_id: HashMap<i64, &RiverNode> = input
        .query_state
        .river_nodes
        .iter()
        .map(|node| (node.id, node))
        .collect();
    let minimum_position = curve.tags.first().map(|tag| tag.position).unwrap_or(0);
    let maximum_position = curve
        .tags
        .last()
        .map(|tag| tag.position)
        .unwrap_or(minimum_position);
    let candidate_span = (maximum_position - minimum_position).max(1) as f64;
    let retained_edges: Vec<&RiverEdge> = input
        .query_state
        .river_edges
        .iter()
        .filter(|edge| {
            clamp01(if edge.normalized_flow != 0.0 {
                edge.normalized_flow
            } else {
                edge.flow
            }) >= input.config.minimum_river_edge_flow
        })
        .take(input.config.maximum_river_edges.max(1))
        .collect();
    output.query_edges = retained_edges.len();

    let mut total_edge_weight = 0.0;
    let mut matched_edge_weight = 0.0;
    let mut distance_mass = 0.0;
    let mut direction_mass = 0.0;
    let mut topology_mass = 0.0;

    for edge in retained_edges {
        let edge_weight = positive(if edge.normalized_flow != 0.0 {
            edge.normalized_flow
        } else {
            edge.flow
        })
        .max(1e-9);
        total_edge_weight += edge_weight;
        let (Some(source), Some(target)) = (
            alignments.get(&edge.source_id),
            alignments.get(&edge.target_id),
        ) else {
            continue;
        };
        if source.candidate_index == target.candidate_index {
            continue;
        }
        if source.candidate_position <= 0 || target.candidate_position <= 0 {
            continue;
        }
        let source_hop = river_node_by_id
            .get(&edge.source_id)
            .map(|node| node.hop)
            .unwrap_or(0);
        let target_hop = river_node_by_id
            .get(&edge.target_id)
            .map(|node| node.hop)
            .unwrap_or(0);
        let query_distance = (target_hop - source_hop).abs().max(1) as f64 / maximum_hop;
        let candidate_distance =
            (target.candidate_position - source.candidate_position).abs() as f64 / candidate_span;
        let distance_similarity = (-(query_distance - candidate_distance).abs()
            / input.config.relative_distance_temperature.max(1e-6))
        .exp();
        let direction_similarity = if target.candidate_position > source.candidate_position {
            1.0
        } else {
            clamp01(input.config.reverse_direction_credit)
        };
        let independent = artifact.independent_fraction_excluding(
            edge.source_id,
            edge.target_id,
            &curve.source_ids,
        );
        let endpoint = (source.quality * target.quality).sqrt();
        let edge_quality =
            clamp01(endpoint * distance_similarity * direction_similarity * independent);
        matched_edge_weight += edge_weight * endpoint;
        distance_mass += edge_weight * endpoint * distance_similarity;
        direction_mass += edge_weight * endpoint * direction_similarity;
        topology_mass += edge_weight * endpoint * edge_quality;
        output.matched_edges += 1;
    }

    output.matched_edge_coverage = if total_edge_weight > 0.0 {
        clamp01(matched_edge_weight / total_edge_weight)
    } else {
        0.0
    };
    if matched_edge_weight > 0.0 {
        output.relative_distance_score = clamp01(distance_mass / matched_edge_weight);
        output.direction_score = clamp01(direction_mass / matched_edge_weight);
        output.edge_topology_score = clamp01(topology_mass / matched_edge_weight);
    }
    output.motif_score = output.edge_topology_score;
    output.edge_graph_score = clamp01(
        0.18 * output.node_alignment_score
            + 0.22 * output.relative_distance_score
            + 0.18 * output.direction_score
            + 0.28 * output.edge_topology_score
            + 0.14 * output.motif_score,
    );
    output.node_graph_score = clamp01((output.node_alignment_score * output.mean_closure).sqrt());
    if output.matched_edges > 0 {
        output.score = output.edge_graph_score;
        output.reliability = clamp01(
            (output.matched_node_coverage * output.matched_edge_coverage * output.mean_closure)
                .cbrt(),
        );
        output.reliability_mode = "edge_topology".to_string();
    } else if output.matched_nodes > 0 {
        output.score = output.node_graph_score;
        output.reliability = input.config.node_only_reliability_cap.min(clamp01(
            (output.matched_node_coverage * output.mean_closure).sqrt(),
        ));
        output.reliability_mode = "node_alignment_fallback".to_string();
    } else {
        output.reliability_mode = "unavailable".to_string();
    }
    output
}

pub(super) fn support_density(supported: f64, total: usize) -> f64 {
    // Penalize unsupported opportunities, not richly supported annotation.
    let unsupported = positive(total as f64 - supported);
    1.0 / (1.0 + positive(unsupported - 6.0) / 16.0)
}

pub(super) fn branch_coverage(topology: &TopologyOutput, mode: &str) -> f64 {
    let nodes = clamp01(topology.matched_node_coverage / 0.35);
    let edges = clamp01(topology.matched_edge_coverage / 0.20);
    if mode == "atomic" {
        nodes
    } else if topology.matched_edges == 0 {
        0.35 * nodes
    } else {
        0.70 * edges + 0.30 * nodes
    }
}

pub(super) fn precision_grounding(topology: &TopologyOutput) -> f64 {
    if topology.exact_matched_nodes == 0 && topology.strongest_node_match_quality < 0.65 {
        0.65
    } else {
        1.0
    }
}

pub(super) fn branch_confidence(topology: &TopologyOutput, mode: &str) -> f64 {
    clamp01(
        branch_coverage(topology, mode)
            * clamp01(topology.mean_closure)
            * clamp01(topology.node_alignment_score),
    )
    .sqrt()
}

pub(super) fn field_resonance(curve: &Curve, input: &NativeInput, support: f64) -> (f64, f64, f64) {
    let local = if input.local_vector.iter().all(|v| v.is_finite())
        && squared_norm(&input.local_vector) > 1e-12
    {
        positive(input.config.pure_local_weight)
    } else {
        0.0
    };
    let transfer = if input.transfer_vector.iter().all(|v| v.is_finite())
        && squared_norm(&input.transfer_vector) > 1e-12
    {
        positive(input.config.pure_transfer_weight)
    } else {
        0.0
    };
    let total = local + transfer;
    let query = clamp01(curve.query_score);
    let field = if total > 1e-12 {
        clamp01(
            (local * clamp01(curve.local_score) + transfer * clamp01(curve.transfer_score)) / total,
        )
    } else {
        query
    };
    let lambda = clamp01(total / (positive(input.config.pure_query_weight) + total).max(1e-12));
    let gain = ((lambda * positive(field - query)).min(0.03) * clamp01(support)).min(1.0 - query);
    (query + gain, field, gain)
}

pub(super) fn scalar_field_support(coverage: f64, potential: f64) -> f64 {
    let potential = positive(potential);
    let covered = clamp01(coverage / 0.5);
    let saturated = clamp01(potential / 0.02);
    let domain = (covered * saturated).sqrt();
    let tail = 0.1 * (potential / (potential + 0.02)).sqrt();
    clamp01(domain.max(tail))
}
