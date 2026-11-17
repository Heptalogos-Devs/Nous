//! Candidate-pool calibration and stable readout request types.
use super::*;

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadoutRequest {
    #[serde(default = "default_top_k")]
    pub top_k: usize,
    #[serde(default)]
    pub include_trace: bool,
    pub pool: PreparedPool,
    pub allowed_file_ids: Option<Vec<i64>>,
    #[serde(default)]
    pub config: NativeConfig,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct PoolCalibration {
    pub id: i64,
    pub union_score: f64,
    pub union_rank: usize,
    pub sources: Vec<String>,
    pub anchor_score: f64,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct PreparedPool {
    pub candidates: Vec<CandidateInput>,
    pub bm25_reference: f64,
    pub selection: Vec<PoolCalibration>,
}
/// Calibrate over the complete authorized recall pool once, before selecting its union.
pub fn prepare_pool(
    connection: &MemoAssets,
    cached: &PipelineOutput,
    candidates: Vec<CandidateInput>,
    config: &NativeConfig,
    fixed: bool,
) -> Result<PreparedPool, String> {
    cached.validate_for(connection)?;
    config.validate()?;
    let curve_load = load_curves(connection, &candidates, connection.dimension)?;
    let mut curves = curve_load.curves;
    let local_domain: HashSet<i64> = cached.local_domain_ids.iter().copied().collect();
    compute_anchor_scores(&mut curves, |curve| {
        curve_load
            .relations
            .get(&curve.file_id)
            .into_iter()
            .flatten()
            .filter(|t| local_domain.contains(&t.0) && connection.tag_vectors.contains_key(&t.0))
            .count()
    });
    let bm25_reference = if config.bm25_k > 0 {
        curves.iter().map(|c| c.bm25_score).fold(0.0, f64::max)
    } else {
        0.0
    };
    for curve in &mut curves {
        curve.query_score = cosine(&cached.original_query_vector, &curve.chunk_vector);
        curve.denoised_score = cosine(&cached.enhanced_vector, &curve.chunk_vector);
        curve.local_score = cosine(&cached.local_vector, &curve.chunk_vector);
        curve.transfer_score = cosine(&cached.transfer_vector, &curve.chunk_vector);
    }
    let all = if fixed { Some(curves.clone()) } else { None };
    let mut selected = select_superset(curves, config, !fixed);
    if let Some(all) = all {
        let mut selected_ids: HashSet<i64> = selected.iter().map(|curve| curve.id).collect();
        for mut curve in all {
            if selected_ids.insert(curve.id) {
                curve.union_rank = selected.len() + 1;
                selected.push(curve);
            }
        }
    }
    let selection = selected
        .iter()
        .map(|c| PoolCalibration {
            id: c.id,
            union_score: c.union_score,
            union_rank: c.union_rank,
            sources: c.sources.clone(),
            anchor_score: c.anchor_score,
        })
        .collect::<Vec<_>>();
    let by_id = candidates
        .into_iter()
        .map(|c| (c.id, c))
        .collect::<HashMap<_, _>>();
    Ok(PreparedPool {
        candidates: selected.iter().map(|c| by_id[&c.id].clone()).collect(),
        bm25_reference,
        selection,
    })
}
