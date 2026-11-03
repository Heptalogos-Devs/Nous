//! Observation fusion into the enhanced query vector.
use super::*;

#[derive(Clone)]
pub(super) struct FusionTag {
    pub(super) id: i64,
    pub(super) name: String,
    pub(super) weight: f64,
    pub(super) is_core: bool,
    pub(super) vector: Arc<[f32]>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct FusionDiagnostics {
    pub(super) requested_count: usize,
    pub(super) found_count: usize,
    pub(super) deduplicated_count: usize,
    pub(super) total_weight: f64,
    pub(super) selected_tag_ids: Vec<i64>,
    pub(super) emergent_count: usize,
    pub(super) supplemented_core_count: usize,
    pub(super) ghost_count: usize,
}

pub(super) fn fuse_observation_with_pool(
    connection: &MemoAssets,
    query: &[f32],
    observation: &SenseOutput,
    gated_tags: &[GatedTag],
    core_tag_ids: &[i64],
    ghost_tags: &[GhostTagInput],
    dynamic_core_boost: f64,
    dimension: usize,
    alpha: f64,
    config: &PipelineConfig,
    tag_vectors: &mut TagVectorPool,
) -> std::result::Result<(Vec<f64>, FusionDiagnostics, Vec<FusionTag>), String> {
    // 与 JS SOTA 完全相同：传播后的原始种子取 max，防止循环共现膨胀；
    // 纯涌现节点按能量排序后只保留 Top-N。
    let gated_by_id: HashMap<i64, &GatedTag> = gated_tags.iter().map(|tag| (tag.id, tag)).collect();
    let mut merged = Vec::new();
    let mut emergent = Vec::new();
    for node in &observation.nodes {
        if node.id <= 0 || node.energy <= 0.0 {
            continue;
        }
        if let Some(seed) = gated_by_id.get(&node.id) {
            merged.push(GatedTag {
                id: seed.id,
                name: seed.name.clone(),
                weight: seed.weight.max(node.energy),
                is_core: seed.is_core,
            });
        } else {
            emergent.push(GatedTag {
                id: node.id,
                name: String::new(),
                weight: node.energy,
                is_core: false,
            });
        }
    }
    emergent.sort_by(|left, right| {
        right
            .weight
            .partial_cmp(&left.weight)
            .unwrap_or(Ordering::Equal)
            .then_with(|| left.id.cmp(&right.id))
    });
    if config.max_emergent_nodes > 0 {
        emergent.truncate(config.max_emergent_nodes);
    }
    let emergent_count = emergent.len();
    merged.extend(emergent);

    // JS 使用传播后 allTags 的最大基准权重；coreBoostFactor 仅用于还原基准，
    // 实际补全 Core 与 Hard Ghost 使用动态 Core 增益。
    let core_divisor =
        if config.core_boost_factor.is_finite() && config.core_boost_factor.abs() > 1e-12 {
            config.core_boost_factor
        } else {
            1.33
        };
    let max_base_weight = merged
        .iter()
        .map(|tag| tag.weight / core_divisor)
        .fold(0.0, f64::max)
        .max(if merged.is_empty() { 1.0 } else { 0.0 });

    // A disconnected canonical core still participates in fusion. Identity is
    // resolved before the numerical kernel; equal labels never merge nodes.
    let mut supplemented_core_count = 0usize;
    for id in core_tag_ids {
        if merged.iter().any(|tag| tag.id == *id) {
            continue;
        }
        if let Some(name) = connection.tag_names.get(id) {
            merged.push(GatedTag {
                id: *id,
                name: name.clone(),
                weight: max_base_weight * dynamic_core_boost,
                is_core: true,
            });
            supplemented_core_count += 1;
        }
    }

    let requested_count = merged.len() + ghost_tags.len();
    let ids = merged.iter().map(|tag| tag.id).collect::<Vec<_>>();
    ensure_tag_vectors_loaded(&connection.tag_vectors, tag_vectors, &ids, dimension)?;
    let mut vectors = Vec::with_capacity(requested_count);
    for tag in merged {
        let Some(vector) = tag_vectors.get(&tag.id).cloned() else {
            continue;
        };
        if magnitude(&vector) > 1e-12 {
            vectors.push(FusionTag {
                id: tag.id,
                name: if tag.name.is_empty() {
                    format!("Tag#{}", tag.id)
                } else {
                    tag.name
                },
                weight: tag.weight,
                is_core: tag.is_core,
                vector,
            });
        }
    }

    // 幽灵节点使用专属负 ID，仅参与去重与向量融合，绝不进入 Artifact/Spike。
    let mut ghost_id = -1i64;
    let mut accepted_ghosts = 0usize;
    for ghost in ghost_tags {
        if ghost.vector.len() != dimension || magnitude(&ghost.vector) <= 1e-12 {
            continue;
        }
        vectors.push(FusionTag {
            id: ghost_id,
            name: ghost.name.clone(),
            weight: max_base_weight
                * if ghost.is_core {
                    dynamic_core_boost
                } else {
                    1.0
                },
            is_core: ghost.is_core,
            vector: Arc::from(ghost.vector.clone()),
        });
        ghost_id -= 1;
        accepted_ghosts += 1;
    }
    let found_count = vectors.len();

    // JS SOTA：按权重降序贪心去重；冗余标签把 20% 能量转移给代表标签，
    // 同时保留 Core 属性。不能简化为“直接丢弃冗余标签”。
    vectors.sort_by(|left, right| {
        right
            .weight
            .partial_cmp(&left.weight)
            .unwrap_or(Ordering::Equal)
            .then_with(|| left.id.cmp(&right.id))
    });
    vectors.truncate(config.max_fusion_tags.clamp(1, 1024));
    let threshold = config.deduplication_threshold.clamp(-1.0, 1.0);
    let mut selected: Vec<FusionTag> = Vec::new();
    for candidate in vectors {
        let redundant_index = selected
            .iter()
            .position(|existing| cosine(&candidate.vector, &existing.vector) > threshold);
        if let Some(index) = redundant_index {
            selected[index].weight += candidate.weight * 0.2;
            selected[index].is_core |= candidate.is_core;
        } else {
            selected.push(candidate);
        }
    }

    let total_weight = selected.iter().map(|entry| entry.weight).sum::<f64>();
    if total_weight <= 0.0 {
        // JS applyTagBoost() 的兼容契约：无法构造 Tag 上下文时原样返回查询向量，
        // 而不是抛错。空 selected 也确保 matched/coreTagsMatched 保持为空。
        let diagnostics = FusionDiagnostics {
            requested_count,
            found_count,
            deduplicated_count: 0,
            total_weight: 0.0,
            selected_tag_ids: Vec::new(),
            emergent_count,
            supplemented_core_count,
            ghost_count: accepted_ghosts,
        };
        return Ok((
            query.iter().map(|value| *value as f64).collect(),
            diagnostics,
            Vec::new(),
        ));
    }
    let mut context = vec![0.0f64; dimension];
    for tag in &selected {
        for offset in 0..dimension {
            context[offset] += tag.vector[offset] as f64 * tag.weight;
        }
    }
    for value in &mut context {
        *value /= total_weight;
    }
    normalize(&mut context);

    let mut fused: Vec<f64> = query
        .iter()
        .zip(context.iter())
        .map(|(source, memo)| (1.0 - alpha) * (*source as f64) + alpha * *memo)
        .collect();
    normalize(&mut fused);
    let diagnostics = FusionDiagnostics {
        requested_count,
        found_count,
        deduplicated_count: selected.len(),
        total_weight,
        selected_tag_ids: selected.iter().map(|entry| entry.id).collect(),
        emergent_count,
        supplemented_core_count,
        ghost_count: accepted_ghosts,
    };
    Ok((fused, diagnostics, selected))
}
