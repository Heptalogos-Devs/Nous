//! Execute a compiled operator graph while all row identities remain snapshot-bound.
use super::delivery::RankedCandidates;
use super::*;
use nous_vcp::config::{
    AnnParameters, FusionParameters, LaneParameters, LaneRankParameters, LocateParameters,
    ReadoutParameters, UnionParameters,
};
use serde::Deserialize;

#[derive(Deserialize)]
struct Node {
    id: String,
    operator: String,
    inputs: BTreeMap<String, Value>,
    #[serde(default)]
    parameters: Value,
    #[serde(default)]
    branch: Option<usize>,
}
#[derive(Deserialize)]
struct Program {
    nodes: Vec<Node>,
    outputs: BTreeMap<String, Value>,
    #[serde(default)]
    branches: Vec<Branch>,
    #[serde(default)]
    query_prefix: String,
}
#[derive(Clone)]
enum Stage {
    Text(String),
    Context(Arc<Vec<ContextLexical>>),
    Stages(Arc<Vec<ThinkingStage>>),
    Vector(Vec<f32>, String),
    Observation(Arc<QueryObservation>),
    Candidates(Arc<CandidateSet>),
    Calibrated(Arc<CalibratedPool>),
    Ranked(Arc<RankedCandidates>),
    Links(Value),
    Result(Arc<Value>),
}
type Values = BTreeMap<(String, String), Stage>;
#[derive(Deserialize)]
struct Branch {
    text: String,
    allowed_rows: Vec<u64>,
    core_rows: Vec<u64>,
    vector_index: Option<usize>,
}

struct BranchContext {
    text: String,
    allowed: BTreeSet<u64>,
    local_vector: Option<Vec<f32>>,
    options: Options,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct RankAdjustment {
    name: Option<String>,
    kind: Option<String>,
    limit: Option<usize>,
    prefer: Vec<String>,
    avoid: Vec<String>,
    sort: Option<String>,
}
fn get<'a>(values: &'a Values, reference: &Value) -> Result<&'a Stage> {
    let node = reference["node"]
        .as_str()
        .context("Program reference node missing")?;
    let port = reference["port"]
        .as_str()
        .context("Program reference port missing")?;
    values
        .get(&(node.into(), port.into()))
        .with_context(|| format!("Program value {node}.{port} unavailable"))
}
fn input<'a>(values: &'a Values, node: &Node, port: &str) -> Result<&'a Stage> {
    get(
        values,
        node.inputs
            .get(port)
            .with_context(|| format!("Node {} input {port} missing", node.id))?,
    )
}
fn put(values: &mut Values, node: &Node, port: &str, value: Stage) {
    let value = match value {
        Stage::Ranked(mut ranked) => {
            let mut seen = BTreeSet::new();
            Arc::make_mut(&mut ranked)
                .used_lanes
                .retain(|lane| seen.insert(lane.clone()));
            Stage::Ranked(ranked)
        }
        other => other,
    };
    values.insert((node.id.clone(), port.into()), value);
}

impl Snapshot {
    #[allow(clippy::too_many_arguments)]
    pub fn execute_program(
        &self,
        name: &str,
        program_json: &str,
        text: &str,
        raw: Option<&[f32]>,
        contextual: Option<&[f32]>,
        branch_raw: &[Vec<f32>],
        branch_contextual: &[Vec<f32>],
        allowed: &BTreeSet<u64>,
        mut options: Options,
        ghost_vectors: Vec<Vec<f32>>,
        fixed: Option<&CandidateSet>,
        cancel: &CancellationToken,
    ) -> Result<Value> {
        let program: Program = serde_json::from_str(program_json)
            .map_err(|error| failure("wave.invalid_program", error))?;
        options.validate_grouping()?;
        QueryControls {
            top_k: options.top_k,
            group_scan_limit: options.group_scan_limit,
            candidates_per_group: options.candidates_per_group,
            ann_search_effort: options.ann_search_effort,
            thinking_min_closure: options.thinking_min_closure,
            rrf_k: options.rrf_k,
            preference_weight: options.preference_weight,
        }
        .validate()
        .map_err(|e| failure("wave.invalid_input", e))?;
        ensure!(
            allowed
                .iter()
                .all(|r| *r < self.inner.candidates.keys.len() as u64),
            "Candidate row outside snapshot"
        );
        if raw.is_some()
            || contextual.is_some()
            || !branch_raw.is_empty()
            || !branch_contextual.is_empty()
        {
            self.validate_query_space(&options)?;
        }
        let raw = raw.map(unit).transpose()?;
        let contextual = contextual.map(unit).transpose()?;
        let local_vector = raw.clone().or_else(|| contextual.clone());
        let mut values = Values::new();
        values.insert(("query".into(), "lexical".into()), Stage::Text(text.into()));
        let context = Arc::new(std::mem::take(&mut options.context_lexical));
        let stages = Arc::new(std::mem::take(&mut options.thinking_stages));
        values.insert(
            ("query".into(), "context_lexical".into()),
            Stage::Context(context.clone()),
        );
        values.insert(
            ("query".into(), "stages".into()),
            Stage::Stages(stages.clone()),
        );
        if let Some(vector) = raw {
            values.insert(
                ("query".into(), "raw_vector".into()),
                Stage::Vector(vector, "query_knn".into()),
            );
        }
        if let Some(vector) = contextual {
            values.insert(
                ("query".into(), "contextual_signal".into()),
                Stage::Vector(vector, "context".into()),
            );
        }
        if let Some(pool) = fixed {
            self.validate_candidates(pool)?;
            ensure!(
                pool.entries
                    .iter()
                    .all(|entry| allowed.contains(&entry.row)),
                "Fixed candidate set exceeds allowed scope"
            );
            values.insert(
                ("scope".into(), "candidates".into()),
                Stage::Candidates(Arc::new(pool.clone())),
            );
        }
        let options_value = serde_json::to_value(&options)?;
        let mut contexts = Vec::with_capacity(program.branches.len());
        let mut expression_allowed = BTreeSet::new();
        for (index, branch) in program.branches.iter().enumerate() {
            ensure!(
                branch.allowed_rows.iter().all(|row| allowed.contains(row)),
                "Expression branch exceeds the query scope"
            );
            let scope = branch.allowed_rows.iter().copied().collect::<BTreeSet<_>>();
            expression_allowed.extend(scope.iter().copied());
            let mut branch_options: Options = serde_json::from_value(options_value.clone())?;
            branch_options.core_rows.clone_from(&branch.core_rows);
            let (raw, contextual) = if let Some(i) = branch.vector_index {
                (
                    Some(unit(
                        branch_raw.get(i).context("Branch query vector missing")?,
                    )?),
                    Some(unit(
                        branch_contextual
                            .get(i)
                            .context("Branch context vector missing")?,
                    )?),
                )
            } else {
                (None, None)
            };
            let branch_local = raw.clone().or_else(|| contextual.clone());
            let query = format!("{}query:{index}", program.query_prefix);
            values.insert(
                (query.clone(), "lexical".into()),
                Stage::Text(branch.text.clone()),
            );
            values.insert(
                (query.clone(), "context_lexical".into()),
                Stage::Context(context.clone()),
            );
            values.insert(
                (query.clone(), "stages".into()),
                Stage::Stages(stages.clone()),
            );
            if let Some(vector) = raw {
                values.insert(
                    (query.clone(), "raw_vector".into()),
                    Stage::Vector(vector, "query_knn".into()),
                );
            }
            if let Some(vector) = contextual {
                values.insert(
                    (query, "contextual_signal".into()),
                    Stage::Vector(vector, "context".into()),
                );
            }
            contexts.push(BranchContext {
                text: branch.text.clone(),
                allowed: scope,
                local_vector: branch_local,
                options: branch_options,
            });
        }
        let allowed = if program.branches.is_empty() {
            allowed
        } else {
            &expression_allowed
        };
        let mut branch_coverage = Vec::new();
        for node in &program.nodes {
            cancel.check()?;
            if node.branch.is_none() {
                for context in &contexts {
                    options
                        .lane_coverage
                        .lock()
                        .unwrap()
                        .extend(context.options.lane_coverage.lock().unwrap().clone());
                }
            }
            let (text, allowed, local_vector, options) = if let Some(index) = node.branch {
                let context = contexts
                    .get_mut(index)
                    .context("Program branch is unavailable")?;
                (
                    context.text.as_str(),
                    &context.allowed,
                    &context.local_vector,
                    &mut context.options,
                )
            } else {
                (text, allowed, &local_vector, &mut options)
            };
            match node.operator.as_str() {
                "thinking.combine" => {
                    let references = node
                        .inputs
                        .get("routes")
                        .and_then(Value::as_array)
                        .context("Thinking branch outputs missing")?;
                    let names = node.parameters["branches"]
                        .as_array()
                        .context("Thinking branch names missing")?;
                    ensure!(
                        references.len() == names.len(),
                        "Thinking branch names do not match outputs"
                    );
                    let mut stages = Vec::new();
                    let mut unavailable = Vec::new();
                    for (reference, name) in references.iter().zip(names) {
                        let Stage::Links(value) = get(&values, reference)? else {
                            anyhow::bail!("Thinking combination requires routes")
                        };
                        if let Some(items) = value.get("stages").and_then(Value::as_array) {
                            for item in items {
                                let mut stage = item.clone();
                                stage["branch"] = name.clone();
                                stages.push(stage);
                            }
                        }
                        if let Some(reason) = value.get("unavailable").and_then(Value::as_object) {
                            let mut reason = reason.clone();
                            reason.insert("branch".into(), name.clone());
                            unavailable.push(Value::Object(reason));
                        }
                    }
                    if options.explore || !stages.is_empty() {
                        for name in node.parameters["missing"].as_array().into_iter().flatten() {
                            unavailable
                                .push(json!({"branch":name,"code":"thinking.no_observation"}));
                        }
                    }
                    let result = if stages.is_empty() && unavailable.is_empty() {
                        Value::Null
                    } else {
                        let mut value = json!({"stages": stages});
                        if !unavailable.is_empty() {
                            value["branch_unavailable"] = json!(unavailable);
                            if value["stages"].as_array().is_some_and(Vec::is_empty) {
                                value["unavailable"] =
                                    json!({"code":"thinking.branches_unavailable"});
                            }
                        }
                        value
                    };
                    put(&mut values, node, "routes", Stage::Links(result));
                }
                "rank.scope" => {
                    let ranked = self.rank_scope(allowed, options, local_vector.clone());
                    put(&mut values, node, "ranked", Stage::Ranked(Arc::new(ranked)));
                }
                "rank.adjust" | "rank.combine" => {
                    let parameters: RankAdjustment =
                        serde_json::from_value(node.parameters.clone())?;
                    let mut ranked = if node.operator == "rank.adjust" {
                        let Stage::Ranked(ranked) = input(&values, node, "ranked")? else {
                            anyhow::bail!("Ranking adjustment requires ranked candidates")
                        };
                        (**ranked).clone()
                    } else {
                        let refs = node
                            .inputs
                            .get("rankings")
                            .and_then(Value::as_array)
                            .context("Combined rankings missing")?;
                        let mut rankings = Vec::new();
                        for reference in refs {
                            let Stage::Ranked(ranked) = get(&values, reference)? else {
                                anyhow::bail!("Combination requires ranked candidates")
                            };
                            rankings.push(ranked.as_ref());
                        }
                        ensure!(
                            !rankings.is_empty(),
                            "Combination requires at least one ranking"
                        );
                        let mut combined = if rankings.len() == 1 {
                            rankings[0].clone()
                        } else {
                            let inputs = rankings
                                .iter()
                                .map(|ranked| (*ranked, 1.0))
                                .collect::<Vec<_>>();
                            self.fuse_rankings(
                                &inputs,
                                options.rrf_k,
                                options,
                                local_vector.clone(),
                            )?
                        };
                        if parameters.kind.as_deref() == Some("all") {
                            let mut common: Option<BTreeSet<u64>> = None;
                            for ranking in rankings {
                                let groups = ranking
                                    .hits
                                    .iter()
                                    .filter_map(|hit| hit["row"].as_u64())
                                    .map(|row| self.inner.candidates.group_rows[row as usize])
                                    .collect::<BTreeSet<_>>();
                                common = Some(match common {
                                    Some(previous) => {
                                        previous.intersection(&groups).copied().collect()
                                    }
                                    None => groups,
                                });
                            }
                            let common = common.unwrap_or_default();
                            combined.hits.retain(|hit| {
                                common.contains(
                                    &self.inner.candidates.group_rows
                                        [hit["row"].as_u64().unwrap() as usize],
                                )
                            });
                        } else {
                            ensure!(
                                parameters.kind.as_deref() == Some("any"),
                                "Invalid combination"
                            );
                        }
                        combined
                    };
                    let lanes = options.lane_coverage.lock().unwrap().clone();
                    ranked = self.apply_preferences(
                        &ranked,
                        &parameters.prefer,
                        &parameters.avoid,
                        allowed,
                        options,
                        cancel,
                    )?;
                    self.sort_ranked(&mut ranked, parameters.sort.as_deref(), options)?;
                    let before = ranked
                        .hits
                        .iter()
                        .filter_map(|hit| hit["row"].as_u64())
                        .map(|row| self.inner.candidates.group_rows[row as usize])
                        .collect::<BTreeSet<_>>()
                        .len();
                    self.limit_ranked_groups(&mut ranked, parameters.limit);
                    if let Some(name) = parameters.name {
                        let groups = ranked
                            .hits
                            .iter()
                            .filter_map(|hit| hit["row"].as_u64())
                            .map(|row| self.inner.candidates.group_rows[row as usize])
                            .collect::<BTreeSet<_>>()
                            .len();
                        let scope_groups = allowed
                            .iter()
                            .map(|row| self.inner.candidates.group_rows[*row as usize])
                            .collect::<BTreeSet<_>>()
                            .len();
                        let scope_only = text.trim().is_empty() && options.core_rows.is_empty();
                        branch_coverage.push(json!({"name":name,"coverage":{
                            "requested_groups":parameters.limit,"returned_groups":groups,
                            "eligible_groups_known":null,"allowed_groups":scope_groups,
                            "recall_exhausted":scope_only || (!lanes.is_empty() && lanes.values().all(|lane|lane.recall_exhausted)),
                            "truncated":before>groups || lanes.values().any(|lane|lane.truncated),
                            "reason":if before>groups {Some("branch_limit")} else {None},"lanes":lanes,
                            "lane_limit_unit":if options.grouping=="document" {"groups"} else {"candidates"},
                            "group_scan_limit":options.group_scan_limit,"passages_per_group":options.passages_per_group,
                            "candidates":ranked.hits.len(),"total_candidates":self.inner.candidates.keys.len(),
                            "max_union_candidates":options.readout.max_union_candidates,"top_k":options.top_k,
                            "offered_candidates":ranked.offered_candidates,"ranked_candidates":ranked.hits.len(),
                            "unique_candidates":ranked.hits.len(),"candidate_limit_reached":false,"top_k_truncated":false
                        }}));
                    }
                    put(&mut values, node, "ranked", Stage::Ranked(Arc::new(ranked)));
                }
                "vcp.observe" => {
                    let Stage::Vector(vector, _) = input(&values, node, "signal")? else {
                        anyhow::bail!("Observation requires a vector signal")
                    };
                    let observation = Arc::new(self.observation(
                        vector,
                        text,
                        &options,
                        ghost_vectors.clone(),
                        cancel,
                    )?);
                    for (port, vector, lane) in [
                        (
                            "enhanced",
                            &observation.output.enhanced_vector,
                            "denoised_field_knn",
                        ),
                        ("local", &observation.output.local_vector, "local_field_knn"),
                        (
                            "transfer",
                            &observation.output.transfer_vector,
                            "transfer_field_knn",
                        ),
                    ] {
                        put(
                            &mut values,
                            node,
                            port,
                            Stage::Vector(vector.clone(), lane.into()),
                        );
                    }
                    put(
                        &mut values,
                        node,
                        "observation",
                        Stage::Observation(observation),
                    );
                }
                "ann.search" => {
                    let Stage::Vector(vector, lane) = input(&values, node, "vector")? else {
                        anyhow::bail!("ANN requires a vector signal")
                    };
                    let parameters: AnnParameters =
                        serde_json::from_value(node.parameters.clone())?;
                    let limit = parameters
                        .candidate_limit
                        .unwrap_or_else(|| options.lane_limit(lane));
                    let candidates_per_group = parameters
                        .candidates_per_group
                        .unwrap_or(options.candidates_per_group);
                    ensure!(
                        (1..=10000).contains(&candidates_per_group),
                        "Invalid candidate group quota"
                    );
                    let search_effort = parameters
                        .search_effort
                        .unwrap_or(options.ann_search_effort);
                    ensure!(
                        (1..=1_000_000).contains(&search_effort),
                        "Invalid ANN search effort"
                    );
                    let hits = if vector.iter().any(|v| *v != 0.0) {
                        self.dense(
                            vector,
                            allowed,
                            limit,
                            candidates_per_group,
                            search_effort,
                            &options,
                            lane,
                            cancel,
                        )?
                    } else {
                        Vec::new()
                    };
                    let candidates = self.lane_set(lane, hits, &options);
                    put(
                        &mut values,
                        node,
                        "candidates",
                        Stage::Candidates(Arc::new(candidates)),
                    );
                }
                "lexical.search" => {
                    let Stage::Text(query) = input(&values, node, "text")? else {
                        anyhow::bail!("Lexical search requires text")
                    };
                    let parameters: LaneParameters =
                        serde_json::from_value(node.parameters.clone())?;
                    let limit = parameters
                        .candidate_limit
                        .unwrap_or_else(|| options.lane_limit("bm25"));
                    let candidates_per_group = parameters
                        .candidates_per_group
                        .unwrap_or(options.candidates_per_group);
                    ensure!(
                        (1..=10000).contains(&candidates_per_group),
                        "Invalid candidate group quota"
                    );
                    let hits = self.lexical(
                        query,
                        allowed,
                        limit,
                        candidates_per_group,
                        &options,
                        "bm25",
                        cancel,
                    )?;
                    put(
                        &mut values,
                        node,
                        "candidates",
                        Stage::Candidates(Arc::new(self.lane_set("bm25", hits, &options))),
                    );
                }
                "concept.direct" => {
                    let Stage::Observation(observation) = input(&values, node, "observation")?
                    else {
                        anyhow::bail!("Direct recall requires an observation")
                    };
                    let parameters: LaneParameters =
                        serde_json::from_value(node.parameters.clone())?;
                    let limit = parameters
                        .candidate_limit
                        .unwrap_or_else(|| options.lane_limit("direct_anchor"));
                    let candidates_per_group = parameters
                        .candidates_per_group
                        .unwrap_or(options.candidates_per_group);
                    ensure!(
                        (1..=10000).contains(&candidates_per_group),
                        "Invalid candidate group quota"
                    );
                    let result = self.direct_lane(
                        observation,
                        allowed,
                        limit,
                        candidates_per_group,
                        &options,
                        cancel,
                    )?;
                    put(
                        &mut values,
                        node,
                        "candidates",
                        Stage::Candidates(Arc::new(result)),
                    );
                }
                "candidates.union" => {
                    let references = node
                        .inputs
                        .get("sets")
                        .and_then(Value::as_array)
                        .context("Union inputs missing")?;
                    let mut sets = Vec::new();
                    for reference in references {
                        let Stage::Candidates(set) = get(&values, reference)? else {
                            anyhow::bail!("Union requires candidate sets")
                        };
                        sets.push(set.as_ref());
                    }
                    let parameters: UnionParameters =
                        serde_json::from_value(node.parameters.clone())?;
                    let candidates_per_group = parameters
                        .candidates_per_group
                        .unwrap_or(options.candidates_per_group);
                    ensure!(
                        (1..=10000).contains(&candidates_per_group),
                        "Invalid candidate group quota"
                    );
                    let result = self.union_sets(
                        &sets,
                        parameters
                            .candidate_limit
                            .unwrap_or(options.readout.max_union_candidates),
                        parameters.group_limit,
                        candidates_per_group,
                        &options,
                    )?;
                    put(
                        &mut values,
                        node,
                        "candidates",
                        Stage::Candidates(Arc::new(result)),
                    );
                }
                "vcp.calibrate" => {
                    let Stage::Observation(observation) = input(&values, node, "observation")?
                    else {
                        anyhow::bail!("Calibration requires an observation")
                    };
                    let Stage::Candidates(candidates) = input(&values, node, "candidates")? else {
                        anyhow::bail!("Calibration requires candidates")
                    };
                    let result = self.calibrate(observation, candidates, &options)?;
                    put(
                        &mut values,
                        node,
                        "pool",
                        Stage::Calibrated(Arc::new(result)),
                    );
                }
                "river.readout" => {
                    let Stage::Observation(observation) = input(&values, node, "observation")?
                    else {
                        anyhow::bail!("River readout requires an observation")
                    };
                    let Stage::Calibrated(pool) = input(&values, node, "pool")? else {
                        anyhow::bail!("River readout requires a calibrated pool")
                    };
                    options
                        .lane_coverage
                        .lock()
                        .unwrap()
                        .extend(pool.candidates.lane_coverage.clone());
                    let parameters: ReadoutParameters =
                        serde_json::from_value(node.parameters.clone())?;
                    let mut result = self.river_readout(
                        observation,
                        pool,
                        allowed,
                        &options,
                        cancel,
                        local_vector.clone(),
                    )?;
                    self.limit_ranked_groups(&mut result, parameters.group_limit);
                    put(&mut values, node, "ranked", Stage::Ranked(Arc::new(result)));
                }
                "dtsc.readout" => {
                    let Stage::Observation(observation) = input(&values, node, "observation")?
                    else {
                        anyhow::bail!("DTSC requires an observation")
                    };
                    let Stage::Candidates(candidates) = input(&values, node, "candidates")? else {
                        anyhow::bail!("DTSC requires candidates")
                    };
                    options
                        .lane_coverage
                        .lock()
                        .unwrap()
                        .extend(candidates.lane_coverage.clone());
                    let parameters: ReadoutParameters =
                        serde_json::from_value(node.parameters.clone())?;
                    let mut result = self.dtsc_readout(
                        observation,
                        candidates,
                        allowed,
                        &options,
                        cancel,
                        local_vector.clone(),
                    )?;
                    self.limit_ranked_groups(&mut result, parameters.group_limit);
                    put(&mut values, node, "ranked", Stage::Ranked(Arc::new(result)));
                }
                "vcp.think" => {
                    let Stage::Observation(observation) = input(&values, node, "observation")?
                    else {
                        anyhow::bail!("Thinking requires an observation")
                    };
                    let Stage::Ranked(ranked) = input(&values, node, "ranked")? else {
                        anyhow::bail!("Thinking requires ranked candidates")
                    };
                    let Stage::Stages(stages) = input(&values, node, "stages")? else {
                        anyhow::bail!("Thinking requires stage inputs")
                    };
                    let discovered;
                    let stages: &[ThinkingStage] = if options.explore && stages.is_empty() {
                        discovered = vec![ThinkingStage {
                            name: "explore".into(),
                            candidate_rows: ranked
                                .hits
                                .iter()
                                .filter_map(|hit| hit["row"].as_u64())
                                .collect(),
                            top_k: options.top_k,
                        }];
                        &discovered
                    } else {
                        stages.as_slice()
                    };
                    let routes = self.thinking(
                        observation,
                        stages,
                        options.thinking_min_closure,
                        allowed,
                        cancel,
                    )?;
                    put(&mut values, node, "routes", Stage::Links(routes));
                }
                "rank.from_lane" => {
                    let Stage::Candidates(candidates) = input(&values, node, "candidates")? else {
                        anyhow::bail!("Rank requires candidates")
                    };
                    let parameters: LaneRankParameters =
                        serde_json::from_value(node.parameters.clone())?;
                    options
                        .lane_coverage
                        .lock()
                        .unwrap()
                        .extend(candidates.lane_coverage.clone());
                    let result = self.rank_lane(
                        candidates,
                        &parameters.lane,
                        parameters.group_limit,
                        &options,
                        local_vector.clone(),
                    )?;
                    put(&mut values, node, "ranked", Stage::Ranked(Arc::new(result)));
                }
                "rank.cosine" => {
                    let Stage::Candidates(candidates) = input(&values, node, "candidates")? else {
                        anyhow::bail!("Cosine readout requires candidates")
                    };
                    let Stage::Vector(vector, _) = input(&values, node, "vector")? else {
                        anyhow::bail!("Cosine readout requires a vector")
                    };
                    let result = self.rank_cosine(candidates, vector, &options)?;
                    put(&mut values, node, "ranked", Stage::Ranked(Arc::new(result)));
                }
                "rank.fuse" => {
                    let references = node
                        .inputs
                        .get("rankings")
                        .and_then(Value::as_array)
                        .context("Fusion inputs missing")?;
                    let parameters: FusionParameters =
                        serde_json::from_value(node.parameters.clone())?;
                    ensure!(
                        parameters.weights.is_empty()
                            || parameters.weights.len() == references.len(),
                        "Fusion weights must match rankings"
                    );
                    let mut rankings = Vec::new();
                    for (index, reference) in references.iter().enumerate() {
                        let Stage::Ranked(ranked) = get(&values, reference)? else {
                            anyhow::bail!("Fusion requires ranked inputs")
                        };
                        rankings.push((
                            ranked.as_ref(),
                            parameters.weights.get(index).copied().unwrap_or(1.0),
                        ));
                    }
                    let result = self.fuse_rankings(
                        &rankings,
                        parameters.k.unwrap_or(options.rrf_k),
                        &options,
                        local_vector.clone(),
                    )?;
                    put(&mut values, node, "ranked", Stage::Ranked(Arc::new(result)));
                }
                "rank.context" => {
                    let Stage::Ranked(base) = input(&values, node, "ranked")? else {
                        anyhow::bail!("Context ranking requires a ranked set")
                    };
                    let Stage::Candidates(candidates) = input(&values, node, "candidates")? else {
                        anyhow::bail!("Context ranking requires recalled candidates")
                    };
                    let result = self.rank_context(base, candidates, &options)?;
                    put(&mut values, node, "ranked", Stage::Ranked(Arc::new(result)));
                }
                "context.recall" => {
                    let Stage::Context(signals) = input(&values, node, "signals")? else {
                        anyhow::bail!("Context recall requires context signals")
                    };
                    let candidates = self.recall_context(signals, allowed, &options, cancel)?;
                    put(
                        &mut values,
                        node,
                        "candidates",
                        Stage::Candidates(Arc::new(candidates)),
                    );
                }
                "passages.locate" => {
                    let Stage::Ranked(ranked) = input(&values, node, "ranked")? else {
                        anyhow::bail!("Localization requires ranking")
                    };
                    let parameters: LocateParameters =
                        serde_json::from_value(node.parameters.clone())?;
                    parameters
                        .validate()
                        .map_err(|error| failure("wave.invalid_program", error))?;
                    options.local_units_per_group = parameters.local_units_per_group;
                    options.localize_signals = parameters
                        .signals
                        .iter()
                        .map(|signal| signal.name().into())
                        .collect();
                    options.localize_rrf_k = parameters.rrf_k;
                    options.localize_lexical_weight = parameters.lexical_weight;
                    options.localize_semantic_weight = parameters.semantic_weight;
                    let result = self.deliver(
                        name,
                        text,
                        allowed,
                        &options,
                        None,
                        cancel,
                        (**ranked).clone(),
                    )?;
                    put(
                        &mut values,
                        node,
                        "passages",
                        Stage::Result(Arc::new(result)),
                    );
                }
                _ => {
                    return Err(failure(
                        "wave.invalid_program",
                        format!("Unsupported operator {}", node.operator),
                    ));
                }
            }
        }
        let mut outputs = serde_json::Map::new();
        for (name, reference) in &program.outputs {
            let mut result = match get(&values, reference)? {
                Stage::Result(value) => (**value).clone(),
                Stage::Links(value) => {
                    outputs.insert(name.clone(), value.clone());
                    continue;
                }
                _ => anyhow::bail!("Unsupported Native output type"),
            };
            if !program.branches.is_empty() {
                let truncated = branch_coverage
                    .iter()
                    .any(|branch| branch["coverage"]["truncated"].as_bool() == Some(true));
                let exhausted = branch_coverage
                    .iter()
                    .all(|branch| branch["coverage"]["recall_exhausted"].as_bool() == Some(true));
                result["branch_coverage"] = json!(branch_coverage);
                result["coverage"]["truncated"] =
                    json!(result["coverage"]["truncated"].as_bool().unwrap_or(false) || truncated);
                result["coverage"]["recall_exhausted"] = json!(exhausted);
                for key in [
                    "eligible_groups_known",
                    "candidates",
                    "offered_candidates",
                    "ranked_candidates",
                    "unique_candidates",
                ] {
                    result["coverage"][key] = Value::Null;
                }
            }
            if options.include_trace {
                let rankings = values
                .iter()
                .filter_map(|((node, port), value)| match value {
                    Stage::Ranked(ranked) => Some((
                        format!("{node}.{port}"),
                        json!({
                            "score_kind": ranked.score_kind,
                            "used_lanes": ranked.used_lanes,
                            "hits": ranked.hits,
                            "operator": program.nodes.iter().find(|entry| entry.id == *node).map(|entry| &entry.operator),
                            "based_on": program.nodes.iter().find(|entry| entry.id == *node).map(|entry| &entry.inputs),
                            "details": ranked.diagnostics,
                        }),
                    )),
                    _ => None,
                })
                .collect::<BTreeMap<_, _>>();
                result["diagnostics"]["ranked_nodes"] = json!(rankings);
                result["diagnostics"]["observation_count"] = json!(
                    values
                        .values()
                        .filter(|value| matches!(value, Stage::Observation(_)))
                        .count()
                );
            }
            outputs.insert(name.clone(), result);
        }
        Ok(Value::Object(outputs))
    }
}
