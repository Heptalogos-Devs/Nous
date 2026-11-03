//! Complete coordinated candidate readout and result materialization.
use super::*;

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadoutRequest {
    #[serde(default = "default_top_k")]
    pub top_k: usize,
    #[serde(default)]
    pub include_trace: bool,
    pub candidates: Vec<CandidateInput>,
    pub allowed_file_ids: Option<Vec<i64>>,
    #[serde(default)]
    pub config: NativeConfig,
}
pub fn readout(
    connection: &MemoAssets,
    cached: &PipelineOutput,
    request: ReadoutRequest,
    cancelled: crate::CancellationCheck<'_>,
) -> Result<NativeOutput, String> {
    crate::check_cancelled(cancelled)?;
    cached.validate_for(connection)?;
    crate::assets::validate_finite_config(&request.config)?;
    if !(0.0..=1.0).contains(&request.config.lexical_weight) {
        return Err("MEMO_CONFIG_INVALID: lexicalWeight must lie in [0, 1]".into());
    }
    if request.candidates.iter().any(|candidate| {
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
        candidates: request.candidates,
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
    let local_domain: HashSet<i64> = input.query_state.local_domain_ids.iter().copied().collect();
    // Anchor hits count only Tags with a loadable vector, as when every candidate
    // Tag was loaded up front, so the local-domain Tags are checked now; the
    // other Tags are read only for the curves that survive selection.
    let file_tags = |curve: &Curve| {
        curve_load
            .relations
            .get(&curve.file_id)
            .into_iter()
            .flatten()
    };
    let domain_tag_ids: Vec<i64> = curves
        .iter()
        .flat_map(file_tags)
        .map(|relation| relation.0)
        .filter(|id| local_domain.contains(id))
        .collect();
    let domain_stats = ensure_tag_vectors_loaded(
        &connection.tag_vectors,
        &mut tag_vectors,
        &domain_tag_ids,
        dimension,
    )?;
    compute_anchor_scores(&mut curves, |curve| {
        file_tags(curve)
            .filter(|relation| {
                local_domain.contains(&relation.0) && tag_vectors.contains_key(&relation.0)
            })
            .count()
    });
    // Normalize within the authorized, hydrated pool before union truncation.
    // Max normalization retains a sole lexical hit, unlike min/max subtraction.
    let maximum_bm25 = if input.config.bm25_k > 0 {
        curves
            .iter()
            .map(|curve| curve.bm25_score)
            .fold(0.0, f64::max)
    } else {
        0.0
    };
    curves.par_iter_mut().for_each(|curve| {
        curve.lexical_score = if maximum_bm25 > 0.0 {
            clamp01(curve.bm25_score / maximum_bm25)
        } else {
            0.0
        };
        curve.query_score = cosine(&input.query.vector, &curve.chunk_vector);
        curve.denoised_score = cosine(&input.denoised_vector, &curve.chunk_vector);
        curve.local_score = cosine(&input.local_vector, &curve.chunk_vector);
        curve.transfer_score = cosine(&input.transfer_vector, &curve.chunk_vector);
    });
    let projected_count = curves.len();
    let mut curves = select_superset(curves, &input.config);
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

    let workspace = FieldWorkspace {
        local: normalize_field(&input.query_state.local_field),
        transfer: normalize_field(&input.query_state.transfer_field),
        local_domain,
        transfer_domain: input
            .query_state
            .transfer_domain_ids
            .iter()
            .copied()
            .collect(),
        source_ids: input
            .query_state
            .source_field
            .iter()
            .map(|entry| entry.0)
            .collect(),
        active_edges: input
            .query_state
            .river_edges
            .iter()
            .filter_map(|edge| {
                // Query support is conditioned on an observed, real transport edge.
                let flow = clamp01(if edge.normalized_flow != 0.0 {
                    edge.normalized_flow
                } else {
                    edge.flow
                });
                (flow >= input.config.minimum_river_edge_flow
                    && artifact.edge_weight(edge.source_id, edge.target_id) > 0.0)
                    .then_some(((edge.source_id, edge.target_id), flow))
            })
            .collect(),
    };
    let direct_ids: Vec<i64> = input
        .query_state
        .field_provenance
        .iter()
        .filter(|entry| {
            entry.hop == 0 && matches!(entry.source_type.as_str(), "core" | "lexical_seed" | "seed")
        })
        .map(|entry| entry.id)
        .collect();
    let fallback_anchor = direct_ids.is_empty();
    let anchor_ids: HashSet<i64> = if fallback_anchor {
        input
            .query_state
            .source_field
            .iter()
            .map(|entry| entry.0)
            .collect()
    } else {
        direct_ids.into_iter().collect()
    };
    let mut seeds: Vec<(i64, f64)> = input
        .query_state
        .source_field
        .iter()
        .filter(|entry| anchor_ids.contains(&entry.0) && entry.1 > 0.0)
        .copied()
        .collect();

    let maximum_seed = seeds.iter().map(|s| s.1).fold(0.0, f64::max);
    let core_ids: HashSet<i64> = input
        .query_state
        .field_provenance
        .iter()
        .filter(|p| p.hop == 0 && matches!(p.source_type.as_str(), "core" | "lexical_seed"))
        .map(|p| p.id)
        .collect();
    seeds.retain(|s| s.1 >= maximum_seed * 0.2 || core_ids.contains(&s.0));

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

    let results: Vec<NativeResultItem> = work
        .into_iter()
        .take(input.top_k.max(1))
        .enumerate()
        .map(|(index, item)| {
            // Same set as before, listed in curve order instead of hash order.
            let mut seen_names = HashSet::new();
            let matched_tags: Vec<String> = item
                .curve
                .tags
                .iter()
                .filter(|tag| seen_names.insert(tag.name.as_str()))
                .map(|tag| tag.name.clone())
                .collect();
            let core_tags_matched = matched_tags
                .iter()
                .filter(|name| core_tag_names.contains(&name.to_lowercase()))
                .cloned()
                .collect();
            let topology_v3 = if input.include_trace {
                Some(serde_json::json!({
                    "mode": "conservative_candidate_self_topology_v2.2",
                    "branchNodeCoverage": clamp01(item.topology.matched_node_coverage / 0.35),
                    "branchEdgeCoverage": clamp01(item.topology.matched_edge_coverage / 0.20),
                    "branchCoverage": branch_coverage(&item.topology, mode),
                    "branchConfidence": branch_confidence(&item.topology, mode),
                    "strongestNodeMatchQuality": item.topology.strongest_node_match_quality,
                    "precisionGrounding": precision_grounding(&item.topology),
                    "fieldSupport": item.observables.field_support,
                    "fieldResonanceBeforeSupport": field_resonance(&item.curve, &input, 1.0).2,
                    "queryScore": clamp01(item.curve.query_score),
                    "fieldScore": item.field_score,
                    "fieldResonanceBonus": item.field_resonance_bonus,
                    "directEvidence": item.direct_evidence,
                    "omega": omega.omega,
                    "regime": omega.regime,
                    "omegaEdge": omega.omega_edge,
                    "omegaEmerge": omega.omega_emerge,
                    "omegaFlow": omega.omega_flow,
                    "omegaGamma": input.config.omega_gamma,
                    "graphGate": omega.omega.powf(input.config.omega_gamma.max(0.0)),
                    "v2Bonus": item.v2_bonus,
                    "gatedV2Bonus": item.gated_bonus,
                    "anchorScore": item.anchor.score,
                    "anchorReliability": item.anchor.reliability,
                    "anchorStrength": item.anchor.strength,
                    "implicitOnlyAnchor": item.anchor.implicit_only,
                    "explicitExactContacts": item.anchor.explicit_exact_contacts,
                    "explicitSemanticContacts": item.anchor.explicit_semantic_contacts,
                    "hasExplicitFocus": item.anchor.has_explicit_focus,
                    "anchorBonus": item.anchor_bonus,
                    "contactedSeeds": item.anchor.contacted_seeds,
                    "exactContacts": item.anchor.exact_contacts,
                    "semanticContacts": item.anchor.semantic_contacts,
                    "meanClosure": item.anchor.mean_closure,
                    "role": item.role,
                    "pureScore": item.pure_score,
                    "pathBonus": item.path_bonus,
                    "graphBonus": item.gated_bonus,
                    "requestedGraphBonus": item.v2_bonus * omega.omega.powf(input.config.omega_gamma.max(0.0)),
                    "finalScore": item.final_score,
                    "nativeBackend": "rust-rayon"
                })).map(|mut trace| {
                    trace["wormholeRrf"] = wormhole_traces
                        .get(&item.curve.id).cloned().unwrap_or(Value::Null);
                    trace
                })
            } else {
                None
            };
            NativeResultItem {
                id: item.curve.id,
                chunk_id: item.curve.id,
                rank: index + 1,
                score: item.final_score,
                ranking_score: item.ranking_score,
                base_score: item.pure_score,
                field_score: item.field_score,
                field_resonance_bonus: item.field_resonance_bonus,
                topology_bonus: item.path_bonus + item.gated_bonus,
                anchor_bonus: item.anchor_bonus,
                path_bonus: item.path_bonus,
                graph_bonus: item.gated_bonus,
                score_clipping: positive(item.pure_score + item.path_bonus + item.gated_bonus
                    + item.anchor_bonus - wormhole_traces.get(&item.curve.id)
                        .and_then(|v| v["baselineScore"].as_f64()).unwrap_or(item.final_score)),
                role: item.role,
                omega: omega.omega,
                river_regime: omega.regime.clone(),
                matched_tags,
                core_tags_matched,
                candidate_sources: item.curve.sources,
                original_score: original_score_by_id
                    .get(&item.curve.id)
                    .copied()
                    .unwrap_or(0.0),
                topology_v3,
                relative_topology: input.include_trace.then_some(item.topology),
                geometry: input.include_trace.then_some(item.geometry),
                observables: input.include_trace.then_some(item.observables),
            }
        })
        .collect();
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
            candidate_tag_vector_source_batches: domain_stats.source_batches
                + selected_stats.source_batches,
            candidate_tag_vector_fetched_nodes: domain_stats.fetched_nodes
                + selected_stats.fetched_nodes,
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
