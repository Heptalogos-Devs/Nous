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
            ServingProfile::Vcp(config) => self.vcp_search(
                query,
                limit,
                *config,
                ScopedVcpCandidates {
                    allowed: None,
                    tags: None,
                    lexical: &BTreeMap::new(),
                },
                None,
            ),
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
            "graph_nodes":self.asset_diagnostics["graph_nodes"],"graph_edges":self.asset_diagnostics["graph_edges"],"artifact_signature":self.assets.signature(),"graph":self.asset_diagnostics["graph"]})
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
        scope: ScopedVcpCandidates<'_>,
        cancelled: nous_vcp::CancellationCheck<'_>,
    ) -> Result<ServingResult> {
        let check_cancel = || -> Result<()> {
            ensure!(
                !cancelled.is_some_and(|check| check()),
                "CANCELLED: VCP computation cancelled"
            );
            Ok(())
        };
        check_cancel()?;
        let scoped_assets = self.restrict_assets(scope.allowed, scope.tags)?;
        let assets = scoped_assets.as_ref().unwrap_or(&self.assets);
        let effective_tags = assets
            .tag_vectors()
            .keys()
            .copied()
            .collect::<BTreeSet<_>>();
        ensure!(
            config.core_keys.len() <= 256 && config.ghost_tags.len() <= 256,
            "VCP query cue budget exceeded"
        );
        let mut core_ids = BTreeSet::new();
        for key in &config.core_keys {
            let id = self
                .mappings
                .tags
                .iter()
                .find(|(id, value)| effective_tags.contains(*id) && *value == key)
                .map(|(id, _)| *id)
                .context("VCP_CORE_UNAVAILABLE: exact core is outside authorized graph assets")?;
            core_ids.insert(id);
        }
        let tag_search =
            |vector: &[f32], count: usize| -> std::result::Result<Vec<(i64, f64)>, String> {
                let hits = self
                    .ann
                    .search_tags_allowed(vector, count, &effective_tags)
                    .map_err(|error| error.to_string())?;
                Ok(hits
                    .into_iter()
                    .map(|hit| (hit.id, hit.similarity))
                    .collect())
            };
        // Derive from the original query once; never mutate adopted provider bytes.
        let normalized_query = unit_vector(query, self.identity.dimension)?;
        for ghost in &mut config.ghost_tags {
            ghost.vector = unit_vector(&ghost.vector, self.identity.dimension)?;
        }
        if config.thinking && config.pipeline.spike_routing.max_transition_records == 0 {
            config.pipeline.spike_routing.max_transition_records = 4096;
        }
        let observation = run_pipeline(
            assets,
            &tag_search,
            PipelineInput {
                query_id: None,
                _query_text: String::new(),
                query_vector: normalized_query,
                core_tag_ids: core_ids.iter().copied().collect(),
                ghost_tags: config.ghost_tags,
                config: config.pipeline,
            },
            cancelled,
        )
        .map_err(anyhow::Error::msg)?;
        check_cancel()?;
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
            check_cancel()?;
            let hits = self
                .ann
                .search_candidates(vector, candidate_limit, scope.allowed)?;
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
        lanes.insert("bm25", json!({"hits":scope.lexical.len()}));
        for (id, score) in scope.lexical {
            offered
                .entry(*id)
                .and_modify(|candidate| candidate.bm25_score = *score)
                .or_insert(CandidateInput {
                    id: *id,
                    score: 0.0,
                    vector_score: 0.0,
                    hybrid_score: 0.0,
                    bm25_score: *score,
                    time_score: 0.0,
                    anchor_score: 0.0,
                });
        }
        let contact_score = |id: i64| {
            assets
                .candidates()
                .get(&id)
                .and_then(|data| assets.files().get(&data.file_id))
                .map(|file| {
                    file.tags
                        .iter()
                        .filter(|tag| core_ids.contains(&tag.tag_id))
                        .map(|tag| tag.tag_id)
                        .collect::<BTreeSet<_>>()
                        .len() as f64
                        / core_ids.len().max(1) as f64
                })
                .unwrap_or(0.0)
        };
        let core_limit = config
            .readout
            .anchor_k
            .min(candidate_limit)
            .min(limit)
            .min(config.readout.max_union_candidates);
        let mut core_offers: Vec<(i64, f64)> = Vec::new();
        let mut core_matches = 0usize;
        if !core_ids.is_empty() && core_limit > 0 {
            for (index, id) in assets.candidates().keys().enumerate() {
                if index % 256 == 0 {
                    check_cancel()?;
                }
                let score = contact_score(*id);
                if score <= 0.0 {
                    continue;
                }
                core_matches += 1;
                let position = core_offers.partition_point(|(old_id, old_score)| {
                    old_score.total_cmp(&score).is_gt() || (*old_score == score && old_id < id)
                });
                if position < core_limit {
                    core_offers.insert(position, (*id, score));
                    core_offers.truncate(core_limit);
                }
            }
        }
        lanes.insert(
            "core",
            json!({"hits":core_offers.len(),"matched":core_matches,"limit":core_limit}),
        );
        for (id, anchor_score) in core_offers {
            offered.entry(id).or_insert(CandidateInput {
                id,
                score: 0.0,
                hybrid_score: 0.0,
                vector_score: 0.0,
                bm25_score: 0.0,
                time_score: 0.0,
                anchor_score,
            });
        }
        for candidate in offered.values_mut() {
            candidate.anchor_score = contact_score(candidate.id);
        }
        config.readout.max_union_candidates = config.readout.max_union_candidates.min(limit);
        let native = readout(
            assets,
            &observation,
            ReadoutRequest {
                top_k: limit,
                include_trace: config.include_trace,
                candidates: offered.into_values().collect(),
                allowed_file_ids: Some(assets.files().keys().copied().collect()),
                config: config.readout,
            },
            cancelled,
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
                assets,
                &observation,
                ThinkingInput {
                    stages: vec![Stage {
                        diary_name: "authorized-snapshot".into(),
                        k: limit.clamp(1, 32),
                        candidate_ids: native
                            .results
                            .iter()
                            .take(256)
                            .map(|item| item.id)
                            .collect(),
                    }],
                    min_closure: config.thinking_min_closure,
                },
            ) {
                Ok(value) => {
                    source_mappings.extend(self.sources_for(value.stages.iter().flat_map(
                        |stage| stage.results.iter().map(|selection| selection.chunk_id),
                    ))?);
                    self.thinking_output(value)?
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
                "candidate_lanes":lanes,"core_keys":self.mappings.tags.iter().filter(|(id,_)|core_ids.contains(*id)).map(|(_,key)|key).collect::<Vec<_>>(),
                "pipeline":observation.metadata,"river":native.diagnostics,"omega":native.omega,"thinking":thinking,"fields":fields}),
        })
    }
}
