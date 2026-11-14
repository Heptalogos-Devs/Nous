//! Readout operators consume explicit candidate sets and never perform recall.
use super::delivery::RankedCandidates;
use super::*;

impl Snapshot {
    pub(super) fn rank_cosine(
        &self,
        candidates: &CandidateSet,
        vector: &[f32],
        options: &Options,
    ) -> Result<RankedCandidates> {
        self.validate_candidates(candidates)?;
        let vectors =
            self.inner.candidate_units.as_ref().ok_or_else(|| {
                failure("wave.capability_missing", "Candidate vectors unavailable")
            })?;
        let vector = unit(vector)?;
        let mut scores = candidates
            .entries
            .iter()
            .map(|entry| {
                let score = vectors[entry.row as usize]
                    .iter()
                    .zip(&vector)
                    .map(|(a, b)| f64::from(*a) * f64::from(*b))
                    .sum::<f64>();
                (entry.row, score)
            })
            .collect::<Vec<_>>();
        scores.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        let mut ranked = self.ranked_shell(options, Some(vector));
        ranked.score_kind = "cosine".into();
        ranked.used_lanes.push("fixed_cosine".into());
        ranked.offered_candidates = candidates.offered_candidates.max(scores.len());
        for (rank, (row, score)) in scores.into_iter().enumerate() {
            ranked
                .channel_scores
                .entry(row)
                .or_default()
                .insert("fixed_cosine".into(), score);
            ranked
                .channel_ranks
                .entry(row)
                .or_default()
                .insert("fixed_cosine".into(), rank + 1);
            ranked.hits.push(json!({"row":row,"quality_score":score,"ranking_score":score,"channels":["fixed_cosine"]}));
        }
        Ok(ranked)
    }

    pub(super) fn sort_ranked(
        &self,
        ranked: &mut RankedCandidates,
        sort: Option<&str>,
        options: &Options,
    ) -> Result<()> {
        let Some(sort) = sort else {
            return Ok(());
        };
        let parts = sort.split(',').collect::<Vec<_>>();
        ensure!(
            !parts.is_empty()
                && parts.len() <= 2
                && matches!(
                    parts[0],
                    "occurred" | "observed" | "formed" | "valid" | "recorded"
                )
                && (parts.len() == 1 || matches!(parts[1], "asc" | "desc")),
            "Invalid time sort"
        );
        let values = options
            .sort_values
            .get(parts[0])
            .context("Time sort values are unavailable")?;
        let mut domains = BTreeSet::new();
        for hit in &ranked.hits {
            let row = hit["row"].as_u64().context("Ranked row missing")? as usize;
            let group = self.inner.candidates.group_rows[row] as usize;
            if let Some(Some(point)) = values.get(&self.inner.candidates.group_keys[group]) {
                domains.insert(point.domain.as_str());
            }
        }
        if domains.len() > 1 {
            return Err(failure(
                "time.domain_mismatch",
                "Cannot sort values from different time domains",
            ));
        }
        let descending = parts.get(1) == Some(&"desc");
        ranked.hits.sort_by(|left, right| {
            let lookup = |hit: &Value| {
                let row = hit["row"].as_u64().unwrap() as usize;
                let group = self.inner.candidates.group_rows[row] as usize;
                values
                    .get(&self.inner.candidates.group_keys[group])
                    .and_then(Option::as_ref)
                    .map(|value| value.ticks)
            };
            match (lookup(left), lookup(right)) {
                (Some(a), Some(b)) => {
                    if descending {
                        b.cmp(&a)
                    } else {
                        a.cmp(&b)
                    }
                }
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => std::cmp::Ordering::Equal,
            }
        });
        ranked.diagnostics["sort"] = json!(sort);
        Ok(())
    }

    pub(super) fn rank_scope(
        &self,
        allowed: &BTreeSet<u64>,
        options: &Options,
        local_vector: Option<Vec<f32>>,
    ) -> RankedCandidates {
        let mut ranked = self.ranked_shell(options, local_vector);
        ranked.score_kind = "scope_order".into();
        let mut seen = BTreeSet::new();
        for row in allowed {
            let group = self.inner.candidates.group_rows[*row as usize];
            if options.grouping == "document" && !seen.insert(group) {
                continue;
            }
            let rank = ranked.hits.len() + 1;
            ranked.hits.push(json!({"row":row,"quality_score":null,
                "ranking_score":1.0 / (options.rrf_k + rank as f64),"channels":["scope"]}));
        }
        ranked.offered_candidates = ranked.hits.len();
        ranked.used_lanes.push("scope".into());
        ranked
    }

    pub(super) fn apply_preferences(
        &self,
        base: &RankedCandidates,
        prefer: &[String],
        avoid: &[String],
        allowed: &BTreeSet<u64>,
        options: &Options,
        cancel: &CancellationToken,
    ) -> Result<RankedCandidates> {
        if prefer.is_empty() && avoid.is_empty() {
            return Ok(base.clone());
        }
        let mut groups = BTreeMap::<u64, f64>::new();
        for hit in &base.hits {
            let row = hit["row"].as_u64().context("Ranked row missing")?;
            let group = self.inner.candidates.group_rows[row as usize];
            if !groups.contains_key(&group) {
                groups.insert(group, 1.0 / (options.rrf_k + groups.len() as f64 + 1.0));
            }
        }
        for (terms, polarity) in [(prefer, 1.0), (avoid, -1.0)] {
            for (index, term) in terms.iter().enumerate() {
                let lane = format!("preference:{polarity}:{index}");
                let hits = self.lexical(
                    term,
                    allowed,
                    options.group_scan_limit,
                    options.candidates_per_group,
                    options,
                    &lane,
                    cancel,
                )?;
                let mut seen = BTreeSet::new();
                for (row, _) in hits {
                    let group = self.inner.candidates.group_rows[row as usize];
                    if seen.insert(group) {
                        if let Some(score) = groups.get_mut(&group) {
                            *score += polarity * options.preference_weight
                                / (options.rrf_k + seen.len() as f64);
                        }
                    }
                }
            }
        }
        let mut result = base.clone();
        result.score_kind = "preference_rrf".into();
        for hit in &mut result.hits {
            let row = hit["row"].as_u64().unwrap() as usize;
            hit["ranking_score"] = json!(groups[&self.inner.candidates.group_rows[row]]);
            hit["quality_score"] = Value::Null;
        }
        result.hits.sort_by(|a, b| {
            b["ranking_score"]
                .as_f64()
                .unwrap()
                .total_cmp(&a["ranking_score"].as_f64().unwrap())
                .then(a["row"].as_u64().cmp(&b["row"].as_u64()))
        });
        result.diagnostics["preferences"] = json!({"prefer":prefer,"avoid":avoid,
            "weight":options.preference_weight,"base_score_kind":base.score_kind});
        Ok(result)
    }

    pub(super) fn limit_ranked_groups(&self, ranked: &mut RankedCandidates, limit: Option<usize>) {
        let Some(limit) = limit else {
            return;
        };
        let mut groups = BTreeSet::new();
        let before = ranked.hits.len();
        ranked.hits.retain(|hit| {
            let row = hit["row"].as_u64().unwrap() as usize;
            let group = self.inner.candidates.group_rows[row];
            if !groups.contains(&group) && groups.len() >= limit {
                return false;
            }
            groups.insert(group);
            true
        });
        ranked.diagnostics["group_limit"] =
            json!({"limit":limit,"discarded_candidates":before-ranked.hits.len()});
    }

    fn ranked_shell(&self, options: &Options, local_vector: Option<Vec<f32>>) -> RankedCandidates {
        RankedCandidates {
            hits: Vec::new(),
            channel_scores: BTreeMap::new(),
            channel_ranks: BTreeMap::new(),
            offered_candidates: 0,
            diagnostics: json!({}),
            thinking: Value::Null,
            score_kind: String::new(),
            wormhole_rrf_active: false,
            used_lanes: Vec::new(),
            skipped: Vec::new(),
            started: std::time::Instant::now(),
            readout_max_union_candidates: options.readout.max_union_candidates.max(1),
            local_vector,
        }
    }

    pub(super) fn rank_lane(
        &self,
        candidates: &CandidateSet,
        lane: &str,
        group_limit: Option<usize>,
        options: &Options,
        local_vector: Option<Vec<f32>>,
    ) -> Result<RankedCandidates> {
        self.validate_candidates(candidates)?;
        let mut ranked = self.ranked_shell(options, local_vector);
        ranked.score_kind = match lane {
            "bm25" => "bm25",
            "query_knn" | "denoised_field_knn" | "local_field_knn" | "transfer_field_knn" => {
                "cosine"
            }
            _ => "lane_score",
        }
        .into();
        ranked.used_lanes.push(lane.into());
        ranked.offered_candidates = candidates.offered_candidates;
        let mut selected = candidates
            .entries
            .iter()
            .filter_map(|entry| entry.lanes.get(lane).map(|score| (entry.row, score)))
            .collect::<Vec<_>>();
        selected.sort_by(|a, b| a.1.rank.cmp(&b.1.rank).then(a.0.cmp(&b.0)));
        let mut groups = BTreeSet::new();
        for (row, score) in selected {
            let group = self.inner.candidates.group_rows[row as usize];
            if options.grouping == "document"
                && !groups.contains(&group)
                && group_limit.is_some_and(|limit| groups.len() >= limit)
            {
                continue;
            }
            groups.insert(group);
            ranked
                .channel_scores
                .entry(row)
                .or_default()
                .insert(lane.into(), score.score);
            ranked
                .channel_ranks
                .entry(row)
                .or_default()
                .insert(lane.into(), score.rank);
            ranked
                .hits
                .push(json!({"row":row,"quality_score":score.score,
                "ranking_score":score.score,"channels":[lane]}));
        }
        Ok(ranked)
    }

    pub(super) fn fuse_rankings(
        &self,
        inputs: &[(&RankedCandidates, f64)],
        k: f64,
        options: &Options,
        local_vector: Option<Vec<f32>>,
    ) -> Result<RankedCandidates> {
        ensure!(
            k.is_finite() && k > 0.0 && inputs.iter().all(|(_, w)| w.is_finite() && *w >= 0.0),
            "Invalid rank fusion parameters"
        );
        let mut ranked = self.ranked_shell(options, local_vector);
        ranked.score_kind = "rrf".into();
        // For document grouping, each input contributes once per group even
        // when different lanes recalled different passages of that document.
        let mut scores = BTreeMap::<u64, (f64, BTreeSet<String>, BTreeSet<u64>)>::new();
        for (input, weight) in inputs {
            for lane in &input.used_lanes {
                ranked.used_lanes.push(lane.clone());
            }
            let mut seen = BTreeSet::new();
            for hit in &input.hits {
                let row = hit["row"].as_u64().context("Ranked row missing")?;
                let identity = if options.grouping == "document" {
                    self.inner.candidates.group_rows[row as usize]
                } else {
                    row
                };
                let first = seen.insert(identity);
                let entry = scores.entry(identity).or_default();
                if first {
                    entry.0 += weight / (k + seen.len() as f64);
                }
                entry.1.extend(input.used_lanes.iter().cloned());
                entry.2.insert(row);
            }
            for (row, lanes) in &input.channel_scores {
                ranked
                    .channel_scores
                    .entry(*row)
                    .or_default()
                    .extend(lanes.clone());
            }
            for (row, lanes) in &input.channel_ranks {
                ranked
                    .channel_ranks
                    .entry(*row)
                    .or_default()
                    .extend(lanes.clone());
            }
        }
        ranked.offered_candidates = scores.values().map(|(_, _, rows)| rows.len()).sum();
        ranked.hits = scores
            .into_iter()
            .flat_map(|(_, (score, lanes, rows))| {
                rows.into_iter().map(move |row| {
                    json!({"row":row,"quality_score":null,
                "ranking_score":score,"channels":lanes})
                })
            })
            .collect();
        ranked.hits.sort_by(|a, b| {
            b["ranking_score"]
                .as_f64()
                .unwrap()
                .total_cmp(&a["ranking_score"].as_f64().unwrap())
                .then(a["row"].as_u64().cmp(&b["row"].as_u64()))
        });
        ranked.diagnostics["fusion"] = json!({"policy":"rrf","k":k,"weights":inputs.iter().map(|(_,w)| *w).collect::<Vec<_>>()});
        Ok(ranked)
    }

    pub(super) fn rank_context(
        &self,
        base: &RankedCandidates,
        candidates: &CandidateSet,
        options: &Options,
    ) -> Result<RankedCandidates> {
        let mut branches = Vec::new();
        for fragment in &options.context_lexical {
            ensure!(
                fragment.contribution.is_finite() && fragment.contribution >= 0.0,
                "Invalid lexical context contribution"
            );
            if fragment.contribution == 0.0 || fragment.text.trim().is_empty() {
                continue;
            }
            let lane = format!("context:{}", fragment.key);
            let ranked =
                self.rank_lane(candidates, &lane, None, options, base.local_vector.clone())?;
            if !ranked.hits.is_empty() {
                branches.push((ranked, fragment.contribution));
            }
        }
        if branches.is_empty() {
            return Ok(base.clone());
        }
        let mut inputs = vec![(base, 1.0)];
        inputs.extend(branches.iter().map(|(ranked, weight)| (ranked, *weight)));
        let mut result =
            self.fuse_rankings(&inputs, options.rrf_k, options, base.local_vector.clone())?;
        result.score_kind = "context_rrf".into();
        result.diagnostics["context"] = json!({
            "base_score_kind": base.score_kind,
            "branches": options.context_lexical.iter().map(|item|
                json!({"key":item.key,"contribution":item.contribution})).collect::<Vec<_>>()
        });
        Ok(result)
    }

    pub(super) fn river_readout(
        &self,
        observation: &QueryObservation,
        pool: &CalibratedPool,
        allowed: &BTreeSet<u64>,
        options: &Options,
        cancel: &CancellationToken,
        local_vector: Option<Vec<f32>>,
    ) -> Result<RankedCandidates> {
        self.validate_observation(observation)?;
        ensure!(
            Arc::ptr_eq(&observation.output, &pool.observation),
            "Calibration belongs to a different observation"
        );
        ensure!(
            pool.candidates
                .entries
                .iter()
                .all(|entry| allowed.contains(&entry.row)),
            "Calibrated pool exceeds the allowed scope"
        );
        let assets = self.inner.assets.as_ref().context("VCP assets required")?;
        let result = readout(
            assets,
            &observation.output,
            ReadoutRequest {
                top_k: pool.candidates.entries.len().max(1),
                include_trace: options.include_trace,
                pool: pool.prepared.clone(),
                allowed_file_ids: None,
                config: options.readout.clone(),
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
        let mut ranked = self.ranked_shell(options, local_vector);
        ranked.score_kind = result.score_kind().into();
        ranked.wormhole_rrf_active = result.wormhole_rrf_active;
        ranked.diagnostics = json!({"readout":result.diagnostics,"parameters":result.parameters,
            "calibration":{"bm25_reference":pool.prepared.bm25_reference,
                "population":pool.population,"union_offered":pool.candidates.offered_candidates,
                "retained":pool.candidates.entries.len()},
            "observation":if options.include_trace {json!({"metadata":observation.output.metadata,"sense":observation.output.observation})} else {Value::Null}});
        ranked.offered_candidates = pool.candidates.offered_candidates;
        self.add_pool_facts(&mut ranked, &pool.candidates);
        ranked.used_lanes.push("rivermemo".into());
        for hit in result.results {
            let row = (hit.id - 1) as u64;
            let entry = pool
                .candidates
                .entries
                .iter()
                .find(|entry| entry.row == row)
                .context("Readout returned a row outside its calibrated pool")?;
            let trace = if options.include_trace {
                let mut trace = serde_json::to_value(&hit)?;
                if let Some(features) = trace.as_object_mut().unwrap().remove("candidateSources") {
                    trace["readoutFeatureSources"] = features;
                }
                trace
            } else {
                Value::Null
            };
            ranked.hits.push(json!({"row":row,"quality_score":hit.score,"ranking_score":hit.ranking_score,
                "channels":entry.lanes.keys().collect::<Vec<_>>(),"chosen_path":hit.chosen_path,"paths":hit.available_paths,"trace":trace}));
        }
        Ok(ranked)
    }

    pub(super) fn dtsc_readout(
        &self,
        observation: &QueryObservation,
        candidates: &CandidateSet,
        allowed: &BTreeSet<u64>,
        options: &Options,
        cancel: &CancellationToken,
        local_vector: Option<Vec<f32>>,
    ) -> Result<RankedCandidates> {
        self.validate_observation(observation)?;
        self.validate_candidates(candidates)?;
        let output = &observation.output;
        let assets = self.inner.assets.as_ref().context("VCP assets required")?;
        let graph = assets.graph();
        let edges = graph
            .node_ids
            .iter()
            .enumerate()
            .flat_map(|(row, id)| {
                (graph.row_offsets[row]..graph.row_offsets[row + 1])
                    .map(move |i| (*id, graph.node_ids[graph.targets[i]], graph.weights[i]))
            })
            .collect();
        let metadata = &output.metadata;
        let input = ReferenceDtscInput {
            dimension: assets.dimension(),
            nodes: output
                .observation
                .nodes
                .iter()
                .map(|n| ReferenceDtscNode {
                    id: n.id,
                    energy: n.energy,
                    normalized_energy: n.normalized_energy,
                    source_type: n.source_type.clone(),
                })
                .collect(),
            source_field: output.observation.source_field.clone(),
            tag_vectors: assets
                .tag_vectors()
                .iter()
                .map(|(id, v)| (*id, v.to_vec()))
                .collect(),
            inbound: graph.inbound.iter().map(|(id, v)| (*id, *v)).collect(),
            anchor_gain: graph.anchor_gain.iter().map(|(id, v)| (*id, *v)).collect(),
            edges,
            candidates: candidates
                .entries
                .iter()
                .filter(|entry| allowed.contains(&entry.row))
                .map(|entry| {
                    let data = &assets.candidates()[&(entry.row as i64 + 1)];
                    let file = &assets.files()[&data.file_id];
                    let score = data
                        .vector
                        .iter()
                        .zip(&output.original_query_vector)
                        .map(|(a, b)| f64::from(*a) * f64::from(*b))
                        .sum();
                    ReferenceDtscCandidate {
                        curve: ReferenceCurve {
                            id: entry.row as i64 + 1,
                            chunk_vector: data.vector.to_vec(),
                            tags: file
                                .tags
                                .iter()
                                .filter(|t| t.position > 0)
                                .map(|t| ReferenceCurveTag {
                                    id: t.tag_id,
                                    vector: assets.tag_vectors()[&t.tag_id].to_vec(),
                                    position: t.position,
                                })
                                .collect(),
                        },
                        score,
                    }
                })
                .collect(),
            original_vector: output.original_query_vector.clone(),
            enhanced_vector: output.enhanced_vector.clone(),
            query_geometry_state: serde_json::from_value(
                json!({"epa":metadata["epa"],"pyramid":metadata["pyramid"]["features"]}),
            )
            .map_err(|e| failure("wave.dtsc_geometry_invalid", e))?,
            config: options.dtsc.clone(),
            top_k: 0,
        };
        cancel.check()?;
        let result = reference_dtsc(&input).map_err(|e| failure("wave.dtsc_failed", e))?;
        let mut ranked = self.ranked_shell(options, local_vector);
        ranked.score_kind = "dtsc".into();
        ranked.diagnostics = serde_json::to_value(result.diagnostics)?;
        ranked.offered_candidates = candidates.offered_candidates;
        self.add_pool_facts(&mut ranked, candidates);
        ranked.used_lanes.push("dtsc".into());
        let mut hits = result.results;
        hits.sort_by(|a, b| b.score.total_cmp(&a.score).then(a.id.cmp(&b.id)));
        for hit in hits {
            let row = (hit.id - 1) as u64;
            let entry = candidates
                .entries
                .iter()
                .find(|entry| entry.row == row)
                .context("DTSC returned an unknown row")?;
            ranked.hits.push(json!({"row":row,"quality_score":hit.score,"ranking_score":hit.score,
                "channels":entry.lanes.keys().collect::<Vec<_>>(),"trace":if options.include_trace {serde_json::to_value(hit)?}else{Value::Null}}));
        }
        Ok(ranked)
    }

    fn add_pool_facts(&self, ranked: &mut RankedCandidates, pool: &CandidateSet) {
        for entry in &pool.entries {
            for (name, lane) in &entry.lanes {
                ranked
                    .channel_scores
                    .entry(entry.row)
                    .or_default()
                    .insert(name.clone(), lane.score);
                ranked
                    .channel_ranks
                    .entry(entry.row)
                    .or_default()
                    .insert(name.clone(), lane.rank);
                ranked.used_lanes.push(name.clone());
            }
        }
    }
}
