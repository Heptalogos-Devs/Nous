//! Dense and complete VCP multi-lane query execution.
use super::*;

impl ServingGeneration {
    pub fn search(
        &self,
        query: &[f32],
        limit: usize,
        profile: ServingProfile,
    ) -> Result<ServingResult> {
        ensure!(
            limit > 0,
            "SERVING_LIMIT_INVALID: search limit must be positive"
        );
        match profile {
            ServingProfile::Dense => {
                let hits = self.ann.search_candidates(query, limit, None)?;
                let source_mappings = self.sources_for(hits.iter().map(|hit| hit.id))?;
                let ranked = hits
                    .into_iter()
                    .map(|hit| self.materialize(hit.id, hit.similarity, None))
                    .collect::<Result<_>>()?;
                Ok(ServingResult {
                    ranked,
                    generation: self.identity.generation.clone(),
                    space: self.identity.space.clone(),
                    source_mappings,
                    diagnostics: json!({"profile":"dense","engine":"usearch","restored":self.restored,"assets":self.query_asset_diagnostics()}),
                })
            }
            ServingProfile::Vcp(config) => self.vcp_search(query, limit, *config),
        }
    }
    pub(super) fn sources_for(
        &self,
        ids: impl Iterator<Item = i64>,
    ) -> Result<BTreeMap<String, ServingSourceMapping>> {
        ids.map(|id| {
            let candidate = self
                .mappings
                .candidates
                .get(&id)
                .context("SERVING_CANDIDATE_UNKNOWN: source mapping escaped snapshot")?;
            Ok((
                candidate.id.clone(),
                ServingSourceMapping {
                    source_refs: candidate.source_refs.clone(),
                },
            ))
        })
        .collect()
    }
    pub(super) fn query_asset_diagnostics(&self) -> Value {
        json!({"candidate_count":self.mappings.candidates.len(),"tag_count":self.mappings.tags.len(),"source_root_count":self.mappings.roots.len(),"vector_policy":VECTOR_POLICY,
            "graph_nodes":self.asset_diagnostics["graph_nodes"],"graph_edges":self.asset_diagnostics["graph_edges"],"artifact_signature":self.assets.signature()})
    }
    pub(super) fn materialize(
        &self,
        id: i64,
        score: f64,
        trace: Option<Value>,
    ) -> Result<ServingHit> {
        let candidate = self
            .mappings
            .candidates
            .get(&id)
            .context("SERVING_CANDIDATE_UNKNOWN: readout escaped frozen mapping")?;
        ensure!(
            score.is_finite(),
            "SERVING_SCORE_INVALID: non-finite readout score"
        );
        Ok(ServingHit {
            id: candidate.id.clone(),
            commit_seq: candidate.commit_seq,
            score,
            trace,
        })
    }
    pub(super) fn vcp_search(
        &self,
        query: &[f32],
        limit: usize,
        mut config: VcpConfig,
    ) -> Result<ServingResult> {
        // Derive from the original query once; never mutate adopted provider bytes.
        let normalized_query = unit_vector(query, self.identity.dimension)?;
        for ghost in &mut config.ghost_tags {
            ghost.vector = unit_vector(&ghost.vector, self.identity.dimension)?;
        }
        if config.thinking && config.pipeline.spike_routing.max_transition_records == 0 {
            config.pipeline.spike_routing.max_transition_records = 4096;
        }
        let observation = run_pipeline(
            &self.assets,
            &self.ann,
            PipelineInput {
                query_id: None,
                _query_text: String::new(),
                query_vector: normalized_query,
                core_tags: config.core_tags,
                ghost_tags: config.ghost_tags,
                config: config.pipeline,
            },
        )
        .map_err(anyhow::Error::msg)?;
        let candidate_limit = config
            .candidate_limit
            .max(limit)
            .min(self.mappings.candidates.len());
        let mut offered = BTreeMap::<i64, CandidateInput>::new();
        let mut lanes = BTreeMap::new();
        // Copied multi-query offering contract: original/enhanced/local/transfer;
        // one observation, one candidate union, one readout.
        for (name, vector) in [
            // ANN normalizes the raw query itself using the identical policy;
            // feeding this lane the derived unit buffer would normalize it twice.
            ("original", query),
            ("enhanced", observation.enhanced_vector.as_slice()),
            ("local", observation.local_vector.as_slice()),
            ("transfer", observation.transfer_vector.as_slice()),
        ] {
            if vector.iter().all(|value| *value == 0.0) {
                lanes.insert(name, json!({"skipped":"zero field","hits":0}));
                continue;
            }
            let hits = self.ann.search_candidates(vector, candidate_limit, None)?;
            lanes.insert(name, json!({"hits":hits.len()}));
            for hit in hits {
                offered
                    .entry(hit.id)
                    .and_modify(|candidate| {
                        candidate.score = candidate.score.max(hit.similarity);
                        candidate.vector_score = candidate.vector_score.max(hit.similarity);
                    })
                    .or_insert(CandidateInput {
                        id: hit.id,
                        score: hit.similarity,
                        vector_score: hit.similarity,
                        hybrid_score: 0.0,
                        bm25_score: 0.0,
                        time_score: 0.0,
                        anchor_score: 0.0,
                    });
            }
        }
        let candidate_ids = offered.keys().copied().collect::<Vec<_>>();
        let native = readout(
            &self.assets,
            &observation,
            ReadoutRequest {
                top_k: limit,
                include_trace: config.include_trace,
                candidates: offered.into_values().collect(),
                allowed_file_ids: Some(self.mappings.source_sets.keys().copied().collect()),
                config: config.readout,
            },
        )
        .map_err(anyhow::Error::msg)?;
        let mut source_mappings = self.sources_for(native.results.iter().map(|hit| hit.id))?;
        let ranked = native
            .results
            .iter()
            .map(|hit| {
                self.materialize(
                    hit.id,
                    hit.score,
                    if config.include_trace {
                        Some(serde_json::to_value(hit)?)
                    } else {
                        None
                    },
                )
            })
            .collect::<Result<_>>()?;
        let thinking = if config.thinking {
            match plan(
                &self.assets,
                &observation,
                ThinkingInput {
                    stages: vec![Stage {
                        diary_name: "authorized-snapshot".into(),
                        k: limit.clamp(1, 32),
                        candidate_ids: candidate_ids.into_iter().take(256).collect(),
                    }],
                    min_closure: 0.2,
                },
            ) {
                Ok(value) => {
                    source_mappings.extend(self.sources_for(value.stages.iter().flat_map(
                        |stage| stage.results.iter().map(|selection| selection.chunk_id),
                    ))?);
                    json!({"status":"ready","plan":value})
                }
                Err(error) => json!({"status":"unavailable","reason":error}),
            }
        } else {
            json!({"status":"not_requested"})
        };
        let fields = if config.include_fields {
            json!({"local":observation.local_field,"transfer":observation.transfer_field,
            "local_vector":observation.local_vector,"transfer_vector":observation.transfer_vector,"observation":observation.observation})
        } else {
            Value::Null
        };
        Ok(ServingResult {
            ranked,
            generation: self.identity.generation.clone(),
            space: self.identity.space.clone(),
            source_mappings,
            diagnostics: json!({"profile":"vcp","engine":"usearch+ags+memo+river-v3","restored":self.restored,"assets":self.query_asset_diagnostics(),
                "candidate_lanes":lanes,"pipeline":observation.metadata,"river":native.diagnostics,"omega":native.omega,"thinking":thinking,"fields":fields}),
        })
    }
}
