//! Candidate/tag hydration, selection and field normalization.
// Adapted from VCPToolBox d1b40c863273cbbba082fb7657671424768c4a53.
// CC BY-NC-SA 4.0; see LICENSE-VCP and UPSTREAM.md.
use super::*;

pub(super) type FileTagRows = HashMap<i64, Vec<(i64, i64, String)>>;

pub(super) fn attach_tags(
    curves: &mut [Curve],
    relations: &FileTagRows,
    tag_vectors: &TagVectorPool,
) {
    for curve in curves {
        let chunk_vector = curve.chunk_vector.clone();
        curve.tags = relations
            .get(&curve.file_id)
            .into_iter()
            .flatten()
            .filter_map(|(id, position, name)| {
                let vector = tag_vectors.get(id)?;
                Some(TagData {
                    id: *id,
                    position: *position,
                    name: name.clone(),
                    vector: vector.clone(),
                    chunk_cosine: clamp01(cosine(vector, &chunk_vector)),
                })
            })
            .collect();
    }
}

pub(super) fn compute_anchor_scores(curves: &mut [Curve], hits: impl Fn(&Curve) -> usize) {
    let max_hits = curves.iter().map(&hits).max().unwrap_or(0);
    if max_hits == 0 {
        return;
    }
    for curve in curves {
        let hits = hits(curve);
        curve.anchor_score = curve.anchor_score.max(hits as f64 / max_hits as f64);
    }
}

pub(super) fn source_top(
    curves: &[Curve],
    field: fn(&Curve) -> f64,
    limit: usize,
) -> Vec<(i64, f64, usize)> {
    let mut ranked: Vec<(i64, f64)> = curves
        .iter()
        .map(|curve| (curve.id, field(curve)))
        .filter(|item| item.1.is_finite() && item.1 > 0.0)
        .collect();
    ranked.sort_by(|left, right| {
        right
            .1
            .partial_cmp(&left.1)
            .unwrap_or(Ordering::Equal)
            .then_with(|| left.0.cmp(&right.0))
    });
    ranked
        .into_iter()
        .take(limit.max(1))
        .enumerate()
        .map(|(index, item)| (item.0, item.1, index + 1))
        .collect()
}

pub(super) fn select_superset(curves: Vec<Curve>, config: &NativeConfig) -> Vec<Curve> {
    let sources = vec![
        (
            "query_knn",
            source_top(&curves, |curve| curve.query_score, config.query_k),
        ),
        (
            "denoised_field_knn",
            source_top(&curves, |curve| curve.denoised_score, config.denoised_k),
        ),
        (
            "local_field_knn",
            source_top(&curves, |curve| curve.local_score, config.local_field_k),
        ),
        (
            "transfer_field_knn",
            source_top(
                &curves,
                |curve| curve.transfer_score,
                config.transfer_field_k,
            ),
        ),
        (
            "bm25",
            source_top(&curves, |curve| curve.bm25_score, config.bm25_k),
        ),
        (
            "time",
            source_top(&curves, |curve| curve.time_score, config.query_k),
        ),
        (
            "anchor_direct",
            source_top(&curves, |curve| curve.anchor_score, config.anchor_k),
        ),
    ];

    let mut source_map: HashMap<i64, Vec<(String, f64, usize)>> = HashMap::new();
    for (name, ranked) in sources {
        if ranked.is_empty() {
            continue;
        }
        let minimum = ranked
            .iter()
            .map(|entry| entry.1)
            .fold(f64::INFINITY, f64::min);
        let maximum = ranked
            .iter()
            .map(|entry| entry.1)
            .fold(f64::NEG_INFINITY, f64::max);
        let spread = maximum - minimum;
        for (id, raw_score, rank) in ranked {
            let normalized = if spread > 1e-12 {
                clamp01((raw_score - minimum) / spread)
            } else {
                clamp01(1.0 / rank as f64)
            };
            source_map
                .entry(id)
                .or_default()
                .push((name.to_string(), normalized, rank));
        }
    }

    let mut selected: Vec<Curve> = curves
        .into_iter()
        .filter_map(|mut curve| {
            let entries = source_map.get(&curve.id)?;
            let maximum = entries.iter().map(|entry| entry.1).fold(0.0, f64::max);
            let mean = entries.iter().map(|entry| entry.1).sum::<f64>() / entries.len() as f64;
            let reciprocal = entries
                .iter()
                .map(|entry| 1.0 / (60.0 + entry.2 as f64))
                .sum::<f64>();
            let multi_bonus = (0.05 * entries.len().saturating_sub(1) as f64).min(0.2);
            curve.union_score = clamp01(
                0.5 * maximum + 0.25 * mean + 0.25 * clamp01(reciprocal * 20.0) + multi_bonus,
            );
            curve.sources = entries.iter().map(|entry| entry.0.clone()).collect();
            Some(curve)
        })
        .collect();
    selected.sort_by(|left, right| {
        right
            .sources
            .len()
            .cmp(&left.sources.len())
            .then_with(|| {
                right
                    .union_score
                    .partial_cmp(&left.union_score)
                    .unwrap_or(Ordering::Equal)
            })
            .then_with(|| left.id.cmp(&right.id))
    });
    selected.truncate(config.max_union_candidates.max(1));
    for (index, curve) in selected.iter_mut().enumerate() {
        curve.union_rank = index + 1;
    }
    selected
}

pub(super) fn normalize_field(entries: &[(i64, f64)]) -> HashMap<i64, f64> {
    let maximum = entries
        .iter()
        .map(|entry| positive(entry.1))
        .fold(0.0, f64::max);
    entries
        .iter()
        .filter_map(|entry| {
            let value = positive(entry.1);
            if entry.0 > 0 && value > 0.0 {
                Some((
                    entry.0,
                    if maximum > 0.0 {
                        value / maximum
                    } else {
                        value
                    },
                ))
            } else {
                None
            }
        })
        .collect()
}

pub(super) fn load_curves(
    assets: &MemoAssets,
    candidates: &[CandidateInput],
    dimension: usize,
) -> Result<CurveLoadOutput, String> {
    let mut chunks_by_id = HashMap::new();
    let mut file_ids = HashSet::new();
    let mut chunk_source_batches = 0;
    let ids: Vec<i64> = candidates
        .iter()
        .map(|candidate| candidate.id)
        .filter(|id| *id > 0)
        .collect();
    for batch in ids.chunks(500) {
        chunk_source_batches += 1;
        for id in batch {
            if let Some(data) = assets.candidates.get(id) {
                if data.vector.len() != dimension
                    || data.vector.iter().any(|value| !value.is_finite())
                {
                    continue;
                }
                chunks_by_id.insert(*id, (data.file_id, data.vector.clone()));
                file_ids.insert(data.file_id);
            }
        }
    }
    let mut relations = FileTagRows::new();
    let mut file_tag_source_batches = 0;
    let file_ids: Vec<_> = file_ids.into_iter().collect();
    for batch in file_ids.chunks(500) {
        file_tag_source_batches += 1;
        for file_id in batch {
            if let Some(data) = assets.files.get(file_id) {
                let mut rows: Vec<_> = data
                    .tags
                    .iter()
                    .filter_map(|row| {
                        assets
                            .tag_names
                            .get(&row.tag_id)
                            .map(|name| (row.tag_id, row.position, name.clone()))
                    })
                    .collect();
                rows.sort_by_key(|row| (row.1, row.0));
                relations.insert(*file_id, rows);
            }
        }
    }
    let mut curves = Vec::with_capacity(candidates.len());
    for candidate in candidates {
        let Some((file_id, chunk_vector)) = chunks_by_id.get(&candidate.id) else {
            continue;
        };
        curves.push(Curve {
            id: candidate.id,
            file_id: *file_id,
            source_ids: assets.candidates[&candidate.id].source_ids.clone(),
            tags: Vec::new(),
            chunk_vector: chunk_vector.clone(),
            query_score: 0.0,
            denoised_score: 0.0,
            local_score: 0.0,
            transfer_score: 0.0,
            bm25_score: positive(candidate.bm25_score),
            time_score: positive(candidate.time_score),
            anchor_score: positive(candidate.anchor_score),
            union_score: 0.0,
            union_rank: 0,
            sources: Vec::new(),
        });
    }
    Ok(CurveLoadOutput {
        curves,
        relations,
        chunk_source_batches,
        file_tag_source_batches,
    })
}
