//! One observation feeds candidate recall, ranked readout and grounded Thinking.
use crate::{
    engine::{slice, unit},
    *,
};
use anyhow::{Context, Result, ensure};
use nous_vcp::{
    config::QueryControls,
    dtsc::*,
    pipeline::{GhostTagInput, PipelineConfig, PipelineInput, PipelineOutput, run_pipeline},
    river::{CandidateInput, NativeConfig, ReadoutRequest, readout},
    thinking::{Stage, ThinkingInput, plan},
};
use numpy::{IntoPyArray, PyArray1, PyReadonlyArray1, PyReadonlyArray2, PyUntypedArrayMethods};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

#[derive(Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct Options {
    top_k: usize,
    candidate_budget: usize,
    rrf_k: f64,
    include_trace: bool,
    core_rows: Vec<u64>,
    ghosts: Vec<Ghost>,
    pipeline: PipelineConfig,
    readout: NativeConfig,
    dtsc: ReferenceDtscConfig,
    thinking_stages: Vec<ThinkingStage>,
    thinking_min_closure: f64,
    dedup_threshold: Option<f64>,
    include_counterevidence: bool,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Ghost {
    name: String,
    #[serde(default)]
    is_core: bool,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ThinkingStage {
    name: String,
    candidate_rows: Vec<u64>,
    top_k: usize,
}
impl Default for Options {
    fn default() -> Self {
        let controls = QueryControls::default();
        Self {
            top_k: controls.top_k,
            candidate_budget: controls.candidate_budget,
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
#[pyclass(frozen, module = "nous_native._core")]
pub struct QueryObservation {
    pub(crate) output: Arc<PipelineOutput>,
    identity: Value,
    text: String,
    #[pyo3(get)]
    diagnostics_json: String,
}
#[pyclass(frozen, module = "nous_native._core")]
pub struct NativeQueryResult {
    rows: Vec<u64>,
    quality: Vec<f64>,
    ranking: Vec<f64>,
    #[pyo3(get)]
    result_json: String,
}
#[pymethods]
impl NativeQueryResult {
    #[getter]
    fn row_ids<'py>(&self, py: Python<'py>) -> Bound<'py, PyArray1<u64>> {
        self.rows.clone().into_pyarray(py)
    }
    #[getter]
    fn quality_scores<'py>(&self, py: Python<'py>) -> Bound<'py, PyArray1<f64>> {
        self.quality.clone().into_pyarray(py)
    }
    #[getter]
    fn ranking_scores<'py>(&self, py: Python<'py>) -> Bound<'py, PyArray1<f64>> {
        self.ranking.clone().into_pyarray(py)
    }
}
fn options(raw: &str) -> PyResult<Options> {
    serde_json::from_str(raw).map_err(|e| native_error(failure("wave.invalid_input", e)))
}
fn array(v: Option<PyReadonlyArray1<'_, f32>>) -> PyResult<Option<Vec<f32>>> {
    v.map(|v| {
        v.as_slice()
            .map(|v| v.to_vec())
            .map_err(|e| native_error(failure("wave.invalid_input", e)))
    })
    .transpose()
}
fn ghosts(v: Option<PyReadonlyArray2<'_, f32>>) -> PyResult<Vec<Vec<f32>>> {
    v.map(|v| {
        let shape = v.shape();
        let width = shape[1];
        if width == 0 {
            return Err(native_error(failure(
                "wave.invalid_input",
                "Ghost vector dimension must be positive",
            )));
        }
        Ok(v.as_slice()
            .map_err(|e| native_error(failure("wave.invalid_input", e)))?
            .chunks(width)
            .map(|r| r.to_vec())
            .collect())
    })
    .unwrap_or(Ok(vec![]))
}
fn allowed(v: Option<PyReadonlyArray1<'_, u64>>, count: usize) -> PyResult<BTreeSet<u64>> {
    v.map(|v| {
        v.as_slice()
            .map(|v| v.iter().copied().collect())
            .map_err(|e| native_error(failure("wave.invalid_input", e)))
    })
    .unwrap_or(Ok((0..count as u64).collect()))
}
impl NativeSnapshot {
    fn execute<T: Send>(
        &self,
        py: Python<'_>,
        token: &CancellationToken,
        operation: impl FnOnce() -> Result<T> + Send,
    ) -> PyResult<T> {
        py.detach(|| {
            self.state.pool.install(|| {
                let _guard = self
                    .state
                    .active
                    .read()
                    .map_err(|_| failure("wave.closed", "Engine lifecycle poisoned"))?;
                ensure!(
                    !self.state.closed.load(Ordering::Acquire),
                    "Native engine closed"
                );
                token.check()?;
                operation()
            })
        })
        .map_err(native_error)
    }
    fn observation(
        &self,
        vector: &[f32],
        text: &str,
        o: &Options,
        ghost_vectors: Vec<Vec<f32>>,
        cancel: &CancellationToken,
    ) -> Result<QueryObservation> {
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
        let mut config: PipelineConfig =
            serde_json::from_value(serde_json::to_value(&o.pipeline)?)?;
        if !o.thinking_stages.is_empty() && config.spike_routing.max_transition_records == 0 {
            config.spike_routing.max_transition_records = usize::MAX;
        }
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
            diagnostics_json,
        })
    }
    fn validate_observation(&self, observation: &QueryObservation) -> Result<()> {
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
    fn lexical(
        &self,
        text: &str,
        allowed: &BTreeSet<u64>,
        limit: usize,
    ) -> Result<Vec<(u64, f64)>> {
        if text.trim().is_empty() {
            return Ok(vec![]);
        }
        let refs = allowed
            .iter()
            .map(|r| self.inner.candidates.keys[*r as usize].clone())
            .collect();
        let lookup = self
            .inner
            .candidates
            .keys
            .iter()
            .enumerate()
            .map(|(r, k)| (k, r as u64))
            .collect::<BTreeMap<_, _>>();
        Ok(self
            .inner
            .lexical
            .search_allowed(text, "snapshot", "snapshot", limit, &refs)?
            .hits
            .into_iter()
            .map(|h| (lookup[&h.id], f64::from(h.score)))
            .collect())
    }
    fn dense(
        &self,
        vector: &[f32],
        allowed: &BTreeSet<u64>,
        limit: usize,
    ) -> Result<Vec<(u64, f64)>> {
        let ann = self
            .inner
            .ann
            .as_ref()
            .ok_or_else(|| failure("wave.capability_missing", "Dense vectors unavailable"))?;
        let ids = allowed.iter().map(|r| *r as i64 + 1).collect();
        Ok(ann
            .search_candidates(vector, limit, Some(&ids))?
            .into_iter()
            .map(|h| ((h.id - 1) as u64, h.similarity))
            .collect())
    }
    fn thinking(
        &self,
        observation: &QueryObservation,
        stages: &[ThinkingStage],
        minimum: f64,
        allowed: &BTreeSet<u64>,
        cancel: &CancellationToken,
    ) -> Value {
        if stages.is_empty() {
            return Value::Null;
        }
        if cancel.is_cancelled() {
            return json!({"unavailable":"cancelled"});
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
        match plan(
            assets,
            &observation.output,
            input,
            Some(&|| cancel.is_cancelled()),
        ) {
            Ok(output) => {
                json!({"stages":output.stages.into_iter().map(|s|json!({"name":s.diary_name,"results":s.results.into_iter().map(|r|json!({"row":r.chunk_id-1,"score":r.score,"parents":r.parents.iter().map(|p|p-1).collect::<Vec<_>>(),"relation":r.relation,"route":r.route})).collect::<Vec<_>>()})).collect::<Vec<_>>()})
            }
            Err(reason) => json!({"unavailable":reason}),
        }
    }
    fn ranked(
        &self,
        mode: &str,
        text: &str,
        vector: Option<&[f32]>,
        allowed: &BTreeSet<u64>,
        o: Options,
        ghost_vectors: Vec<Vec<f32>>,
        provided: Option<&QueryObservation>,
        cancel: &CancellationToken,
    ) -> Result<NativeQueryResult> {
        QueryControls {
            top_k: o.top_k,
            candidate_budget: o.candidate_budget,
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
        let mut used_lanes = Vec::new();
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
                used_lanes.push("tagmemo");
                for (rank, (row, score)) in self
                    .dense(&output.enhanced_vector, allowed, o.candidate_budget)?
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
                let mut pool = BTreeMap::<u64, (f64, f64, Vec<String>)>::new();
                for (name, vector) in [
                    ("query_knn", &output.original_query_vector),
                    ("denoised_field_knn", &output.enhanced_vector),
                    ("local_field_knn", &output.local_vector),
                    ("transfer_field_knn", &output.transfer_vector),
                ] {
                    if vector.iter().any(|v| *v != 0.0) {
                        for (rank, (row, score)) in self
                            .dense(vector, allowed, o.candidate_budget)?
                            .into_iter()
                            .enumerate()
                        {
                            channel_ranks
                                .entry(row)
                                .or_default()
                                .insert(name.into(), rank + 1);
                            channel_scores
                                .entry(row)
                                .or_default()
                                .insert(name.into(), score);
                            let entry = pool.entry(row).or_default();
                            entry.0 = entry.0.max(score);
                            entry.2.push(name.into());
                        }
                    }
                }
                for (rank, (row, score)) in self
                    .lexical(&observation.text, allowed, o.candidate_budget)?
                    .into_iter()
                    .enumerate()
                {
                    channel_ranks
                        .entry(row)
                        .or_default()
                        .insert("bm25".into(), rank + 1);
                    let entry = pool.entry(row).or_default();
                    entry.1 = score;
                    channel_scores
                        .entry(row)
                        .or_default()
                        .insert("bm25".into(), score);
                    entry.2.push("bm25".into());
                }
                let cores = o.core_rows.iter().copied().collect::<BTreeSet<_>>();
                for row in allowed {
                    cancel.check()?;
                    if slice(
                        &self.inner.input.candidates.membership_indptr,
                        &self.inner.input.candidates.membership_indices,
                        *row as usize,
                    )
                    .iter()
                    .any(|r| cores.contains(r))
                    {
                        pool.entry(*row).or_default().2.push("direct_anchor".into());
                        channel_scores
                            .entry(*row)
                            .or_default()
                            .insert("direct_anchor".into(), 1.0);
                    }
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
                offered_candidates = pool.len();
                let candidates = pool
                    .iter()
                    .map(|(row, (score, bm25, lanes))| CandidateInput {
                        id: *row as i64 + 1,
                        score: *score,
                        hybrid_score: 0.0,
                        vector_score: *score,
                        bm25_score: *bm25,
                        time_score: 0.0,
                        anchor_score: if lanes.iter().any(|l| l == "direct_anchor") {
                            1.0
                        } else {
                            0.0
                        },
                    })
                    .collect();
                let assets = self.inner.assets.as_ref().unwrap();
                if mode == "dtsc" {
                    score_kind = "dtsc";
                    used_lanes.push("dtsc");
                    let graph = &assets.snapshot().graph;
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
                    let dtsc_input=ReferenceDtscInput{dimension:assets.dimension(),nodes:output.observation.nodes.iter().map(|n|ReferenceDtscNode{id:n.id,energy:n.energy,normalized_energy:n.normalized_energy,source_type:n.source_type.clone()}).collect(),source_field:output.observation.source_field.clone(),tag_vectors:assets.tag_vectors().iter().map(|(id,v)|(*id,v.to_vec())).collect(),inbound:graph.inbound.iter().map(|(id,v)|(*id,*v)).collect(),anchor_gain:graph.anchor_gain.iter().map(|(id,v)|(*id,*v)).collect(),edges,candidates:pool.iter().map(|(row,(score,_,_))|{let data=&assets.candidates()[&(*row as i64+1)];let file=&assets.files()[&data.file_id];ReferenceDtscCandidate{curve:ReferenceCurve{id:*row as i64+1,chunk_vector:data.vector.to_vec(),tags:file.tags.iter().filter(|t|t.position>0).map(|t|ReferenceCurveTag{id:t.tag_id,vector:assets.tag_vectors()[&t.tag_id].to_vec(),position:t.position}).collect()},score:*score}}).collect(),original_vector:output.original_query_vector.clone(),enhanced_vector:output.enhanced_vector.clone(),query_geometry_state:serde_json::from_value(json!({"epa":metadata["epa"],"pyramid":metadata["pyramid"]["features"]})).unwrap_or_default(),config:o.dtsc,top_k:0};
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
                    used_lanes.push("rivermemo");
                    let config = o.readout;
                    let result = readout(
                        assets,
                        output,
                        ReadoutRequest {
                            top_k: readout_max_union_candidates,
                            include_trace: o.include_trace,
                            candidates,
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
                        let channels = if hit.candidate_sources.is_empty() {
                            pool[&row].2.clone()
                        } else {
                            hit.candidate_sources.clone()
                        };
                        hits.push(json!({"row":row,"quality_score":hit.score,"ranking_score":hit.ranking_score,"channels":channels,"trace":if o.include_trace{serde_json::to_value(hit)?}else{Value::Null}}));
                    }
                }
            }
            thinking = self.thinking(
                observation,
                &o.thinking_stages,
                o.thinking_min_closure,
                allowed,
                cancel,
            );
        } else {
            match mode {
                "lexical" => {
                    score_kind = "bm25";
                    used_lanes.push("lexical");
                    for (rank, (row, score)) in self
                        .lexical(text, allowed, o.candidate_budget)?
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
                    used_lanes.push("dense");
                    for (rank, (row, score)) in self
                        .dense(
                            vector.context("Query vector required")?,
                            allowed,
                            o.candidate_budget,
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
                    let mut lanes =
                        vec![("lexical", self.lexical(text, allowed, o.candidate_budget)?)];
                    if self.inner.ann.is_some() {
                        lanes.push((
                            "dense",
                            self.dense(
                                vector.context("Query vector required")?,
                                allowed,
                                o.candidate_budget,
                            )?,
                        ));
                    } else {
                        skipped.push(json!({"lane":"dense","reason":"vectors unavailable"}));
                    }
                    skipped.push(
                        json!({"lane":"rivermemo","reason":"concept/fact assets unavailable"}),
                    );
                    for (lane, results) in lanes {
                        used_lanes.push(lane);
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
        let ranked_candidates = hits.len();
        if offered_candidates == 0 {
            offered_candidates = ranked_candidates;
        }
        let mut dedup_vectors = BTreeMap::new();
        if o.dedup_threshold.is_some() {
            if let Some(vectors) = &self.inner.input.candidates.vectors {
                for hit in &hits {
                    let row = hit["row"].as_u64().unwrap();
                    dedup_vectors.insert(row, unit(vectors.row(row as usize))?);
                }
            }
        }
        let mut kept = Vec::<Value>::new();
        let threshold = o.dedup_threshold;
        if let Some(v) = threshold {
            ensure!(
                v.is_finite() && (-1.0..=1.0).contains(&v),
                "Dedup threshold invalid"
            );
        }
        for mut hit in hits {
            cancel.check()?;
            let row = hit["row"].as_u64().unwrap() as usize;
            let mut folded = false;
            if let (Some(threshold), Some(vectors)) =
                (threshold, self.inner.input.candidates.vectors.as_ref())
            {
                let _ = vectors;
                let v = &dedup_vectors[&(row as u64)];
                for representative in &mut kept {
                    let other = representative["row"].as_u64().unwrap() as usize;
                    let other = &dedup_vectors[&(other as u64)];
                    let cosine = v
                        .iter()
                        .zip(other)
                        .map(|(a, b)| f64::from(*a) * f64::from(*b))
                        .sum::<f64>();
                    if cosine >= threshold {
                        representative["folded_rows"]
                            .as_array_mut()
                            .unwrap()
                            .push(json!({"row":row,"similarity":cosine}));
                        folded = true;
                        break;
                    }
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
            hit["folded_rows"] = json!([]);
            hit["counterevidence_rows"] = json!([]);
            hit["channel_ranks"] = json!(channel_ranks.get(&(row as u64)));
            hit["channel_scores"] = json!(channel_scores.get(&(row as u64)));
            hit["score_kind"] = json!(score_kind);
            hit["rank"] = json!(kept.len() + 1);
            if o.include_counterevidence {
                let assoc = &self.inner.input.associations;
                let mut targets = BTreeSet::new();
                for i in 0..assoc.sources.len() {
                    if assoc.polarities[i] >= 0 {
                        continue;
                    }
                    if concepts.contains(&assoc.sources[i]) {
                        targets.insert(assoc.targets[i]);
                    }
                    if !assoc.directed[i] && concepts.contains(&assoc.targets[i]) {
                        targets.insert(assoc.sources[i]);
                    }
                }
                let rows = allowed
                    .iter()
                    .filter(|r| {
                        **r != row as u64
                            && slice(
                                &self.inner.input.candidates.membership_indptr,
                                &self.inner.input.candidates.membership_indices,
                                **r as usize,
                            )
                            .iter()
                            .any(|t| targets.contains(t))
                    })
                    .copied()
                    .collect::<Vec<_>>();
                hit["counterevidence_rows"] = json!(rows);
            }
            kept.push(hit);
        }
        let before = kept.len();
        kept.truncate(o.top_k);
        let rows = kept.iter().map(|v| v["row"].as_u64().unwrap()).collect();
        let quality = kept
            .iter()
            .map(|v| v["quality_score"].as_f64().unwrap_or(f64::NAN))
            .collect();
        let ranking = kept
            .iter()
            .map(|v| v["ranking_score"].as_f64().unwrap())
            .collect();
        diagnostics["total_ms"] = json!(started.elapsed().as_secs_f64() * 1000.0);
        diagnostics["candidate_budget"] = json!(o.candidate_budget);
        diagnostics["readout_max_union_candidates"] = json!(readout_max_union_candidates);
        diagnostics["input_scale"] = json!({"allowed_candidates":allowed.len(),"concepts":self.inner.concepts.keys.len(),"candidate_budget":o.candidate_budget});
        Ok(NativeQueryResult{rows,quality,ranking,result_json:json!({"mode":mode,"score_kind":score_kind,"wormhole_rrf_active":wormhole_rrf_active,"used_lanes":used_lanes,"skipped_lanes":skipped,"hits":kept,"thinking":thinking,"algorithm_versions":{"vcp_upstream":"d5c019ebbac1f317c2938d655d2ce916e41b9164","pipeline":"memo.conservative-topology-v2.2","river":"rivermemo.conservative-topology-v2.2"},"coverage":{"candidates":before,"total_candidates":self.inner.candidates.keys.len(),"candidate_budget":o.candidate_budget,"top_k":o.top_k,"offered_candidates":offered_candidates,"ranked_candidates":ranked_candidates,"unique_candidates":before,"candidate_limit_reached":offered_candidates>o.candidate_budget || (ranked_candidates==o.candidate_budget&&allowed.len()>o.candidate_budget),"top_k_truncated":before>o.top_k},"diagnostics":if o.include_trace{diagnostics}else{Value::Null}}).to_string()})
    }
}
#[pymethods]
impl NativeSnapshot {
    #[getter]
    fn identity_json(&self) -> String {
        self.inner.input.identity.to_string()
    }
    #[getter]
    pub(crate) fn capabilities(&self) -> Vec<&'static str> {
        let mut result = vec!["lexical"];
        if self.inner.ann.is_some() {
            result.extend(["dense", "hybrid"])
        }
        if self.inner.assets.is_some() {
            result.extend(["tagmemo", "rivermemo", "dtsc", "thinking"])
        }
        result
    }
    #[getter]
    fn statistics_json(&self) -> String {
        self.inner.stats.to_string()
    }
    #[pyo3(signature=(vector,*,text="",options_json="{}",ghost_vectors=None,cancel=None))]
    fn observe(
        &self,
        py: Python<'_>,
        vector: PyReadonlyArray1<'_, f32>,
        text: &str,
        options_json: &str,
        ghost_vectors: Option<PyReadonlyArray2<'_, f32>>,
        cancel: Option<PyRef<'_, CancellationToken>>,
    ) -> PyResult<QueryObservation> {
        let vector = array(Some(vector))?.unwrap();
        let o = options(options_json)?;
        let ghosts = ghosts(ghost_vectors)?;
        let token = cancel.map(|v| v.clone()).unwrap_or_default();
        self.execute(py, &token, || {
            self.observation(&vector, text, &o, ghosts, &token)
        })
    }
    #[pyo3(signature=(*,mode,text="",vector=None,allowed_rows=None,options_json="{}",ghost_vectors=None,cancel=None))]
    #[allow(clippy::too_many_arguments)]
    fn search(
        &self,
        py: Python<'_>,
        mode: &str,
        text: &str,
        vector: Option<PyReadonlyArray1<'_, f32>>,
        allowed_rows: Option<PyReadonlyArray1<'_, u64>>,
        options_json: &str,
        ghost_vectors: Option<PyReadonlyArray2<'_, f32>>,
        cancel: Option<PyRef<'_, CancellationToken>>,
    ) -> PyResult<NativeQueryResult> {
        let vector = array(vector)?;
        let allowed = allowed(allowed_rows, self.inner.candidates.keys.len())?;
        let o = options(options_json)?;
        let ghosts = ghosts(ghost_vectors)?;
        let token = cancel.map(|v| v.clone()).unwrap_or_default();
        self.execute(py, &token, || {
            self.ranked(
                mode,
                text,
                vector.as_deref(),
                &allowed,
                o,
                ghosts,
                None,
                &token,
            )
        })
    }
    #[pyo3(signature=(observation,*,allowed_rows=None,options_json="{}",cancel=None))]
    fn readout(
        &self,
        py: Python<'_>,
        observation: PyRef<'_, QueryObservation>,
        allowed_rows: Option<PyReadonlyArray1<'_, u64>>,
        options_json: &str,
        cancel: Option<PyRef<'_, CancellationToken>>,
    ) -> PyResult<NativeQueryResult> {
        let allowed = allowed(allowed_rows, self.inner.candidates.keys.len())?;
        let o = options(options_json)?;
        let token = cancel.map(|v| v.clone()).unwrap_or_default();
        let observation = &*observation;
        self.execute(py, &token, || {
            self.ranked(
                "rivermemo",
                &observation.text,
                None,
                &allowed,
                o,
                vec![],
                Some(observation),
                &token,
            )
        })
    }
    #[pyo3(signature=(observation,stages_json,*,cancel=None))]
    fn think(
        &self,
        py: Python<'_>,
        observation: PyRef<'_, QueryObservation>,
        stages_json: &str,
        cancel: Option<PyRef<'_, CancellationToken>>,
    ) -> PyResult<String> {
        let stages: Vec<ThinkingStage> = serde_json::from_str(stages_json)
            .map_err(|e| native_error(failure("wave.invalid_input", e)))?;
        let token = cancel.map(|v| v.clone()).unwrap_or_default();
        let observation = &*observation;
        self.execute(py, &token, || {
            self.validate_observation(observation)?;
            Ok(self
                .thinking(
                    observation,
                    &stages,
                    QueryControls::default().thinking_min_closure,
                    &(0..self.inner.candidates.keys.len() as u64).collect(),
                    &token,
                )
                .to_string())
        })
    }
    #[pyo3(signature=(rows,*,options_json="{}",cancel=None))]
    fn neighborhood(
        &self,
        py: Python<'_>,
        rows: PyReadonlyArray1<'_, u64>,
        options_json: &str,
        cancel: Option<PyRef<'_, CancellationToken>>,
    ) -> PyResult<String> {
        let rows = rows
            .as_slice()
            .map_err(|e| native_error(failure("wave.invalid_input", e)))?
            .to_vec();
        let options: Value = serde_json::from_str(options_json)
            .map_err(|e| native_error(failure("wave.invalid_input", e)))?;
        let limit = options.get("limit").and_then(Value::as_u64).unwrap_or(100) as usize;
        let token = cancel.map(|v| v.clone()).unwrap_or_default();
        self.execute(py,&token,||{
            let assets=self.inner.assets.as_ref().ok_or_else(||failure("wave.capability_missing","Graph unavailable"))?;
            let graph=assets.snapshot().graph;let mut edges=Vec::new();let mut nodes=Vec::new();let selected=rows.iter().copied().collect::<BTreeSet<_>>();
            for row in rows{token.check()?;ensure!(row<self.inner.concepts.keys.len()as u64,"Concept row out of bounds");
                let mut outgoing=0.0;let mut outdegree=0;
                if let Some(index)=graph.node_index.get(&(row as i64+1)){for cursor in graph.row_offsets[*index]..graph.row_offsets[*index+1]{outdegree+=1;outgoing+=graph.weights[cursor];edges.push(json!({"source":row,"target":graph.node_ids[graph.targets[cursor]]-1,"weight":graph.weights[cursor]}));}}
                let mut roots=BTreeSet::new();for seq in 0..self.inner.facts.sequence_keys.len(){if slice(&self.inner.input.facts.indptr,&self.inner.input.facts.indices,seq).contains(&row){roots.insert(self.inner.input.facts.sequence_roots[seq]);}}
                let candidate_rows=(0..self.inner.candidates.keys.len()).filter(|r|slice(&self.inner.input.candidates.membership_indptr,&self.inner.input.candidates.membership_indices,*r).contains(&row)).collect::<Vec<_>>();
                nodes.push(json!({"row":row,"independent_roots":roots.len(),"root_rows":roots,"inbound_weight":graph.inbound.get(&(row as i64+1)).copied().unwrap_or(0.0),"outbound_weight":outgoing,"outdegree":outdegree,"candidate_coverage":candidate_rows.len(),"candidate_rows":candidate_rows}));
            }
            let assoc=&self.inner.input.associations;let associations=(0..assoc.sources.len()).filter(|i|selected.contains(&assoc.sources[*i])||selected.contains(&assoc.targets[*i])).map(|i|json!({"key":self.inner.associations.keys[i],"relation":self.inner.associations.relations[i],"source":assoc.sources[i],"target":assoc.targets[i],"directed":assoc.directed[i],"strength":assoc.strengths[i],"confidence":assoc.confidences[i],"polarity":assoc.polarities[i],"evidence_roots":slice(&assoc.root_indptr,&assoc.root_indices,i)})).collect::<Vec<_>>();
            let total_edges=edges.len();edges.truncate(limit);Ok(json!({"nodes":nodes,"edges":edges,"associations":associations,"truncated":total_edges>limit}).to_string())
        })
    }
}
