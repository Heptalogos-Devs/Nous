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
}
#[derive(Deserialize)]
struct Program {
    nodes: Vec<Node>,
    outputs: BTreeMap<String, Value>,
}
#[derive(Clone)]
enum Stage {
    Marker,
    Text(String),
    Vector(Vec<f32>, String),
    Observation(Arc<QueryObservation>),
    Candidates(Arc<CandidateSet>),
    Calibrated(Arc<CalibratedPool>),
    Ranked(Arc<RankedCandidates>),
    Links(Value),
    Result(Arc<NativeQueryResult>),
}
type Values = BTreeMap<(String, String), Stage>;
enum ProgramRun {
    Complete(NativeQueryResult),
    Ranked(
        RankedCandidates,
        Option<Arc<QueryObservation>>,
        BTreeMap<String, crate::grouping::LaneCoverage>,
    ),
}
#[derive(Deserialize)]
struct ExpressionBranch {
    name: String,
    text: String,
    allowed_rows: Vec<u64>,
    core_rows: Vec<u64>,
    vector_index: Option<usize>,
    limit: Option<usize>,
    prefer: Vec<String>,
    avoid: Vec<String>,
    sort: Option<String>,
}
#[derive(Deserialize)]
struct ExpressionTree {
    kind: String,
    branch: Option<usize>,
    #[serde(default)]
    children: Vec<ExpressionTree>,
    limit: Option<usize>,
    sort: Option<String>,
    #[serde(default)]
    prefer: Vec<String>,
    #[serde(default)]
    avoid: Vec<String>,
}
#[derive(Deserialize)]
struct BoundExpression {
    tree: ExpressionTree,
    branches: Vec<ExpressionBranch>,
    #[serde(default)]
    prefer: Vec<String>,
    #[serde(default)]
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
    fn combine_expression(
        &self,
        tree: &ExpressionTree,
        branches: &[RankedCandidates],
        options: &Options,
        local_vector: Option<Vec<f32>>,
        allowed: &BTreeSet<u64>,
        cancel: &CancellationToken,
    ) -> Result<RankedCandidates> {
        if tree.kind == "term" {
            let index = tree.branch.context("Expression term needs a branch")?;
            return branches
                .get(index)
                .cloned()
                .context("Expression branch is unavailable");
        }
        ensure!(
            matches!(tree.kind.as_str(), "any" | "all") && !tree.children.is_empty(),
            "Expression must be a nonempty any/all combination"
        );
        let children = tree
            .children
            .iter()
            .map(|child| {
                self.combine_expression(
                    child,
                    branches,
                    options,
                    local_vector.clone(),
                    allowed,
                    cancel,
                )
            })
            .collect::<Result<Vec<_>>>()?;
        let mut result = if children.len() == 1 {
            children[0].clone()
        } else {
            let inputs = children
                .iter()
                .map(|ranked| (ranked, 1.0))
                .collect::<Vec<_>>();
            self.fuse_rankings(&inputs, options.rrf_k, options, local_vector)?
        };
        if tree.kind == "all" {
            let mut common: Option<BTreeSet<u64>> = None;
            for ranked in &children {
                let groups = ranked
                    .hits
                    .iter()
                    .filter_map(|hit| hit["row"].as_u64())
                    .map(|row| self.inner.candidates.group_rows[row as usize])
                    .collect::<BTreeSet<_>>();
                common = Some(match common {
                    Some(previous) => previous.intersection(&groups).copied().collect(),
                    None => groups,
                });
            }
            let common = common.unwrap_or_default();
            result.hits.retain(|hit| {
                common.contains(
                    &self.inner.candidates.group_rows[hit["row"].as_u64().unwrap() as usize],
                )
            });
        }
        result =
            self.apply_preferences(&result, &tree.prefer, &tree.avoid, allowed, options, cancel)?;
        self.sort_ranked(&mut result, tree.sort.as_deref(), options)?;
        self.limit_ranked_groups(&mut result, tree.limit);
        Ok(result)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn execute_expression(
        &self,
        name: &str,
        program_json: &str,
        expression_json: &str,
        text: &str,
        original_vector: Option<&[f32]>,
        raw_vectors: &[Vec<f32>],
        contextual_vectors: &[Vec<f32>],
        allowed: &BTreeSet<u64>,
        options_json: &str,
        ghost_vectors: Vec<Vec<f32>>,
        cancel: &CancellationToken,
    ) -> Result<NativeQueryResult> {
        let expression: BoundExpression = serde_json::from_str(expression_json)
            .map_err(|error| failure("wave.invalid_input", error))?;
        let plan: Program = serde_json::from_str(program_json)
            .map_err(|error| failure("wave.invalid_program", error))?;
        let context_nodes = plan
            .nodes
            .iter()
            .filter(|node| node.operator == "context.recall")
            .map(|node| node.id.as_str())
            .collect::<BTreeSet<_>>();
        let context_in_pool = plan
            .nodes
            .iter()
            .filter(|node| node.operator == "candidates.union")
            .any(|node| {
                node.inputs
                    .get("sets")
                    .and_then(Value::as_array)
                    .is_some_and(|sets| {
                        sets.iter().any(|reference| {
                            reference["node"]
                                .as_str()
                                .is_some_and(|name| context_nodes.contains(name))
                        })
                    })
            });
        let mut options: Options = serde_json::from_str(options_json)?;
        let local = original_vector.map(unit).transpose()?;
        let mut rankings = Vec::new();
        let mut observations = Vec::new();
        let mut branch_coverage = Vec::new();
        let mut expression_allowed = BTreeSet::new();
        for branch in &expression.branches {
            cancel.check()?;
            let scope = branch
                .allowed_rows
                .iter()
                .copied()
                .filter(|row| allowed.contains(row))
                .collect::<BTreeSet<_>>();
            expression_allowed.extend(scope.iter().copied());
            let mut branch_options: Options = serde_json::from_str(options_json)?;
            branch_options.core_rows = branch.core_rows.clone();
            if !context_in_pool {
                branch_options.context_lexical.clear();
            }
            branch_options.explore = false;
            branch_options.thinking_stages.clear();
            let scope_only = branch.text.trim().is_empty() && branch.core_rows.is_empty();
            let (mut ranked, observation, lanes) = if scope_only {
                (
                    self.rank_scope(&scope, &branch_options, local.clone()),
                    None,
                    BTreeMap::new(),
                )
            } else {
                let raw = branch
                    .vector_index
                    .and_then(|index| raw_vectors.get(index))
                    .map(Vec::as_slice);
                let contextual = branch
                    .vector_index
                    .and_then(|index| contextual_vectors.get(index))
                    .map(Vec::as_slice);
                match self.run_program(
                    name,
                    program_json,
                    &branch.text,
                    raw,
                    contextual,
                    &scope,
                    branch_options,
                    ghost_vectors.clone(),
                    None,
                    cancel,
                    true,
                )? {
                    ProgramRun::Ranked(ranked, observation, lanes) => (ranked, observation, lanes),
                    ProgramRun::Complete(_) => {
                        return Err(failure(
                            "wave.invalid_program",
                            "Branch produced a complete delivery",
                        ));
                    }
                }
            };
            let branch_options: Options = serde_json::from_str(options_json)?;
            if context_in_pool && expression.tree.kind != "term" {
                let direct_groups = ranked
                    .hits
                    .iter()
                    .filter(|hit| {
                        hit["channels"].as_array().is_some_and(|lanes| {
                            lanes.iter().any(|lane| {
                                lane.as_str()
                                    .is_some_and(|name| !name.starts_with("context:"))
                            })
                        })
                    })
                    .filter_map(|hit| hit["row"].as_u64())
                    .map(|row| self.inner.candidates.group_rows[row as usize])
                    .collect::<BTreeSet<_>>();
                ranked.hits.retain(|hit| {
                    direct_groups.contains(
                        &self.inner.candidates.group_rows[hit["row"].as_u64().unwrap() as usize],
                    )
                });
            }
            ranked = self.apply_preferences(
                &ranked,
                &branch.prefer,
                &branch.avoid,
                &scope,
                &branch_options,
                cancel,
            )?;
            self.sort_ranked(&mut ranked, branch.sort.as_deref(), &branch_options)?;
            let groups_before_limit = ranked
                .hits
                .iter()
                .filter_map(|hit| hit["row"].as_u64())
                .map(|row| self.inner.candidates.group_rows[row as usize])
                .collect::<BTreeSet<_>>()
                .len();
            self.limit_ranked_groups(&mut ranked, branch.limit);
            let groups = ranked
                .hits
                .iter()
                .filter_map(|hit| hit["row"].as_u64())
                .map(|row| self.inner.candidates.group_rows[row as usize])
                .collect::<BTreeSet<_>>();
            let scope_groups = scope
                .iter()
                .map(|row| self.inner.candidates.group_rows[*row as usize])
                .collect::<BTreeSet<_>>()
                .len();
            branch_coverage.push(json!({"name":branch.name,"coverage":{
                "requested_groups":branch.limit,"returned_groups":groups.len(),"eligible_groups_known":null,
                "allowed_groups":scope_groups,"recall_exhausted":scope_only || (!lanes.is_empty() && lanes.values().all(|lane|lane.recall_exhausted)),
                "truncated":groups_before_limit>groups.len() || lanes.values().any(|lane|lane.truncated),
                "reason":if groups_before_limit>groups.len(){Some("branch_limit")}else{None},"lanes":lanes,
                "lane_limit_unit":if options.grouping=="document"{"groups"}else{"candidates"},
                "group_scan_limit":options.group_scan_limit,"passages_per_group":options.passages_per_group,
                "candidates":ranked.hits.len(),"total_candidates":self.inner.candidates.keys.len(),
                "max_union_candidates":options.readout.max_union_candidates,"top_k":options.top_k,
                "offered_candidates":ranked.offered_candidates,"ranked_candidates":ranked.hits.len(),
                "unique_candidates":ranked.hits.len(),"candidate_limit_reached":false,"top_k_truncated":false
            }}));
            rankings.push(ranked);
            observations.push(observation);
        }
        let mut ranked = self.combine_expression(
            &expression.tree,
            &rankings,
            &options,
            local.clone(),
            &expression_allowed,
            cancel,
        )?;
        let context_allowed = if expression.tree.kind == "term" {
            expression_allowed.clone()
        } else {
            let groups = ranked
                .hits
                .iter()
                .filter_map(|hit| hit["row"].as_u64())
                .map(|row| self.inner.candidates.group_rows[row as usize])
                .collect::<BTreeSet<_>>();
            expression_allowed
                .iter()
                .copied()
                .filter(|row| groups.contains(&self.inner.candidates.group_rows[*row as usize]))
                .collect()
        };
        if !context_in_pool && !context_nodes.is_empty() {
            let context_candidates = self.recall_context(&context_allowed, &options, cancel)?;
            for fragment in &options.context_lexical {
                let lane = format!("context:{}", fragment.key);
                let rows = context_candidates
                    .entries
                    .iter()
                    .filter(|entry| entry.lanes.contains_key(&lane))
                    .map(|entry| entry.row)
                    .collect::<Vec<_>>();
                let groups = rows
                    .iter()
                    .map(|row| self.inner.candidates.group_rows[*row as usize])
                    .collect::<BTreeSet<_>>();
                let lane_coverage = context_candidates.lane_coverage.get(&lane);
                let lanes = lane_coverage
                    .map(|coverage| {
                        [(lane.clone(), coverage.clone())]
                            .into_iter()
                            .collect::<BTreeMap<_, _>>()
                    })
                    .unwrap_or_default();
                branch_coverage.push(json!({"name":lane,"coverage":{
                    "requested_groups":null,"returned_groups":groups.len(),
                    "eligible_groups_known":lane_coverage.and_then(|coverage| coverage.eligible_groups_known),
                    "allowed_groups":context_allowed.iter().map(|row| self.inner.candidates.group_rows[*row as usize]).collect::<BTreeSet<_>>().len(),
                    "recall_exhausted":lane_coverage.is_some_and(|coverage|coverage.recall_exhausted),
                    "truncated":lane_coverage.is_some_and(|coverage|coverage.truncated),
                    "reason":lane_coverage.and_then(|coverage|coverage.reason.clone()),"lanes":lanes,
                    "lane_limit_unit":if options.grouping=="document"{"groups"}else{"candidates"},
                    "group_scan_limit":options.group_scan_limit,"passages_per_group":options.passages_per_group,
                    "candidates":rows.len(),"total_candidates":self.inner.candidates.keys.len(),
                    "max_union_candidates":options.readout.max_union_candidates,"top_k":options.top_k,
                    "offered_candidates":rows.len(),"ranked_candidates":rows.len(),"unique_candidates":rows.len(),
                    "candidate_limit_reached":false,"top_k_truncated":false
                }}));
            }
            ranked = self.rank_context(&ranked, &context_candidates, &options)?;
        }
        ranked = self.apply_preferences(
            &ranked,
            &expression.prefer,
            &expression.avoid,
            &context_allowed,
            &options,
            cancel,
        )?;
        self.sort_ranked(&mut ranked, expression.sort.as_deref(), &options)?;
        ranked.local_vector = local;
        if options.explore {
            if let Some(observation) = observations.iter().flatten().next() {
                let stages = vec![ThinkingStage {
                    name: "explore".into(),
                    top_k: options.top_k,
                    candidate_rows: ranked
                        .hits
                        .iter()
                        .filter_map(|hit| hit["row"].as_u64())
                        .collect(),
                }];
                ranked.thinking = self.thinking(
                    observation,
                    &stages,
                    options.thinking_min_closure,
                    &context_allowed,
                    cancel,
                )?;
            }
        }
        if let Some(locator) = plan
            .nodes
            .iter()
            .find(|node| node.operator == "passages.locate")
        {
            let parameters: LocateParameters = serde_json::from_value(locator.parameters.clone())?;
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
        }
        let mut result =
            self.deliver(name, text, &context_allowed, &options, None, cancel, ranked)?;
        let mut value: Value = serde_json::from_str(&result.result_json)?;
        value["branch_coverage"] = json!(branch_coverage);
        let any_truncated = value["branch_coverage"]
            .as_array()
            .unwrap()
            .iter()
            .any(|branch| branch["coverage"]["truncated"].as_bool() == Some(true));
        let all_exhausted = value["branch_coverage"]
            .as_array()
            .unwrap()
            .iter()
            .all(|branch| branch["coverage"]["recall_exhausted"].as_bool() == Some(true));
        value["coverage"]["truncated"] =
            json!(value["coverage"]["truncated"].as_bool().unwrap_or(false) || any_truncated);
        value["coverage"]["recall_exhausted"] = json!(all_exhausted);
        value["coverage"]["eligible_groups_known"] = Value::Null;
        value["coverage"]["candidates"] = Value::Null;
        value["coverage"]["offered_candidates"] = Value::Null;
        value["coverage"]["ranked_candidates"] = Value::Null;
        value["coverage"]["unique_candidates"] = Value::Null;
        result.result_json = value.to_string();
        Ok(result)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn execute_program(
        &self,
        name: &str,
        program_json: &str,
        text: &str,
        raw: Option<&[f32]>,
        contextual: Option<&[f32]>,
        allowed: &BTreeSet<u64>,
        options: Options,
        ghost_vectors: Vec<Vec<f32>>,
        fixed: Option<&CandidateSet>,
        cancel: &CancellationToken,
    ) -> Result<NativeQueryResult> {
        match self.run_program(
            name,
            program_json,
            text,
            raw,
            contextual,
            allowed,
            options,
            ghost_vectors,
            fixed,
            cancel,
            false,
        )? {
            ProgramRun::Complete(result) => Ok(result),
            ProgramRun::Ranked(_, _, _) => {
                unreachable!("complete program returned an intermediate rank")
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn run_program(
        &self,
        name: &str,
        program_json: &str,
        text: &str,
        raw: Option<&[f32]>,
        contextual: Option<&[f32]>,
        allowed: &BTreeSet<u64>,
        mut options: Options,
        ghost_vectors: Vec<Vec<f32>>,
        fixed: Option<&CandidateSet>,
        cancel: &CancellationToken,
        stop_at_rank: bool,
    ) -> Result<ProgramRun> {
        let program: Program =
            serde_json::from_str(program_json).map_err(|e| failure("wave.invalid_program", e))?;
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
        if raw.is_some() || contextual.is_some() {
            self.validate_query_space(&options)?;
        }
        let raw = raw.map(unit).transpose()?;
        let contextual = contextual.map(unit).transpose()?;
        let local_vector = raw.clone().or_else(|| contextual.clone());
        let mut values = Values::new();
        values.insert(("query".into(), "lexical".into()), Stage::Text(text.into()));
        values.insert(("query".into(), "intent".into()), Stage::Marker);
        values.insert(("query".into(), "stages".into()), Stage::Marker);
        values.insert(("query".into(), "context_lexical".into()), Stage::Marker);
        values.insert(("snapshot".into(), "handle".into()), Stage::Marker);
        values.insert(("scope".into(), "allowed".into()), Stage::Marker);
        values.insert(("scope".into(), "materials".into()), Stage::Marker);
        values.insert(("delivery".into(), "request".into()), Stage::Marker);
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
        for node in &program.nodes {
            cancel.check()?;
            match node.operator.as_str() {
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
                    let discovered;
                    let stages = if options.explore && options.thinking_stages.is_empty() {
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
                        &options.thinking_stages
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
                    let mut seen = BTreeSet::new();
                    for (index, reference) in references.iter().enumerate() {
                        let identity = serde_json::to_string(reference)?;
                        if !seen.insert(identity) {
                            continue;
                        }
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
                    let candidates = self.recall_context(allowed, &options, cancel)?;
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
                    if stop_at_rank {
                        let observation = values.values().find_map(|value| match value {
                            Stage::Observation(observation) => Some(observation.clone()),
                            _ => None,
                        });
                        return Ok(ProgramRun::Ranked(
                            (**ranked).clone(),
                            observation,
                            options.lane_coverage.lock().unwrap().clone(),
                        ));
                    }
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
                "passages.select" => {
                    let Stage::Result(result) = input(&values, node, "passages")? else {
                        anyhow::bail!("Selection requires localized passages")
                    };
                    let result = result.clone();
                    put(&mut values, node, "selected", Stage::Result(result));
                }
                "delivery.read" => {
                    let Stage::Result(result) = input(&values, node, "selected")? else {
                        anyhow::bail!("Delivery requires selected passages")
                    };
                    let result = result.clone();
                    put(&mut values, node, "evidence", Stage::Result(result));
                }
                _ => {
                    return Err(failure(
                        "wave.invalid_program",
                        format!("Unsupported operator {}", node.operator),
                    ));
                }
            }
        }
        let output = program
            .outputs
            .get("evidence")
            .context("Program evidence output missing")?;
        let Stage::Result(result) = get(&values, output)? else {
            anyhow::bail!("Program output is not evidence")
        };
        let mut result = (**result).clone();
        if let Some(reference) = program.outputs.get("thinking") {
            let Stage::Links(routes) = get(&values, reference)? else {
                anyhow::bail!("Thinking output is not routes")
            };
            let mut value: Value = serde_json::from_str(&result.result_json)?;
            value["thinking"] = routes.clone();
            result.result_json = value.to_string();
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
            let mut value: Value = serde_json::from_str(&result.result_json)?;
            value["diagnostics"]["ranked_nodes"] = json!(rankings);
            value["diagnostics"]["observation_count"] = json!(
                values
                    .values()
                    .filter(|value| matches!(value, Stage::Observation(_)))
                    .count()
            );
            result.result_json = value.to_string();
        }
        Ok(ProgramRun::Complete(result))
    }
}
