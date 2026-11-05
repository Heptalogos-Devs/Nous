//! One observation feeds candidate recall, ranked readout and grounded Thinking.
mod recall;
use crate::{
    CancellationToken, failure,
    snapshot::{Snapshot, slice, unit},
};
use anyhow::{Context, Result, ensure};
use nous_vcp::{
    config::QueryControls,
    dtsc::*,
    pipeline::{GhostTagInput, PipelineConfig, PipelineInput, PipelineOutput, run_pipeline},
    river::{CandidateInput, NativeConfig, PreparedPool, ReadoutRequest, prepare_pool, readout},
    thinking::{Stage, ThinkingError, ThinkingInput, plan},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

/// An ordered, snapshot-bound pool. Readout never runs recall for this value.
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CandidatePool {
    #[serde(default = "candidate_rank_scope")]
    pub rank_scope: String,
    pub identity: Value,
    pub entries: Vec<PoolEntry>,
    #[serde(default)]
    pub calibration: Option<PreparedPool>,
    #[serde(default)]
    pub offered_candidates: usize,
    #[serde(default)]
    pub lane_coverage: BTreeMap<String, crate::grouping::LaneCoverage>,
}
fn candidate_rank_scope() -> String {
    "candidate_lane".into()
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PoolEntry {
    pub row: u64,
    pub lanes: BTreeMap<String, LaneScore>,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LaneScore {
    pub score: f64,
    pub rank: usize,
}
#[derive(Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Options {
    pub grouping: String,
    pub passages_per_group: usize,
    pub group_scan_limit: usize,
    #[serde(skip)]
    pub lane_coverage: std::sync::Mutex<BTreeMap<String, crate::grouping::LaneCoverage>>,
    pub query_space_signature: Option<String>,
    pub top_k: usize,
    pub rrf_k: f64,
    pub include_trace: bool,
    pub core_rows: Vec<u64>,
    pub ghosts: Vec<Ghost>,
    pub pipeline: PipelineConfig,
    pub readout: NativeConfig,
    pub dtsc: ReferenceDtscConfig,
    pub thinking_stages: Vec<ThinkingStage>,
    pub thinking_min_closure: f64,
    pub dedup_threshold: Option<f64>,
    pub include_counterevidence: bool,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Ghost {
    pub name: String,
    #[serde(default)]
    pub is_core: bool,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ThinkingStage {
    pub name: String,
    pub candidate_rows: Vec<u64>,
    pub top_k: usize,
}
impl Default for Options {
    fn default() -> Self {
        let controls = QueryControls::default();
        Self {
            query_space_signature: None,
            grouping: "candidate".into(),
            passages_per_group: 1,
            group_scan_limit: controls.group_scan_limit,
            lane_coverage: Default::default(),
            top_k: controls.top_k,
            rrf_k: controls.rrf_k,
            include_trace: false,
            core_rows: vec![],
            ghosts: vec![],
            pipeline: Default::default(),
            readout: Default::default(),
            dtsc: Default::default(),
            thinking_stages: vec![],
            thinking_min_closure: controls.thinking_min_closure,
            dedup_threshold: None,
            include_counterevidence: true,
        }
    }
}
impl Options {
    fn validate_grouping(&self) -> Result<()> {
        ensure!(
            matches!(self.grouping.as_str(), "candidate" | "document")
                && self.passages_per_group > 0,
            "Invalid result grouping"
        );
        ensure!(
            (1..=1_000_000).contains(&self.group_scan_limit),
            "group_scan_limit must be in 1..1000000"
        );
        Ok(())
    }
    fn lane_limit(&self, lane: &str) -> usize {
        match lane {
            "query_knn" => self.readout.query_k,
            "denoised_field_knn" => self.readout.denoised_k,
            "local_field_knn" => self.readout.local_field_k,
            "transfer_field_knn" => self.readout.transfer_field_k,
            "bm25" => self.readout.bm25_k,
            "direct_anchor" => self.readout.anchor_k,
            _ => 0,
        }
    }
}
pub struct QueryObservation {
    pub output: Arc<PipelineOutput>,
    pub identity: Value,
    pub text: String,
    pub core_rows: Vec<u64>,
    pub records_enabled: bool,
    pub diagnostics_json: String,
}
pub struct NativeQueryResult {
    pub rows: Vec<u64>,
    pub quality: Vec<f64>,
    pub ranking: Vec<f64>,
    pub result_json: String,
}

impl Snapshot {
    fn validate_query_space(&self, o: &Options) -> Result<()> {
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
    pub fn validate_pool(&self, pool: &CandidatePool) -> Result<()> {
        ensure!(
            matches!(pool.rank_scope.as_str(), "candidate_lane" | "grouped_lane"),
            "Invalid candidate pool rank_scope"
        );
        if pool.identity != self.inner.input.identity {
            return Err(crate::Failure {
                code: "wave.candidate_pool_mismatch",
                message: "Candidate pool belongs to another snapshot".into(),
                details: json!({"field":"identity","expected":self.inner.input.identity,"actual":pool.identity}),
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
        for entry in &pool.entries {
            if entry.row >= self.inner.candidates.keys.len() as u64 {
                return Err(invalid(
                    "entries.row",
                    "Candidate pool row is outside this snapshot",
                    json!({"row":entry.row,"candidate_count":self.inner.candidates.keys.len()}),
                ));
            }
            if !rows.insert(entry.row) {
                return Err(invalid(
                    "entries.row",
                    "Candidate pool contains a duplicate row",
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
                if !lane.score.is_finite() {
                    return Err(invalid(
                        "entries.lanes.score",
                        "Candidate lane score must be finite",
                        json!({"row":entry.row,"lane":name,"score":lane.score}),
                    ));
                }
            }
        }
        if let Some(calibration) = &pool.calibration {
            if !calibration.bm25_reference.is_finite() || calibration.bm25_reference < 0.0 {
                return Err(invalid(
                    "calibration.bm25_reference",
                    "BM25 reference must be finite and nonnegative",
                    json!({"value":calibration.bm25_reference}),
                ));
            }
            let mut selected = BTreeSet::new();
            let mut ranks = BTreeSet::new();
            for item in &calibration.selection {
                let row = (item.id > 0).then(|| (item.id - 1) as u64);
                if !row.is_some_and(|row| rows.contains(&row)) || !selected.insert(item.id) {
                    return Err(invalid(
                        "calibration.selection.id",
                        "Calibrated selection must identify unique pool rows",
                        json!({"id":item.id,"row":row}),
                    ));
                }
                if !item.union_score.is_finite() || !(0.0..=1.0).contains(&item.union_score) {
                    return Err(invalid(
                        "calibration.selection.union_score",
                        "Union score must be finite and within [0,1]",
                        json!({"row":row,"value":item.union_score}),
                    ));
                }
                if !item.anchor_score.is_finite() || !(0.0..=1.0).contains(&item.anchor_score) {
                    return Err(invalid(
                        "calibration.selection.anchor_score",
                        "Anchor score must be finite and within [0,1]",
                        json!({"row":row,"value":item.anchor_score}),
                    ));
                }
                if item.union_rank == 0
                    || item.union_rank > rows.len()
                    || !ranks.insert(item.union_rank)
                {
                    return Err(invalid(
                        "calibration.selection.union_rank",
                        "Calibrated ranks must uniquely order the pool",
                        json!({"row":row,"rank":item.union_rank,"candidate_count":rows.len()}),
                    ));
                }
            }
            let mut candidates = BTreeSet::new();
            for item in &calibration.candidates {
                let row = (item.id > 0).then(|| (item.id - 1) as u64);
                if !row.is_some_and(|row| rows.contains(&row)) || !candidates.insert(item.id) {
                    return Err(invalid(
                        "calibration.candidates.id",
                        "Calibrated candidates must identify unique pool rows",
                        json!({"id":item.id,"row":row}),
                    ));
                }
                for (field, value) in [
                    ("score", item.score),
                    ("hybrid_score", item.hybrid_score),
                    ("vector_score", item.vector_score),
                    ("bm25_score", item.bm25_score),
                    ("time_score", item.time_score),
                    ("anchor_score", item.anchor_score),
                ] {
                    if !value.is_finite() {
                        return Err(invalid(
                            &format!("calibration.candidates.{field}"),
                            "Calibrated score must be finite",
                            json!({"row":row,"value":value}),
                        ));
                    }
                }
            }
            if selected.len() != rows.len() || candidates.len() != rows.len() {
                return Err(invalid(
                    "calibration",
                    "Calibration must cover the complete candidate pool",
                    json!({"pool_rows":rows,"selected_ids":selected,"candidate_ids":candidates}),
                ));
            }
        }
        Ok(())
    }
    fn prepare_pool(
        &self,
        observation: &QueryObservation,
        pool: &CandidatePool,
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
                    .chain(candidate_pool.entries.iter()
                        .filter(|entry| allowed.contains(&entry.row))
                        .flat_map(|entry| entry.lanes.keys()))
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
                    let dtsc_input=ReferenceDtscInput{dimension:assets.dimension(),nodes:output.observation.nodes.iter().map(|n|ReferenceDtscNode{id:n.id,energy:n.energy,normalized_energy:n.normalized_energy,source_type:n.source_type.clone()}).collect(),source_field:output.observation.source_field.clone(),tag_vectors:assets.tag_vectors().iter().map(|(id,v)|(*id,v.to_vec())).collect(),inbound:graph.inbound.iter().map(|(id,v)|(*id,*v)).collect(),anchor_gain:graph.anchor_gain.iter().map(|(id,v)|(*id,*v)).collect(),edges,candidates:pool.iter().map(|(row,(score,_,_))|{let data=&assets.candidates()[&(*row as i64+1)];let file=&assets.files()[&data.file_id];ReferenceDtscCandidate{curve:ReferenceCurve{id:*row as i64+1,chunk_vector:data.vector.to_vec(),tags:file.tags.iter().filter(|t|t.position>0).map(|t|ReferenceCurveTag{id:t.tag_id,vector:assets.tag_vectors()[&t.tag_id].to_vec(),position:t.position}).collect()},score:*score}}).collect(),original_vector:output.original_query_vector.clone(),enhanced_vector:output.enhanced_vector.clone(),query_geometry_state:serde_json::from_value(json!({"epa":metadata["epa"],"pyramid":metadata["pyramid"]["features"]})).map_err(|e|failure("wave.dtsc_geometry_invalid",e))?,config:o.dtsc,top_k:0};
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
        cancel.check()?;
        if o.grouping == "document"
            && provided_pool.is_none()
            && hits.len() > readout_max_union_candidates
        {
            offered_candidates = offered_candidates.max(hits.len());
            let ordered = hits
                .iter()
                .map(|hit| hit["row"].as_u64().unwrap())
                .collect::<Vec<_>>();
            let selected = crate::grouping::fair_rows(
                &ordered,
                &self.inner.candidates.group_rows,
                readout_max_union_candidates,
                o.passages_per_group,
            );
            hits.retain(|hit| selected.contains(&hit["row"].as_u64().unwrap()));
        }
        let ranked_candidates = hits.len();
        if offered_candidates == 0 {
            offered_candidates = ranked_candidates;
        }
        let dedup_vectors = if o.grouping == "candidate" && o.dedup_threshold.is_some() {
            self.inner.candidate_units.as_ref().map(|vectors| {
                hits.iter()
                    .map(|hit| vectors[hit["row"].as_u64().unwrap() as usize].as_ref())
                    .collect::<Vec<_>>()
            })
        } else {
            None
        };
        let mut representatives = crate::ordered_dedup::OrderedDedup::default();
        let mut kept = Vec::<Value>::new();
        let threshold = o.dedup_threshold;
        if let Some(v) = threshold {
            ensure!(
                v.is_finite() && (-1.0..=1.0).contains(&v),
                "Dedup threshold invalid"
            );
        }
        for (hit_index, mut hit) in hits.into_iter().enumerate() {
            cancel.check()?;
            let row = hit["row"].as_u64().unwrap() as usize;
            let mut folded = false;
            if let (Some(threshold), Some(vectors)) = (threshold, dedup_vectors.as_ref()) {
                let vector = &vectors[hit_index];
                if let Some((index, cosine)) = representatives.first_match(vector, threshold) {
                    kept[index]["folded_rows"]
                        .as_array_mut()
                        .unwrap()
                        .push(json!({"row":row,"similarity":cosine}));
                    folded = true;
                } else {
                    representatives.push(vector);
                }
            }
            if folded {
                continue;
            }
            let concepts = slice(
                &self.inner.input.candidates.membership_indptr,
                &self.inner.input.candidates.membership_indices,
                row,
            );
            hit["concept_rows"] = json!(concepts);
            hit["evidence_roots"] = json!(slice(
                &self.inner.input.candidates.root_indptr,
                &self.inner.input.candidates.root_indices,
                row
            ));
            if o.include_trace {
                if let Some(assets) = &self.inner.assets {
                    let graph = assets.graph();
                    let roots = slice(
                        &self.inner.input.candidates.root_indptr,
                        &self.inner.input.candidates.root_indices,
                        row,
                    )
                    .iter()
                    .map(|r| *r as i64 + 1)
                    .collect::<Vec<_>>();
                    let mut support = Vec::new();
                    for concept in concepts {
                        let source = *concept as i64 + 1;
                        if let Some(index) = graph.node_index.get(&source) {
                            for cursor in graph.row_offsets[*index]..graph.row_offsets[*index + 1] {
                                let target = graph.node_ids[graph.targets[cursor]];
                                if concepts.contains(&((target - 1) as u64)) {
                                    support.push(json!({"source":source-1,"target":target-1,"independent_support_fraction":graph.independent_support_fraction(source,target,&roots),"support_weight":graph.independent_fraction_excluding(source,target,&roots)}));
                                }
                            }
                        }
                    }
                    hit["independent_support"] = json!(support);
                }
            }
            hit["folded_rows"] = json!([]);
            hit["counterevidence_links"] = json!([]);
            hit["channel_ranks"] = json!(channel_ranks.get(&(row as u64)));
            hit["channel_scores"] = json!(channel_scores.get(&(row as u64)));
            hit["score_kind"] = json!(score_kind);
            hit["rank"] = json!(kept.len() + 1);
            if o.include_counterevidence {
                let targets = concepts
                    .iter()
                    .flat_map(|c| self.inner.negative_neighbors.get(c).into_iter().flatten())
                    .copied()
                    .collect::<BTreeSet<_>>();
                let rows = targets
                    .iter()
                    .flat_map(|t| self.inner.concept_candidates.get(t).into_iter().flatten())
                    .filter(|r| **r != row as u64 && allowed.contains(r))
                    .copied()
                    .collect::<BTreeSet<_>>();
                let links = rows
                    .into_iter()
                    .map(|related| {
                        let related_concepts = slice(
                            &self.inner.input.candidates.membership_indptr,
                            &self.inner.input.candidates.membership_indices,
                            related as usize,
                        );
                        let associations = concepts
                            .iter()
                            .flat_map(|source| {
                                related_concepts.iter().flat_map(move |target| {
                                    self.inner
                                        .negative_relations
                                        .get(&(*source, *target))
                                        .into_iter()
                                        .flatten()
                                })
                            })
                            .copied()
                            .collect::<BTreeSet<_>>();
                        json!({"row":related,"associations":associations})
                    })
                    .collect::<Vec<_>>();
                hit["counterevidence_links"] = json!(links);
            }
            kept.push(hit);
        }
        let before = kept.len();
        let all_groups = kept
            .iter()
            .map(|hit| self.inner.candidates.group_rows[hit["row"].as_u64().unwrap() as usize])
            .collect::<BTreeSet<_>>();
        let mut groups = Vec::<Value>::new();
        if o.grouping == "document" {
            // Keep the final readout order: the first candidate establishes each document rank.
            let mut group_indices = BTreeMap::<u64, usize>::new();
            let mut selected = Vec::new();
            for (index, mut hit) in kept.into_iter().enumerate() {
                let row = hit["row"].as_u64().unwrap();
                let group = self.inner.candidates.group_rows[row as usize];
                if !group_indices.contains_key(&group) {
                    if groups.len() >= o.top_k {
                        continue;
                    }
                    group_indices.insert(group, groups.len());
                    groups.push(json!({"group_row":group,"group_key":self.inner.candidates.group_keys[group as usize],"group_rank":groups.len()+1,"representative_row":row,"passage_rows":[]}));
                }
                let group_index = group_indices[&group];
                let passages = groups[group_index]["passage_rows"].as_array_mut().unwrap();
                if passages.len() >= o.passages_per_group {
                    continue;
                }
                passages.push(json!(row));
                hit["candidate_rank"] = json!(index + 1);
                hit["group_rank"] = json!(group_index + 1);
                hit["group_key"] = json!(self.inner.candidates.group_keys[group as usize]);
                selected.push(hit);
            }
            kept = selected;
        } else {
            kept.truncate(o.top_k);
        }
        let lane_coverage = o.lane_coverage.lock().unwrap().clone();
        let recall_exhausted =
            !lane_coverage.is_empty() && lane_coverage.values().all(|lane| lane.recall_exhausted);
        let recall_truncated = lane_coverage.values().any(|lane| lane.truncated);
        let pool_truncated = offered_candidates > readout_max_union_candidates;
        let top_k_truncated = if o.grouping == "document" {
            all_groups.len() > o.top_k
        } else {
            before > o.top_k
        };
        let eligible_groups_known = if lane_coverage.len() == 1 {
            lane_coverage.values().next().unwrap().eligible_groups_known
        } else if recall_exhausted && !recall_truncated && !pool_truncated {
            Some(all_groups.len())
        } else {
            None
        };
        let coverage_reason = if pool_truncated {
            Some("max_union_candidates".to_string())
        } else if recall_truncated {
            lane_coverage.values().find_map(|lane| lane.reason.clone())
        } else if top_k_truncated {
            Some("top_k".to_string())
        } else {
            None
        };
        let rows = kept.iter().map(|v| v["row"].as_u64().unwrap()).collect();
        let quality = kept
            .iter()
            .map(|v| v["quality_score"].as_f64().unwrap_or(f64::NAN))
            .collect();
        let ranking = kept
            .iter()
            .map(|v| v["ranking_score"].as_f64().unwrap())
            .collect();
        let mut reported_lanes = BTreeSet::new();
        used_lanes.retain(|lane| reported_lanes.insert(lane.clone()));
        diagnostics["total_ms"] = json!(started.elapsed().as_secs_f64() * 1000.0);
        diagnostics["max_union_candidates"] = json!(readout_max_union_candidates);
        diagnostics["readout_max_union_candidates"] = json!(readout_max_union_candidates);
        diagnostics["input_scale"] = json!({"allowed_candidates":allowed.len(),"concepts":self.inner.concepts.keys.len(),"max_union_candidates":readout_max_union_candidates});
        Ok(NativeQueryResult{rows,quality,ranking,result_json:json!({"mode":mode,"score_kind":score_kind,"wormhole_rrf_active":wormhole_rrf_active,"used_lanes":used_lanes,"skipped_lanes":skipped,"hits":kept,"groups":groups,"grouping":o.grouping,"rank_scope":provided_pool.map(|pool|pool.rank_scope.as_str()).unwrap_or(if o.grouping == "document" {"grouped_lane"} else {"candidate_lane"}),"thinking":thinking,"algorithm_versions":{"vcp_upstream":"d5c019ebbac1f317c2938d655d2ce916e41b9164","pipeline":"memo.conservative-topology-v2.2","river":"rivermemo.conservative-topology-v2.2"},"coverage":{"requested_groups":if o.grouping=="document"{Some(o.top_k)}else{None},"returned_groups":groups.len(),"eligible_groups_known":eligible_groups_known,"allowed_groups":allowed.iter().map(|row|self.inner.candidates.group_rows[*row as usize]).collect::<BTreeSet<_>>().len(),"recall_exhausted":recall_exhausted,"truncated":recall_truncated||pool_truncated||top_k_truncated,"reason":coverage_reason,"lanes":lane_coverage,"lane_limit_unit":if o.grouping == "document" {"groups"}else{"candidates"},"group_scan_limit":o.group_scan_limit,"passages_per_group":o.passages_per_group,"candidates":before,"total_candidates":self.inner.candidates.keys.len(),"max_union_candidates":readout_max_union_candidates,"top_k":o.top_k,"offered_candidates":offered_candidates,"ranked_candidates":ranked_candidates,"unique_candidates":before,"candidate_limit_reached":offered_candidates>readout_max_union_candidates,"top_k_truncated":top_k_truncated},"diagnostics":if o.include_trace{diagnostics}else{Value::Null}}).to_string()})
    }
}
