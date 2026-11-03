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
    for pair in curve.tags.windows(2) {
        let current = &pair[0];
        let next = &pair[1];
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
        let supported = (local_supported || transfer_supported) && (forward > 0.0 || reverse > 0.0);
        let weight_total = (config.local_weight + config.transfer_weight).max(1e-12);
        let potential = (config.local_weight * local_potential
            + config.transfer_weight * transfer_potential)
            / weight_total;
        let quality = if supported {
            clamp01(
                potential
                    * direction.max(config.direction_floor).sqrt()
                    * continuity.max(0.0).sqrt(),
            )
        } else {
            0.0
        };
        output.segment_count += 1;
        output.supported_segments += usize::from(supported);
        output.transfer_segments += usize::from(
            supported
                && transfer_supported
                && (!local_supported || transfer_potential > local_potential),
        );
        output.mean_direction += direction;
        output.mean_continuity += continuity;
        output.mean_local_potential += local_potential;
        output.mean_transfer_potential += transfer_potential;
        quality_mass += quality;
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
    let mut alignments = HashMap::new();
    let mut total_node_weight = 0.0;
    let mut matched_node_weight = 0.0;
    let mut node_quality_mass = 0.0;

    for node in &input.query_state.river_nodes {
        let weight = positive(if node.normalized_energy != 0.0 {
            node.normalized_energy
        } else {
            node.energy
        })
        .max(1e-9);
        total_node_weight += weight;
        let exact = curve
            .tags
            .iter()
            .enumerate()
            .find(|item| item.1.id == node.id);
        let alignment = if let Some((index, tag)) = exact {
            Some(Alignment {
                candidate_index: index,
                candidate_position: tag.position,
                quality: tag.chunk_cosine.sqrt(),
                closure: tag.chunk_cosine,
            })
        } else {
            let query_vector = query_tag_vectors.get(&node.id);
            curve
                .tags
                .iter()
                .enumerate()
                .filter_map(|(index, tag)| {
                    // 查询河网与候选曲线的原始 Tag 向量均已加载；直接计算精确
                    // 余弦，避免在 NativeArtifact 中再常驻一份字符串键 Pairwise 表。
                    let similarity = query_vector
                        .map(|vector| clamp01(cosines.tag_pair(node.id, vector, tag)))
                        .unwrap_or(0.0);
                    if similarity < input.config.semantic_node_threshold {
                        return None;
                    }
                    let normalized = clamp01(
                        (similarity - input.config.semantic_node_threshold)
                            / (1.0 - input.config.semantic_node_threshold).max(1e-9),
                    );
                    Some(Alignment {
                        candidate_index: index,
                        candidate_position: tag.position,
                        quality: (normalized * tag.chunk_cosine).sqrt(),
                        closure: tag.chunk_cosine,
                    })
                })
                .max_by(|left, right| {
                    left.quality
                        .partial_cmp(&right.quality)
                        .unwrap_or(Ordering::Equal)
                        .then_with(|| right.candidate_index.cmp(&left.candidate_index))
                })
        };
        if let Some(alignment) = alignment {
            matched_node_weight += weight;
            node_quality_mass += weight * alignment.quality;
            alignments.insert(node.id, alignment);
        }
    }

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
        matched_edge_weight += edge_weight;
        distance_mass += edge_weight * distance_similarity;
        direction_mass += edge_weight * direction_similarity;
        topology_mass += edge_weight * edge_quality;
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
