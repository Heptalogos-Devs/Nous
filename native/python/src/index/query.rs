//! Coarse native execution over the frozen authorized generation.
use super::*;
impl Frozen {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn query(
        &self,
        mode: &str,
        text: &str,
        vector: Option<&[f32]>,
        allowed: &BTreeSet<u64>,
        allowed_graph: Option<&BTreeSet<u64>>,
        options: QueryOptions,
        ghost_vectors: Option<Vec<Vec<f32>>>,
        cancel: &CancellationToken,
    ) -> Result<SearchResult> {
        let start = Instant::now();
        cancel.check()?;
        ensure!(
            (1..=10000).contains(&options.limit),
            "limit must be between 1 and 10000"
        );
        ensure!(
            allowed
                .iter()
                .all(|id| (*id as usize) < self.data.references.len()),
            "allowed row out of bounds"
        );
        ensure!(
            allowed_graph.is_none_or(|ids| ids
                .iter()
                .all(|id| (*id as usize) < self.data.assets.tags.len())),
            "allowed graph node out of bounds"
        );
        ensure!(
            [options.lexical_weight, options.dense_weight, options.rrf_k]
                .iter()
                .all(|v| v.is_finite() && *v >= 0.0)
                && options.rrf_k > 0.0,
            "fusion parameters must be finite and nonnegative; rrf_k positive"
        );
        let allowed_refs = allowed
            .iter()
            .map(|row| self.data.references[*row as usize].clone())
            .collect::<BTreeSet<_>>();
        let lexical = || -> Result<Vec<(u64, f64)>> {
            Ok(self
                .lexical
                .search_allowed(text, "snapshot", "snapshot", options.limit, &allowed_refs)?
                .hits
                .into_iter()
                .map(|hit| (self.reference_rows[&hit.id], f64::from(hit.score)))
                .collect())
        };
        let dense = || -> Result<Vec<(u64, f64)>> {
            let engine = self
                .dense
                .as_ref()
                .context("CAPABILITY_MISSING: dense vectors are not available")?;
            let query = vector.context("query vector required")?;
            let ids = allowed.iter().map(|row| *row as i64 + 1).collect();
            Ok(engine
                .search_candidates(query, options.limit, Some(&ids))?
                .into_iter()
                .map(|hit| ((hit.id - 1) as u64, hit.similarity))
                .collect())
        };
        let mut diagnostics = json!({"mode":mode,"allowed_rows":allowed.len()});
        let mut ranked = match mode {
            "lexical" => lexical()?,
            "dense" => dense()?,
            "hybrid" => {
                let mut scores = BTreeMap::<u64, f64>::new();
                for (hits, weight) in [
                    (lexical()?, options.lexical_weight),
                    (dense()?, options.dense_weight),
                ] {
                    for (rank, (row, _)) in hits.into_iter().enumerate() {
                        *scores.entry(row).or_default() +=
                            weight / (options.rrf_k + rank as f64 + 1.0)
                    }
                }
                scores.into_iter().collect()
            }
            "vcp" => {
                let engine = self
                    .vcp
                    .as_ref()
                    .context("CAPABILITY_MISSING: VCP requires tag vectors and memberships")?;
                let query = vector.context("query vector required")?;
                let keys = allowed_graph.map(|ids| {
                    ids.iter()
                        .map(|id| self.data.assets.tags[*id as usize].key.clone())
                        .collect()
                });
                let config = options.vcp.into_config(ghost_vectors)?;
                let lexical_hits = if text.trim().is_empty() {
                    Vec::new()
                } else {
                    lexical()?
                        .into_iter()
                        .map(|(row, score)| nous_retrieval::serving::ServingLexicalHit {
                            reference: self.data.references[row as usize].clone(),
                            score,
                        })
                        .collect()
                };
                let result = engine.search_authorized(
                    query,
                    options.limit,
                    ServingProfile::Vcp(Box::new(config)),
                    nous_retrieval::serving::ServingQueryScope {
                        references: &allowed_refs,
                        tag_keys: keys.as_ref(),
                        lexical: &lexical_hits,
                    },
                    &|| cancel.cancelled(),
                )?;
                diagnostics["vcp"] = result.diagnostics;
                diagnostics["candidate_traces"] = json!(
                    result
                        .ranked
                        .iter()
                        .filter(|hit| hit.trace.is_some())
                        .map(|hit| json!({"row_id":self.reference_rows[&hit.id],"trace":hit.trace}))
                        .collect::<Vec<_>>()
                );
                result
                    .ranked
                    .into_iter()
                    .map(|hit| (self.reference_rows[&hit.id], hit.score))
                    .collect()
            }
            _ => anyhow::bail!("unknown query mode {mode}"),
        };
        ranked.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        ranked.truncate(options.limit);
        diagnostics["elapsed_ms"] = json!(start.elapsed().as_secs_f64() * 1000.0);
        Ok(SearchResult {
            rows: ranked.iter().map(|(row, _)| *row).collect(),
            scores: ranked.iter().map(|(_, score)| *score).collect(),
            generation: self.generation.clone(),
            diagnostics_json: diagnostics.to_string(),
        })
    }
}
