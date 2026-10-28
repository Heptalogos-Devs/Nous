//! One coordinated EPA/Pyramid/sensing/fusion/field execution.
use super::*;

pub fn run_pipeline(
    connection: &MemoAssets,
    index: &dyn TagSearch,
    input: PipelineInput,
    cancelled: crate::CancellationCheck<'_>,
) -> Result<PipelineOutput, String> {
    crate::check_cancelled(cancelled)?;
    crate::assets::validate_finite_config(&input.config)?;
    let total_started = Instant::now();
    let dimensions = connection.dimension;
    let artifact = connection.graph.clone();
    let artifact_sig = &connection.signature;
    if input.query_vector.len() != dimensions
        || input.query_vector.iter().any(|v| !v.is_finite())
        || input
            .ghost_tags
            .iter()
            .any(|g| g.vector.len() != dimensions || g.vector.iter().any(|v| !v.is_finite()))
    {
        return Err("Memo pipeline vector dimensions or values are invalid".into());
    }
    let epa_started = Instant::now();
    if ![
        "absolute",
        "relativeLinear",
        "relativeSqrt",
        "backgroundRelative",
    ]
    .contains(&input.config.epa_gate_mode.as_str())
    {
        return Err("Invalid epaGateMode".into());
    }
    let epa = analyze_epa_with_mode(
        connection,
        &input.query_vector,
        dimensions,
        &input.config.epa_gate_mode,
    );
    let epa_ms = epa_started.elapsed().as_secs_f64() * 1000.0;

    let mut tag_vectors = TagVectorPool::new();
    let pyramid_started = Instant::now();
    let (pyramid, level_tags, residuals) = analyze_pyramid_with_pool(
        index,
        connection,
        &input.query_vector,
        dimensions,
        &input.config,
        &mut tag_vectors,
        cancelled,
    )?;
    let pyramid_ms = pyramid_started.elapsed().as_secs_f64() * 1000.0;

    let gating_started = Instant::now();
    let (gated_tags, effective_tag_boost, dynamic_core_boost) =
        gate_tags(&level_tags, &epa, &pyramid, &input.core_tags, &input.config);
    // 空 Pyramid 门控不再中止：Spike 生成合法空观测，随后字符串 Core/幽灵
    // 仍按 JS SOTA 在传播后注入；若最终仍为空，融合层返回原查询向量。
    let seeds: Vec<SenseSeed> = gated_tags
        .iter()
        .map(|tag| SenseSeed {
            id: tag.id,
            energy: tag.weight,
            source_type: if tag.is_core {
                "core".to_string()
            } else {
                "seed".to_string()
            },
        })
        .collect();
    let gating_ms = gating_started.elapsed().as_secs_f64() * 1000.0;

    let sensing_started = Instant::now();
    let mut seed_levels = HashMap::new();
    for (level, tags) in level_tags.iter().enumerate() {
        for tag in tags {
            seed_levels.entry(tag.id).or_insert(level);
        }
    }
    let mut wormholes = DynamicWormholeContext::try_new(
        &connection.tag_vectors,
        &mut tag_vectors,
        residuals,
        seed_levels,
    )?;
    for (layer, tags) in level_tags.iter().enumerate() {
        for tag in tags {
            wormholes.reuse_score(layer, tag.id, tag.similarity);
        }
    }
    let observation = sense_prevalidated(
        &artifact,
        artifact_sig,
        SenseInput {
            query_id: input.query_id.clone(),
            seeds,
            config: input.config.spike_routing.clone(),
        },
        Some(&mut wormholes),
        cancelled,
    )?;
    let sensing_diagnostics = observation.diagnostics.clone();
    drop(wormholes);
    let sensing_ms = sensing_started.elapsed().as_secs_f64() * 1000.0;

    let fusion_started = Instant::now();
    let (enhanced_vector, fusion, selected_tags) = fuse_observation_with_pool(
        connection,
        &input.query_vector,
        &observation,
        &gated_tags,
        &input.core_tags,
        &input.ghost_tags,
        dynamic_core_boost,
        dimensions,
        effective_tag_boost,
        &input.config,
        &mut tag_vectors,
    )?;
    let fusion_ms = fusion_started.elapsed().as_secs_f64() * 1000.0;
    let dual_fields = solve_dual_fields_with_pool(
        &artifact,
        connection,
        &observation.source_field,
        dimensions,
        &input.config,
        &mut tag_vectors,
        &observation
            .nodes
            .iter()
            .map(|node| node.id)
            .collect::<Vec<_>>(),
        &crate::projection::seed_basins(&observation, &artifact),
        cancelled,
    )?;

    let enhanced_vector_f32: Vec<f32> = enhanced_vector.iter().map(|value| *value as f32).collect();
    let observation_cached = false;
    let maximum_weight = selected_tags
        .iter()
        .map(|tag| tag.weight)
        .fold(0.0, f64::max);
    let matched_tags = selected_tags
        .iter()
        .filter(|tag| {
            tag.is_core
                || tag.weight
                    > maximum_weight
                        * if technical_name(&tag.name, true) {
                            input.config.tech_tag_threshold
                        } else {
                            input.config.normal_tag_threshold
                        }
        })
        .map(|tag| tag.name.clone())
        .collect::<Vec<_>>();
    let core_tags_matched = selected_tags
        .iter()
        .filter(|tag| tag.is_core)
        .map(|tag| tag.name.clone())
        .collect::<Vec<_>>();
    let seed_nodes = gated_tags.len();

    let dual_field_diagnostics = dual_fields.diagnostics;
    let metadata_json = serde_json::to_string(&PipelineMetadataOutput {
        schema: PIPELINE_SCHEMA.to_string(),
        algorithm_version: PIPELINE_ALGORITHM.to_string(),
        artifact_sig: artifact_sig.to_string(),
        query_id: input.query_id.clone(),
        epa,
        pyramid,
        matched_tags,
        core_tags_matched,
        effective_tag_boost,
        diagnostics: PipelineDiagnostics {
            backend: "rust-unified-memo-pipeline".to_string(),
            runtime_ownership: "typed-query-output".to_string(),
            epa_ms,
            pyramid_ms,
            gating_ms,
            sensing_ms,
            fusion_ms,
            dual_field_solve_ms: dual_field_diagnostics.solve_ms,
            dual_field_project_ms: dual_field_diagnostics.project_ms,
            dual_field_domain_ms: dual_field_diagnostics.domain_ms,
            total_ms: total_started.elapsed().as_secs_f64() * 1000.0,
            seed_nodes,
            observation_cached,
            observation_cache_error: None,
            dual_field: dual_field_diagnostics,
            fusion,
            sensing: sensing_diagnostics,
        },
    })
    .map_err(|error| format!("encode unified Memo pipeline metadata failed: {}", error))?;

    Ok(PipelineOutput {
        generation: connection.generation.clone(),
        artifact_sig: connection.signature.clone(),
        original_query_vector: input.query_vector,
        enhanced_vector: enhanced_vector_f32,
        local_vector: dual_fields.local_vector,
        transfer_vector: dual_fields.transfer_vector,
        local_field: dual_fields.local_field,
        transfer_field: dual_fields.transfer_field,
        local_domain_ids: dual_fields.local_domain_ids,
        transfer_domain_ids: dual_fields.transfer_domain_ids,
        observation,
        tag_vectors,
        metadata: serde_json::from_str(&metadata_json).map_err(|error| error.to_string())?,
    })
}
