//! Complete coordinated candidate readout and result materialization.
use super::*;
mod materialize;
mod pool;
mod query_context;

use materialize::{ResultContext, materialize_results};
pub use pool::{PoolCalibration, PreparedPool, ReadoutRequest, prepare_pool};
use query_context::{QueryContext, query_context};

pub fn readout(
    connection: &MemoAssets,
    cached: &PipelineOutput,
    request: ReadoutRequest,
    cancelled: crate::CancellationCheck<'_>,
) -> Result<NativeOutput, String> {
    crate::check_cancelled(cancelled)?;
    cached.validate_for(connection)?;
    request.config.validate()?;
    if request.pool.candidates.iter().any(|candidate| {
        candidate.id <= 0
            || [
                candidate.score,
                candidate.hybrid_score,
                candidate.vector_score,
                candidate.bm25_score,
                candidate.time_score,
                candidate.anchor_score,
            ]
            .iter()
            .any(|value| !value.is_finite())
    }) || request
        .allowed_file_ids
        .as_ref()
        .is_some_and(|ids| ids.iter().any(|id| *id <= 0))
    {
        return Err(
            "MEMO_READOUT_INPUT_INVALID: candidate scores and allowed file IDs must be valid"
                .into(),
        );
    }
    let total_started = Instant::now();
    let artifact_sig = &connection.signature;
    let mut tag_vectors = cached.tag_vectors.clone();
    let input = NativeInput {
        dimension: connection.dimension,
        top_k: request.top_k,
        include_trace: request.include_trace,
        query: QueryInput {
            _text: String::new(),
            vector: cached.original_query_vector.clone(),
        },
        denoised_vector: cached.enhanced_vector.clone(),
        local_vector: cached.local_vector.clone(),
        transfer_vector: cached.transfer_vector.clone(),
        candidates: request.pool.candidates,
        allowed_file_ids: request.allowed_file_ids,
        config: request.config,
        query_state: QueryStateInput {
            query_id: cached.observation.query_id.clone(),
            source_field: cached.observation.source_field.clone(),
            local_field: cached.local_field.clone(),
            transfer_field: cached.transfer_field.clone(),
            local_domain_ids: cached.local_domain_ids.clone(),
            transfer_domain_ids: cached.transfer_domain_ids.clone(),
            river_nodes: cached
                .observation
                .nodes
                .iter()
                .map(|node| RiverNode {
                    id: node.id,
                    energy: node.energy,
                    normalized_energy: node.normalized_energy,
                    hop: node.hop as i64,
                })
                .collect(),
            river_edges: cached
                .observation
                .edges
                .iter()
                .map(|edge| RiverEdge {
                    source_id: edge.source_id,
                    target_id: edge.target_id,
                    flow: edge.flow,
                    normalized_flow: edge.normalized_flow,
                    wormhole: edge.wormhole,
                    immediate_return: edge.immediate_return,
                })
                .collect(),
            field_provenance: cached
                .observation
                .nodes
                .iter()
                .map(|node| SourceProvenance {
                    id: node.id,
                    hop: node.hop as i64,
                    source_type: node.source_type.clone(),
                })
                .collect(),
            complete_observation: !cached.observation.source_field.is_empty(),
        },
    };

    let dimension = input.dimension;
    if input.query.vector.len() != dimension
        || input.denoised_vector.len() != dimension
        || input.local_vector.len() != dimension
        || input.transfer_vector.len() != dimension
    {
        return Err(format!(
            "RiverMemo native vector dimension mismatch: expected {}",
            dimension
        ));
    }

    let load_started = Instant::now();
    let artifact = connection.graph.clone();
    let curve_load = load_curves(connection, &input.candidates, dimension)?;
    let mut curves = curve_load.curves;
    // 显式空集合也意味着无权限；必须在候选归一化和统计前移除越权项。
    let allowed: Option<HashSet<i64>> = input
        .allowed_file_ids
        .as_ref()
        .map(|ids| ids.iter().copied().collect());
    if let Some(scope) = &allowed {
        curves.retain(|curve| scope.contains(&curve.file_id));
    }
    let original_score_by_id: HashMap<i64, f64> = input
        .candidates
        .iter()
        .map(|candidate| {
            (
                candidate.id,
                if candidate.score != 0.0 {
                    candidate.score
                } else if candidate.hybrid_score != 0.0 {
                    candidate.hybrid_score
                } else {
                    candidate.vector_score
                },
            )
        })
        .collect();
    let maximum_bm25 = request.pool.bm25_reference;
    let mut calibration_by_id = HashMap::new();
    for calibration in &request.pool.selection {
        calibration_by_id
            .entry(calibration.id)
            .or_insert(calibration);
    }
    for curve in &mut curves {
        let calibrated = calibration_by_id
            .get(&curve.id)
            .ok_or_else(|| "MEMO_POOL_CALIBRATION_MISSING".to_string())?;
        curve.union_score = calibrated.union_score;
        curve.union_rank = calibrated.union_rank;
        curve.sources = calibrated.sources.clone();
        curve.anchor_score = calibrated.anchor_score;
        curve.lexical_score = if maximum_bm25 > 0.0 {
            clamp01(curve.bm25_score / maximum_bm25)
        } else {
            0.0
        };
        curve.query_score = cosine(&input.query.vector, &curve.chunk_vector);
        curve.denoised_score = cosine(&input.denoised_vector, &curve.chunk_vector);
        curve.local_score = cosine(&input.local_vector, &curve.chunk_vector);
        curve.transfer_score = cosine(&input.transfer_vector, &curve.chunk_vector);
    }
    let projected_count = curves.len();
    let file_tags = |curve: &Curve| {
        curve_load
            .relations
            .get(&curve.file_id)
            .into_iter()
            .flatten()
    };
    let selected_tag_ids: Vec<i64> = curves
        .iter()
        .flat_map(file_tags)
        .map(|relation| relation.0)
        .collect();
    let selected_stats = ensure_tag_vectors_loaded(
        &connection.tag_vectors,
        &mut tag_vectors,
        &selected_tag_ids,
        dimension,
    )?;
    attach_tags(&mut curves, &curve_load.relations, &tag_vectors);
    let selected_count = curves.len();

    let QueryContext {
        field: workspace,
        seeds,
        core_ids,
        fallback_anchor,
    } = query_context(&input, &artifact);

    // Query River 节点和 Direct Anchor/Seed 共享一次分块 Tag 向量读取。
    // 同一 ID 同时属于河网和锚点时只读取、解码并存储一次。
    let mut requested_tag_ids: Vec<i64> = input
        .query_state
        .river_nodes
        .iter()
        .map(|node| node.id)
        .collect();
    requested_tag_ids.extend(seeds.iter().map(|seed| seed.0));
    requested_tag_ids.extend(
        input
            .query_state
            .river_edges
            .iter()
            .filter(|edge| edge.wormhole)
            .map(|edge| edge.target_id),
    );
    let query_stats = ensure_tag_vectors_loaded(
        &connection.tag_vectors,
        &mut tag_vectors,
        &requested_tag_ids,
        dimension,
    )?;
    let query_tag_source_batches = query_stats.source_batches;
    let query_and_seed_tag_vectors = tag_vectors;
    let mut wormhole_model = wormhole::build(&input, &query_and_seed_tag_vectors, &artifact);
    let mut wormhole_gate = wormhole::gate(&mut wormhole_model, connection)?;
    wormhole::refine(&mut wormhole_model, &mut wormhole_gate, connection, &input)?;
    let load_ms = load_started.elapsed().as_secs_f64() * 1000.0;

    crate::check_cancelled(cancelled)?;
    let compute_started = Instant::now();
    let morphology = compute_query_morphology(&input);
    let cosines = CosineCache::build(
        &curves,
        &input.query.vector,
        input
            .query_state
            .river_nodes
            .iter()
            .map(|node| node.id)
            .chain(seeds.iter().map(|seed| seed.0)),
        &query_and_seed_tag_vectors,
    );

    // Indexed parallel results preserve curve order; rewards do not depend on peers.
    let anchor_results = compute_anchors(
        &curves,
        &seeds,
        &query_and_seed_tag_vectors,
        &artifact,
        &input.config,
        fallback_anchor,
        &cosines,
        &core_ids,
    );
    // Expand only after pool selection. Each path retains the same body and membership anchors.
    let mut path_curves = Vec::new();
    let mut path_anchors = Vec::new();
    for (curve, anchor) in curves.into_iter().zip(anchor_results) {
        let Some(paths) = connection
            .candidate_paths
            .get(&curve.id)
            .filter(|p| !p.is_empty())
        else {
            path_curves.push(curve);
            path_anchors.push(anchor);
            continue;
        };
        for (index, steps) in paths.iter().enumerate() {
            let mut variant = curve.clone();
            variant.path_index = Some(index);
            variant.multi_path = paths.len() > 1;
            // Preserve ordered occurrences and append unrepresented membership contacts.
            let mut tags = Vec::new();
            let mut included = HashSet::new();
            for step in steps {
                if let Some(tag) = curve.tags.iter().find(|t| t.id == step.tag_id) {
                    let mut tag = tag.clone();
                    tag.position = step.position;
                    included.insert(tag.id);
                    tags.push(tag);
                }
            }
            for tag in &curve.tags {
                if !included.contains(&tag.id) {
                    let mut tag = tag.clone();
                    tag.position = 0;
                    tags.push(tag);
                }
            }
            variant.tags = tags;
            path_curves.push(variant);
            path_anchors.push(anchor.clone());
        }
    }
    let curves = path_curves;
    let anchor_results = path_anchors;
    let core_tag_names: HashSet<String> = input
        .query_state
        .field_provenance
        .iter()
        .filter(|entry| entry.source_type == "core")
        .filter_map(|entry| {
            curves
                .iter()
                .flat_map(|curve| curve.tags.iter())
                .find(|tag| tag.id == entry.id)
                .map(|tag| tag.name.to_lowercase())
        })
        .collect();

    let mut work: Vec<ScoredWork> = curves
        .into_par_iter()
        .zip(anchor_results.into_par_iter())
        .map(|(curve, anchor)| {
            let geometry = evaluate_path(&curve, &workspace, &artifact, &input.config);
            let topology = evaluate_topology(
                &curve,
                &input,
                &artifact,
                &query_and_seed_tag_vectors,
                &cosines,
            );
            let visible = true; // 越权候选已在进入候选池前删除。
            let observables =
                evaluate_observables(&curve, &geometry, &input, &workspace, visible, &cosines);
            let (semantic_base, field_score, field_resonance_bonus) =
                field_resonance(&curve, &input, observables.field_support);
            let topology_raw = clamp01(
                0.625 * geometry.path_quality
                    + 0.375
                        * clamp01(
                            0.35 * observables.local_coverage
                                + 0.25 * observables.transfer_coverage
                                + 0.25 * clamp01(observables.local_potential)
                                + 0.15 * clamp01(observables.transfer_potential),
                        ),
            );
            let path_reliability =
                clamp01(geometry.path_quality / input.config.topology_path_saturation.max(1e-6));
            let topology_reliability = (path_reliability * observables.query_chunk_score).sqrt();
            let topology_bonus =
                input.config.topology_bonus_cap * topology_raw * topology_reliability;
            let pure_score = semantic_base;
            let path_bonus = topology_bonus.min(input.config.topology_bonus_cap);
            let atomic_score = 0.75 * topology.node_graph_score + 0.25 * topology.edge_graph_score;
            let propositional_score =
                0.25 * topology.node_graph_score + 0.75 * topology.edge_graph_score;
            let narrative_score =
                0.15 * topology.node_graph_score + 0.85 * topology.edge_graph_score;
            let graph_score = clamp01(
                morphology.atomic_weight * atomic_score
                    + morphology.propositional_weight * propositional_score
                    + morphology.narrative_weight * narrative_score,
            );
            ScoredWork {
                curve,
                geometry,
                topology,
                direct_evidence: observables.semantic_boundary_score.max(observables.direct),
                observables,
                pure_score,
                semantic_score: semantic_base,
                lexical_base: semantic_base,
                field_score,
                field_resonance_bonus,
                graph_score,
                role: String::new(),
                anchor,
                v2_bonus: 0.0,
                gated_bonus: 0.0,
                path_bonus,
                anchor_bonus: 0.0,
                final_score: 0.0,
                ranking_score: 0.0,
            }
        })
        .collect();

    let omega = compute_omega(&input);
    let mode = morphology.dominant_mode.as_str();
    assign_v3_scores(&mut work, mode, &omega, &input.config);
    let mut path_diagnostics = HashMap::<i64, Vec<Value>>::new();
    let mut selected_paths = Vec::<ScoredWork>::new();
    let mut selected_by_id = HashMap::<i64, usize>::new();
    for item in work {
        path_diagnostics.entry(item.curve.id).or_default().push(serde_json::json!({"path":item.curve.path_index,"P":item.path_bonus,"G":item.gated_bonus,"B":item.pure_score,"A":item.anchor_bonus}));
        if let Some(&index) = selected_by_id.get(&item.curve.id) {
            let existing = &mut selected_paths[index];
            if item.path_bonus + item.gated_bonus > existing.path_bonus + existing.gated_bonus {
                let base = existing.pure_score;
                let anchor = existing.anchor_bonus;
                *existing = item;
                existing.pure_score = base;
                existing.anchor_bonus = anchor;
                existing.final_score =
                    clamp01(base + anchor + existing.path_bonus + existing.gated_bonus);
                existing.ranking_score = existing.final_score;
            }
        } else {
            selected_by_id.insert(item.curve.id, selected_paths.len());
            selected_paths.push(item);
        }
    }
    let mut work = selected_paths;
    work.sort_by(|left, right| {
        right
            .final_score
            .partial_cmp(&left.final_score)
            .unwrap_or(Ordering::Equal)
            .then_with(|| {
                right
                    .curve
                    .union_score
                    .partial_cmp(&left.curve.union_score)
                    .unwrap_or(Ordering::Equal)
            })
            .then_with(|| left.curve.union_rank.cmp(&right.curve.union_rank))
    });
    let wormhole_fusion = wormhole::fuse_rrf(&mut work, &wormhole_model, &input, &omega);
    let wormhole_rrf_active = wormhole_fusion.active;
    let wormhole_traces = wormhole_fusion.traces;
    // Stable sort retains the complete baseline comparator for equal fusion scores.
    work.sort_by(|a, b| b.ranking_score.total_cmp(&a.ranking_score));
    let ranked_count = work.len();

    let results = materialize_results(
        work,
        ResultContext {
            input: &input,
            core_tag_names: &core_tag_names,
            mode,
            omega: &omega,
            wormhole_traces: &wormhole_traces,
            original_score_by_id: &original_score_by_id,
            path_diagnostics,
        },
    );
    let compute_ms = compute_started.elapsed().as_secs_f64() * 1000.0;

    let output = NativeOutput {
        wormhole_rrf_active,
        schema: RESULT_SCHEMA.to_string(),
        algorithm_version: ALGORITHM_VERSION.to_string(),
        artifact_sig: artifact_sig.to_string(),
        query_id: input.query_state.query_id.clone(),
        omega,
        query_mode: mode.to_string(),
        query_morphology: morphology,
        diagnostics: NativeDiagnostics {
            backend: "rust-rayon-owned-assets".to_string(),
            offered_candidates: input.candidates.len(),
            projected_candidates: projected_count,
            selected_candidates: selected_count,
            ranked_candidates: ranked_count,
            returned_candidates: results.len(),
            rayon_threads: rayon::current_num_threads(),
            artifact_nodes: artifact.node_ids.len(),
            artifact_edges: artifact.targets.len(),
            load_ms,
            compute_ms,
            total_ms: total_started.elapsed().as_secs_f64() * 1000.0,
            chunk_source_batches: curve_load.chunk_source_batches,
            file_tag_source_batches: curve_load.file_tag_source_batches,
            query_tag_source_batches,
            candidate_tag_vector_source_batches: selected_stats.source_batches,
            candidate_tag_vector_fetched_nodes: selected_stats.fetched_nodes,
        },
        results,
        parameters: serde_json::json!({
            "parsedConfig": input.config,
            "wormholeGateMode": "global-two-joint-files-distinct-checksums-third-tag-explicit-focus",
            "wormholeEvidenceScope": "global-file-tag-graph; independent-of-candidate-and-retrieval-scope",
            "wormholeGate": wormhole_gate,
            "wormholeEligibleRoutes": wormhole_model.eligible_routes,
            "wormholeAcceptedRoutes": wormhole_model.routes.len(),
            "wormholeScoring": "strict-global-gate; explanation-weighted-RRF; untouched-baseline-channel",
            "wormholeRrfK": 60,
            "wormholeStrongFlowRatio": 0.5,
            "wormholeWeightReference": "maximum observed non-return ordinary edge flow; observation maximum if absent",
            "baselineAlgorithm": "rivermemo.conservative-topology-v2.2",
            "fieldResonanceCap": 0.03,
            "fieldSupportPotentialSaturation": 0.02,
            "fieldSupportCoverageSaturation": 0.5,
            "fieldSupportTailCap": 0.1,
            "implicitContactDiscount": 0.35,
            "implicitStrengthCap": 0.035,
            "weakSemanticPivot": 0.65,
            "span2Discount": 0.7,
            "globalOnlyPathGain": 0.5,
            "unsupportedTagAllowance": 6,
            "unsupportedTagScale": 16,
            "branchNodeSaturation": 0.35,
            "branchEdgeSaturation": 0.20,
            "branchEdgeWeight": 0.70,
            "branchNodeWeight": 0.30,
            "branchNodeOnlyWeight": 0.35,
            "softMatchQualityThreshold": 0.65,
            "softMatchDiscount": 0.65,
            "graphConfidence": "sqrt(branchCoverage*meanClosure*nodeAlignmentScore)",
            "graphReward": "candidate-self-no-role-multiplier",
            "retiredControls": ["conditionalBandwidth", "conditionalClosureBandwidth",
                "conditionalDirectBandwidth", "minimumPeers", "minimumEffectivePeers",
                "innovationConfidenceZ", "innovationScale", "anchorActivationZ",
                "anchorFrontierContrast", "anchorFrontierAbsFloor", "reliabilitySeedSaturation"]
        }),
    };
    Ok(output)
}
