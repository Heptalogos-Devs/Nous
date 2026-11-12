//! Select lanes and calculate ranked candidate scores.
use super::delivery::RankedCandidates;
use super::*;

impl Snapshot {
    pub fn ranked(
        &self,
        mode: &str,
        text: &str,
        vector: Option<&[f32]>,
        allowed: &BTreeSet<u64>,
        o: Options,
        ghost_vectors: Vec<Vec<f32>>,
        provided: Option<&QueryObservation>,
        provided_pool: Option<&CandidatePool>,
        cancel: &CancellationToken,
    ) -> Result<NativeQueryResult> {
        if vector.is_some() {
            self.validate_query_space(&o)?;
        }
        o.validate_grouping()?;
        ensure!(
            o.grouping != "document" || o.top_k <= o.readout.max_union_candidates,
            "Requested document count exceeds max_union_candidates"
        );
        let normalized = vector.map(unit).transpose()?;
        let vector = normalized.as_deref();
        let mut local_vector = vector.map(<[f32]>::to_vec);
        QueryControls {
            top_k: o.top_k,
            group_scan_limit: o.group_scan_limit,
            thinking_min_closure: o.thinking_min_closure,
            rrf_k: o.rrf_k,
        }
        .validate()
        .map_err(|e| failure("wave.invalid_input", e))?;
        ensure!(
            allowed
                .iter()
                .all(|r| *r < self.inner.candidates.keys.len() as u64),
            "Candidate row outside snapshot"
        );
        ensure!(
            o.thinking_stages
                .iter()
                .flat_map(|s| &s.candidate_rows)
                .all(|r| *r < self.inner.candidates.keys.len() as u64),
            "Thinking candidate row outside snapshot"
        );
        if provided_pool.is_some() {
            ensure!(
                matches!(mode, "rivermemo" | "dtsc" | "hybrid"),
                "Fixed pools require rivermemo, dtsc or hybrid readout"
            );
        }
        let started = std::time::Instant::now();
        let mut channel_scores = BTreeMap::<u64, BTreeMap<String, f64>>::new();
        let mut hits = Vec::<Value>::new();
        let mut channel_ranks = BTreeMap::<u64, BTreeMap<String, usize>>::new();
        let mut offered_candidates = 0usize;
        let readout_max_union_candidates = o.readout.max_union_candidates.max(1);
        let mut diagnostics = json!({});
        let mut thinking = Value::Null;
        let score_kind;
        let mut wormhole_rrf_active = false;
        let mut used_lanes = Vec::<String>::new();
        let mut skipped = Vec::new();
        let use_vcp = matches!(mode, "tagmemo" | "rivermemo" | "dtsc")
            || (mode == "hybrid" && self.inner.assets.is_some());
        if use_vcp {
            let owned;
            if let Some(observation) = provided {
                self.validate_observation(observation)?;
                owned = None;
            } else {
                owned = Some(self.observation(
                    vector.context("Query vector required")?,
                    text,
                    &o,
                    ghost_vectors,
                    cancel,
                )?);
            }
            let observation = provided.or(owned.as_ref()).unwrap();
            let output = &observation.output;
            if mode == "tagmemo" {
                local_vector = Some(unit(&output.enhanced_vector)?);
            }
            if mode == "tagmemo" {
                score_kind = "tagmemo_cosine";
                used_lanes.push("tagmemo".into());
                for (rank, (row, score)) in self
                    .dense(
                        &output.enhanced_vector,
                        allowed,
                        o.readout.query_k,
                        &o,
                        "tagmemo",
                        cancel,
                    )?
                    .into_iter()
                    .enumerate()
                {
                    channel_ranks
                        .entry(row)
                        .or_default()
                        .insert("tagmemo".into(), rank + 1);
                    channel_scores
                        .entry(row)
                        .or_default()
                        .insert("tagmemo".into(), score);
                    hits.push(json!({"row":row,"quality_score":score,"ranking_score":score,"channels":["tagmemo"]}));
                }
            } else {
                let recalled;
                let candidate_pool = if let Some(pool) = provided_pool {
                    self.validate_pool(pool)?;
                    pool
                } else {
                    recalled = self.recall(observation, allowed, &o, cancel)?;
                    &recalled
                };
                o.lane_coverage
                    .lock()
                    .unwrap()
                    .extend(candidate_pool.lane_coverage.clone());
                let recall_lanes = candidate_pool
                    .lane_coverage
                    .keys()
                    .chain(
                        candidate_pool
                            .entries
                            .iter()
                            .filter(|entry| allowed.contains(&entry.row))
                            .flat_map(|entry| entry.lanes.keys()),
                    )
                    .cloned()
                    .collect::<BTreeSet<_>>();
                used_lanes.extend(recall_lanes);
                let mut pool = BTreeMap::<u64, (f64, f64, Vec<String>)>::new();
                for entry in &candidate_pool.entries {
                    if !allowed.contains(&entry.row) {
                        continue;
                    }
                    let row = entry.row;
                    let lanes = entry.lanes.keys().cloned().collect::<Vec<_>>();
                    for (name, lane) in &entry.lanes {
                        channel_ranks
                            .entry(row)
                            .or_default()
                            .insert(name.clone(), lane.rank);
                        channel_scores
                            .entry(row)
                            .or_default()
                            .insert(name.clone(), lane.score);
                    }
                    pool.insert(
                        row,
                        (0.0, entry.lanes.get("bm25").map_or(0.0, |v| v.score), lanes),
                    );
                }
                for (row, (score, _, _)) in &mut pool {
                    let candidate =
                        &self.inner.assets.as_ref().unwrap().candidates()[&(*row as i64 + 1)];
                    *score = candidate
                        .vector
                        .iter()
                        .zip(&output.original_query_vector)
                        .map(|(a, b)| f64::from(*a) * f64::from(*b))
                        .sum();
                }
                offered_candidates = candidate_pool.offered_candidates.max(pool.len());
                let assets = self.inner.assets.as_ref().unwrap();
                if mode == "dtsc" {
                    score_kind = "dtsc";
                    used_lanes.push("dtsc".into());
                    let graph = assets.graph();
                    let edges = graph
                        .node_ids
                        .iter()
                        .enumerate()
                        .flat_map(|(row, id)| {
                            (graph.row_offsets[row]..graph.row_offsets[row + 1]).map(move |i| {
                                (*id, graph.node_ids[graph.targets[i]], graph.weights[i])
                            })
                        })
                        .collect();
                    let metadata = &output.metadata;
                    let dtsc_input=ReferenceDtscInput{dimension:assets.dimension(),nodes:output.observation.nodes.iter().map(|n|ReferenceDtscNode{id:n.id,energy:n.energy,normalized_energy:n.normalized_energy,source_type:n.source_type.clone()}).collect(),source_field:output.observation.source_field.clone(),tag_vectors:assets.tag_vectors().iter().map(|(id,v)|(*id,v.to_vec())).collect(),inbound:graph.inbound.iter().map(|(id,v)|(*id,*v)).collect(),anchor_gain:graph.anchor_gain.iter().map(|(id,v)|(*id,*v)).collect(),edges,candidates:pool.iter().map(|(row,(score,_,_))|{let data=&assets.candidates()[&(*row as i64+1)];let file=&assets.files()[&data.file_id];ReferenceDtscCandidate{curve:ReferenceCurve{id:*row as i64+1,chunk_vector:data.vector.to_vec(),tags:file.tags.iter().filter(|t|t.position>0).map(|t|ReferenceCurveTag{id:t.tag_id,vector:assets.tag_vectors()[&t.tag_id].to_vec(),position:t.position}).collect()},score:*score}}).collect(),original_vector:output.original_query_vector.clone(),enhanced_vector:output.enhanced_vector.clone(),query_geometry_state:serde_json::from_value(json!({"epa":metadata["epa"],"pyramid":metadata["pyramid"]["features"]})).map_err(|e|failure("wave.dtsc_geometry_invalid",e))?,config:o.dtsc.clone(),top_k:0};
                    let result =
                        reference_dtsc(&dtsc_input).map_err(|e| failure("wave.dtsc_failed", e))?;
                    diagnostics = serde_json::to_value(result.diagnostics)?;
                    let mut results = result.results;
                    results.sort_by(|a, b| b.score.total_cmp(&a.score));
                    for hit in results {
                        let row = (hit.id - 1) as u64;
                        hits.push(json!({"row":row,"quality_score":hit.score,"ranking_score":hit.score,"channels":pool[&row].2,"trace":if o.include_trace{serde_json::to_value(hit)?}else{Value::Null}}));
                    }
                } else {
                    used_lanes.push("rivermemo".into());
                    let config = o.readout.clone();
                    let result = readout(
                        assets,
                        output,
                        ReadoutRequest {
                            top_k: if o.grouping == "document" || provided_pool.is_some() {
                                candidate_pool.entries.len().max(1)
                            } else {
                                readout_max_union_candidates
                            },
                            include_trace: o.include_trace,
                            pool: if let Some(prepared) = &candidate_pool.calibration {
                                if candidate_pool
                                    .entries
                                    .iter()
                                    .all(|e| allowed.contains(&e.row))
                                {
                                    prepared.clone()
                                } else {
                                    let filtered = CandidatePool {
                                        rank_scope: candidate_pool.rank_scope.clone(),
                                        identity: candidate_pool.identity.clone(),
                                        entries: candidate_pool
                                            .entries
                                            .iter()
                                            .filter(|e| allowed.contains(&e.row))
                                            .cloned()
                                            .collect(),
                                        calibration: None,
                                        offered_candidates: 0,
                                        lane_coverage: BTreeMap::new(),
                                    };
                                    self.prepare_pool(observation, &filtered, &config, true)?
                                }
                            } else {
                                let filtered = CandidatePool {
                                    rank_scope: candidate_pool.rank_scope.clone(),
                                    identity: candidate_pool.identity.clone(),
                                    entries: candidate_pool
                                        .entries
                                        .iter()
                                        .filter(|e| allowed.contains(&e.row))
                                        .cloned()
                                        .collect(),
                                    calibration: None,
                                    offered_candidates: 0,
                                    lane_coverage: BTreeMap::new(),
                                };
                                self.prepare_pool(observation, &filtered, &config, true)?
                            },
                            allowed_file_ids: None,
                            config,
                        },
                        Some(&|| cancel.is_cancelled()),
                    )
                    .map_err(|e| {
                        failure(
                            if cancel.is_cancelled() {
                                "wave.cancelled"
                            } else {
                                "wave.readout_failed"
                            },
                            e,
                        )
                    })?;
                    wormhole_rrf_active = result.wormhole_rrf_active;
                    score_kind = result.score_kind();
                    diagnostics = json!({"readout":result.diagnostics,"parameters":result.parameters,"observation":if o.include_trace{json!({"metadata":output.metadata,"sense":output.observation})}else{Value::Null}});
                    for hit in result.results {
                        let row = (hit.id - 1) as u64;
                        let trace = if o.include_trace {
                            let mut trace = serde_json::to_value(&hit)?;
                            if let Some(features) =
                                trace.as_object_mut().unwrap().remove("candidateSources")
                            {
                                trace["readoutFeatureSources"] = features;
                            }
                            trace
                        } else {
                            Value::Null
                        };
                        hits.push(json!({"row":row,"quality_score":hit.score,"ranking_score":hit.ranking_score,"channels":pool[&row].2,"chosen_path":hit.chosen_path,"paths":hit.available_paths,"trace":trace}));
                    }
                }
            }
            thinking = self.thinking(
                observation,
                &o.thinking_stages,
                o.thinking_min_closure,
                allowed,
                cancel,
            )?;
        } else {
            match mode {
                "lexical" => {
                    score_kind = "bm25";
                    used_lanes.push("lexical".into());
                    for (rank, (row, score)) in self
                        .lexical(text, allowed, o.readout.bm25_k, &o, cancel)?
                        .into_iter()
                        .enumerate()
                    {
                        channel_ranks
                            .entry(row)
                            .or_default()
                            .insert("lexical".into(), rank + 1);
                        channel_scores
                            .entry(row)
                            .or_default()
                            .insert("lexical".into(), score);
                        hits.push(json!({"row":row,"quality_score":score,"ranking_score":score,"channels":["lexical"]}));
                    }
                }
                "dense" => {
                    score_kind = "cosine";
                    used_lanes.push("dense".into());
                    for (rank, (row, score)) in self
                        .dense(
                            vector.context("Query vector required")?,
                            allowed,
                            o.readout.query_k,
                            &o,
                            "dense",
                            cancel,
                        )?
                        .into_iter()
                        .enumerate()
                    {
                        channel_ranks
                            .entry(row)
                            .or_default()
                            .insert("dense".into(), rank + 1);
                        channel_scores
                            .entry(row)
                            .or_default()
                            .insert("dense".into(), score);
                        hits.push(json!({"row":row,"quality_score":score,"ranking_score":score,"channels":["dense"]}));
                    }
                }
                "hybrid" => {
                    score_kind = "hybrid_rrf";
                    let mut scores = BTreeMap::<u64, (f64, Vec<&str>)>::new();
                    let mut lanes = vec![(
                        "lexical",
                        self.lexical(text, allowed, o.readout.bm25_k, &o, cancel)?,
                    )];
                    if self.inner.ann.is_some() {
                        lanes.push((
                            "dense",
                            self.dense(
                                vector.context("Query vector required")?,
                                allowed,
                                o.readout.query_k,
                                &o,
                                "dense",
                                cancel,
                            )?,
                        ));
                    } else {
                        skipped.push(json!({"lane":"dense","reason":"vectors unavailable"}));
                    }
                    skipped.push(
                        json!({"lane":"rivermemo","reason":"concept/fact assets unavailable"}),
                    );
                    for (lane, results) in lanes {
                        used_lanes.push(lane.into());
                        for (rank, (row, raw_score)) in results.iter().enumerate() {
                            channel_ranks
                                .entry(*row)
                                .or_default()
                                .insert(lane.into(), rank + 1);
                            channel_scores
                                .entry(*row)
                                .or_default()
                                .insert(lane.into(), *raw_score);
                            let value = scores.entry(*row).or_default();
                            value.0 += 1.0 / (o.rrf_k + rank as f64 + 1.0);
                            value.1.push(lane);
                        }
                    }
                    for (row, (score, lanes)) in scores {
                        hits.push(json!({"row":row,"quality_score":null,"ranking_score":score,"channels":lanes}));
                    }
                    hits.sort_by(|a, b| {
                        b["ranking_score"]
                            .as_f64()
                            .unwrap()
                            .total_cmp(&a["ranking_score"].as_f64().unwrap())
                    });
                }
                _ => {
                    return Err(failure(
                        "wave.invalid_input",
                        format!("Unknown query mode {mode}"),
                    ));
                }
            }
        }
        self.deliver(
            mode,
            text,
            allowed,
            &o,
            provided_pool,
            cancel,
            RankedCandidates {
                hits,
                channel_scores,
                channel_ranks,
                offered_candidates,
                diagnostics,
                thinking,
                score_kind,
                wormhole_rrf_active,
                used_lanes,
                skipped,
                started,
                readout_max_union_candidates,
                local_vector,
            },
        )
    }
}
