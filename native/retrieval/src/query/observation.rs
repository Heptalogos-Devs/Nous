//! Validate observations and prepare grounded Thinking inputs.
use super::*;

impl Snapshot {
    pub(super) fn validate_query_space(&self, o: &Options) -> Result<()> {
        if o.query_space_signature.as_deref()
            != self.inner.input.identity["space_signature"].as_str()
        {
            return Err(failure(
                "wave.space_mismatch",
                "Query vector space identity differs from this snapshot",
            ));
        }
        Ok(())
    }
    pub fn observation(
        &self,
        vector: &[f32],
        text: &str,
        o: &Options,
        ghost_vectors: Vec<Vec<f32>>,
        cancel: &CancellationToken,
    ) -> Result<QueryObservation> {
        self.validate_query_space(o)?;
        let assets = self.inner.assets.as_ref().ok_or_else(|| {
            failure(
                "wave.capability_missing",
                "VCP requires concept vectors and source facts",
            )
        })?;
        let ann = self
            .inner
            .ann
            .as_ref()
            .context("Candidate/Concept index missing")?;
        ensure!(
            o.ghosts.len() == ghost_vectors.len(),
            "Ghost metadata/vector rows mismatch"
        );
        ensure!(
            o.core_rows
                .iter()
                .all(|r| *r < self.inner.concepts.keys.len() as u64),
            "Core concept row out of bounds"
        );
        let vector = unit(vector)?;
        let search =
            |vector: &[f32], limit: usize| -> std::result::Result<Vec<(i64, f64)>, String> {
                ann.search_tags(vector, limit)
                    .map(|hits| hits.into_iter().map(|h| (h.id, h.similarity)).collect())
                    .map_err(|e| e.to_string())
            };
        let config: PipelineConfig = serde_json::from_value(serde_json::to_value(&o.pipeline)?)?;
        let records_enabled = config.spike_routing.max_transition_records > 0;
        let output = run_pipeline(
            assets,
            &search,
            PipelineInput {
                query_id: Some(uuid::Uuid::new_v4().to_string()),
                query_text: text.into(),
                query_vector: vector,
                core_tag_ids: o.core_rows.iter().map(|r| *r as i64 + 1).collect(),
                ghost_tags: o
                    .ghosts
                    .iter()
                    .zip(ghost_vectors)
                    .map(|(g, v)| GhostTagInput {
                        name: g.name.clone(),
                        is_core: g.is_core,
                        vector: v,
                    })
                    .collect(),
                config,
            },
            Some(&|| cancel.is_cancelled()),
        )
        .map_err(|e| {
            if cancel.is_cancelled() {
                failure("wave.cancelled", e)
            } else {
                failure("wave.observation_failed", e)
            }
        })?;
        let diagnostics_json = if o.include_trace {
            json!({"metadata":output.metadata,"observation":output.observation}).to_string()
        } else {
            json!({"metadata":output.metadata,"nodes":output.observation.nodes.len(),"transitions":output.observation.transitions.len(),"transitions_truncated":output.observation.transitions_truncated}).to_string()
        };
        Ok(QueryObservation {
            output: Arc::new(output),
            identity: self.inner.input.identity.clone(),
            text: text.into(),
            core_rows: o.core_rows.clone(),
            records_enabled,
            diagnostics_json,
        })
    }
    pub fn validate_observation(&self, observation: &QueryObservation) -> Result<()> {
        if observation.identity != self.inner.input.identity {
            return Err(failure(
                "wave.observation_mismatch",
                "Observation identity mismatch",
            ));
        }
        let assets = self
            .inner
            .assets
            .as_ref()
            .ok_or_else(|| failure("wave.capability_missing", "VCP unavailable"))?;
        observation
            .output
            .validate_for(assets)
            .map_err(|e| failure("wave.observation_mismatch", e))
    }
    pub fn neighborhood(
        &self,
        rows: Vec<u64>,
        limit: usize,
        token: &CancellationToken,
    ) -> Result<String> {
        let assets = self
            .inner
            .assets
            .as_ref()
            .ok_or_else(|| failure("wave.capability_missing", "Graph unavailable"))?;
        let graph = assets.graph();
        let mut edges = Vec::new();
        let mut nodes = Vec::new();
        let selected = rows.iter().copied().collect::<BTreeSet<_>>();
        for row in rows {
            token.check()?;
            ensure!(
                row < self.inner.concepts.keys.len() as u64,
                "Concept row out of bounds"
            );
            let mut outgoing = 0.0;
            let mut outdegree = 0;
            if let Some(index) = graph.node_index.get(&(row as i64 + 1)) {
                for cursor in graph.row_offsets[*index]..graph.row_offsets[*index + 1] {
                    outdegree += 1;
                    outgoing += graph.weights[cursor];
                    edges.push(json!({"source":row,"target":graph.node_ids[graph.targets[cursor]]-1,"weight":graph.weights[cursor]}));
                }
            }
            let mut roots = BTreeSet::new();
            for seq in 0..self.inner.facts.sequence_keys.len() {
                if slice(
                    &self.inner.input.facts.indptr,
                    &self.inner.input.facts.indices,
                    seq,
                )
                .contains(&row)
                {
                    roots.insert(self.inner.input.facts.sequence_roots[seq]);
                }
            }
            let candidate_rows = self
                .inner
                .concept_candidates
                .get(&row)
                .cloned()
                .unwrap_or_default();
            nodes.push(json!({"row":row,"independent_roots":roots.len(),"root_rows":roots,"inbound_weight":graph.inbound.get(&(row as i64+1)).copied().unwrap_or(0.0),"outbound_weight":outgoing,"outdegree":outdegree,"candidate_coverage":candidate_rows.len(),"candidate_rows":candidate_rows,"source_witness_groups":assets.root_facts().values().filter(|f|f.tags.contains(&(row as i64+1))).map(|f|f.required_roots.iter().map(|r|r-1).collect::<Vec<_>>()).collect::<Vec<_>>()}));
        }
        let assoc = &self.inner.input.associations;
        let associations=(0..assoc.sources.len()).filter(|i|selected.contains(&assoc.sources[*i])||selected.contains(&assoc.targets[*i])).map(|i|json!({"key":self.inner.associations.keys[i],"relation":self.inner.associations.relations[i],"source":assoc.sources[i],"target":assoc.targets[i],"directed":assoc.directed[i],"strength":assoc.strengths[i],"confidence":assoc.confidences[i],"polarity":assoc.polarities[i],"support_group":self.inner.associations.support_groups[assoc.edge_groups[i] as usize],"evidence_roots":slice(&assoc.group_root_indptr,&assoc.group_root_indices,assoc.edge_groups[i] as usize)})).collect::<Vec<_>>();
        let total_edges = edges.len();
        edges.truncate(limit);
        Ok(json!({"nodes":nodes,"edges":edges,"associations":associations,"truncated":total_edges>limit}).to_string())
    }
    pub fn validate_candidates(&self, candidates: &CandidateSet) -> Result<()> {
        ensure!(
            matches!(
                candidates.rank_scope.as_str(),
                "candidate_lane" | "grouped_lane"
            ),
            "Invalid candidate set rank_scope"
        );
        if candidates.identity != self.inner.input.identity {
            return Err(crate::Failure {
                code: "wave.candidate_set_mismatch",
                message: "Candidate set belongs to another snapshot".into(),
                details: json!({"field":"identity","expected":self.inner.input.identity,"actual":candidates.identity}),
            }.into());
        }
        let invalid = |field: &str, message: &str, mut details: Value| -> anyhow::Error {
            details["field"] = json!(field);
            crate::Failure {
                code: "wave.invalid_input",
                message: message.into(),
                details,
            }
            .into()
        };
        let mut rows = BTreeSet::new();
        for entry in &candidates.entries {
            if entry.row >= self.inner.candidates.keys.len() as u64 {
                return Err(invalid(
                    "entries.row",
                    "Candidate set row is outside this snapshot",
                    json!({"row":entry.row,"candidate_count":self.inner.candidates.keys.len()}),
                ));
            }
            if !rows.insert(entry.row) {
                return Err(invalid(
                    "entries.row",
                    "Candidate set contains a duplicate row",
                    json!({"row":entry.row}),
                ));
            }
            for (name, lane) in &entry.lanes {
                if lane.rank == 0 {
                    return Err(invalid(
                        "entries.lanes.rank",
                        "Candidate lane rank must be positive",
                        json!({"row":entry.row,"lane":name,"rank":lane.rank}),
                    ));
                }
                if !lane.score.is_finite() || !lane.weight.is_finite() || lane.weight < 0.0 {
                    return Err(invalid(
                        "entries.lanes.score",
                        "Candidate lane score must be finite",
                        json!({"row":entry.row,"lane":name,"score":lane.score}),
                    ));
                }
            }
        }
        Ok(())
    }
    pub(super) fn prepare_pool(
        &self,
        observation: &QueryObservation,
        pool: &CandidateSet,
        config: &NativeConfig,
        fixed: bool,
    ) -> Result<PreparedPool> {
        let assets = self.inner.assets.as_ref().context("VCP assets required")?;
        let candidates = pool
            .entries
            .iter()
            .map(|entry| {
                let data = &assets.candidates()[&(entry.row as i64 + 1)];
                let score = data
                    .vector
                    .iter()
                    .zip(&observation.output.original_query_vector)
                    .map(|(a, b)| f64::from(*a) * f64::from(*b))
                    .sum();
                CandidateInput {
                    id: entry.row as i64 + 1,
                    score,
                    hybrid_score: 0.0,
                    vector_score: score,
                    bm25_score: entry.lanes.get("bm25").map_or(0.0, |l| l.score),
                    time_score: 0.0,
                    anchor_score: if entry.lanes.contains_key("direct_anchor") {
                        1.0
                    } else {
                        0.0
                    },
                }
            })
            .collect();
        prepare_pool(assets, &observation.output, candidates, config, fixed)
            .map_err(|e| failure("wave.candidate_calibration_failed", e))
    }
    pub fn thinking(
        &self,
        observation: &QueryObservation,
        stages: &[ThinkingStage],
        minimum: f64,
        allowed: &BTreeSet<u64>,
        cancel: &CancellationToken,
    ) -> Result<Value> {
        if stages.is_empty() {
            return Ok(Value::Null);
        }
        if cancel.is_cancelled() {
            cancel.check()?;
        }
        if stages
            .iter()
            .flat_map(|s| &s.candidate_rows)
            .any(|r| *r >= self.inner.candidates.keys.len() as u64)
        {
            return Err(failure(
                "wave.invalid_input",
                "Thinking candidate row is outside this snapshot",
            ));
        }
        let assets = self.inner.assets.as_ref().unwrap();
        let input = ThinkingInput {
            stages: stages
                .iter()
                .map(|s| Stage {
                    diary_name: s.name.clone(),
                    k: s.top_k,
                    candidate_ids: s
                        .candidate_rows
                        .iter()
                        .filter(|r| allowed.contains(r))
                        .map(|r| *r as i64 + 1)
                        .collect(),
                })
                .collect(),
            min_closure: minimum,
        };
        input
            .validate()
            .map_err(|e| failure("wave.invalid_input", e))?;
        if !observation.records_enabled {
            return Ok(
                json!({"unavailable":{"code":"thinking.records_disabled","message":"Transition recording is disabled"}}),
            );
        }
        if observation.output.observation.transitions_truncated {
            return Ok(
                json!({"unavailable":{"code":"thinking.records_truncated","message":"Transition record limit reached"}}),
            );
        }
        if observation.output.observation.transitions.is_empty() {
            return Ok(
                json!({"unavailable":{"code":"thinking.records_missing","message":"No recorded route is available"}}),
            );
        }
        match plan(
            assets,
            &observation.output,
            input,
            Some(&|| cancel.is_cancelled()),
        ) {
            Ok(output) => Ok(
                json!({"stages":output.stages.into_iter().map(|s|json!({"name":s.diary_name,"results":s.results.into_iter().map(|r|json!({"row":r.chunk_id-1,"score":r.score,"chosen_path":r.chosen_path,"parents":r.parents.iter().map(|p|p-1).collect::<Vec<_>>(),"relation":r.relation,"route":r.route})).collect::<Vec<_>>()})).collect::<Vec<_>>() }),
            ),
            Err(ThinkingError::NoGroundedRoute) => Ok(
                json!({"unavailable":{"code":"thinking.no_grounded_route","message":"No candidate grounds a route in this observation"}}),
            ),
            Err(ThinkingError::RecordsUnavailable { truncated }) => Ok(
                json!({"unavailable":{"code":if truncated{"thinking.records_truncated"}else{"thinking.records_missing"}}}),
            ),
            Err(ThinkingError::Cancelled) => Err(failure("wave.cancelled", "Thinking cancelled")),
            Err(ThinkingError::Invalid(reason)) => Err(failure("wave.invalid_input", reason)),
        }
    }
}
