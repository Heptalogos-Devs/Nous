//! Auxiliary evidence from observed wormholes, never a replacement scorer.
//! Strict global admission enables a separate correction key; baseline scores stay intact.
use super::*;

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Route {
    source_id: i64,
    target_id: i64,
    witness_pairs: Vec<(Vec<i64>, Vec<i64>)>,
    flow: f64,
    relative_flow: f64,
    #[serde(skip)]
    vector: Arc<[f32]>,
}

#[derive(Default)]
pub(super) struct Model {
    pub routes: Vec<Route>,
    pub eligible_routes: usize,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct GateDecision {
    source_id: i64,
    target_id: i64,
    witness_file_ids: Vec<i64>,
    accepted: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    refined_evidence: Option<Value>,
}

pub(super) fn gate(
    model: &mut Model,
    connection: &MemoAssets,
) -> Result<Vec<GateDecision>, String> {
    let mut decisions = Vec::new();
    for route in &model.routes {
        let roots: Vec<i64> = connection
            .root_facts
            .iter()
            .filter(|(_, fact)| {
                fact.tags.contains(&route.source_id) && fact.tags.contains(&route.target_id)
            })
            .map(|(id, _)| *id)
            .collect();
        decisions.push(GateDecision {
            source_id: route.source_id,
            target_id: route.target_id,
            accepted: roots.len() >= 2,
            witness_file_ids: roots,
            refined_evidence: None,
        });
    }
    model.routes.retain(|r| {
        decisions
            .iter()
            .any(|d| d.source_id == r.source_id && d.target_id == r.target_id && d.accepted)
    });
    Ok(decisions)
}
pub(super) fn refine(
    model: &mut Model,
    decisions: &mut [GateDecision],
    connection: &MemoAssets,
    input: &NativeInput,
) -> Result<(), String> {
    let focus: HashSet<i64> = input
        .query_state
        .field_provenance
        .iter()
        .filter(|p| p.hop == 0 && matches!(p.source_type.as_str(), "core" | "lexical_seed"))
        .map(|p| p.id)
        .collect();
    for d in decisions.iter_mut().filter(|d| d.accepted) {
        let truncated = d.witness_file_ids.len() > 128;
        let mut witness = None;
        let mut witness_pairs = Vec::new();
        if !truncated {
            for (i, left_id) in d.witness_file_ids.iter().enumerate() {
                for right_id in d.witness_file_ids.iter().skip(i + 1) {
                    let left = &connection.root_facts[left_id];
                    let right = &connection.root_facts[right_id];
                    if left.checksum.is_empty()
                        || right.checksum.is_empty()
                        || left.checksum == right.checksum
                        || left
                            .required_roots
                            .iter()
                            .any(|r| right.required_roots.contains(r))
                        || left.tags.len() > 128
                        || right.tags.len() > 128
                    {
                        continue;
                    }
                    let anchor = focus
                        .iter()
                        .copied()
                        .filter(|id| {
                            *id != d.source_id
                                && *id != d.target_id
                                && (left.tags.contains(id) || right.tags.contains(id))
                        })
                        .min();
                    let bridge = left
                        .tags
                        .intersection(&right.tags)
                        .copied()
                        .filter(|id| {
                            *id != d.source_id && *id != d.target_id && !focus.contains(id)
                        })
                        .min();
                    if let (Some(anchor), Some(bridge)) = (anchor, bridge) {
                        witness = Some(
                            serde_json::json!({"rootIds":[left_id,right_id],"focusId":anchor,"bridgeTagId":bridge}),
                        );
                        witness_pairs
                            .push((left.required_roots.clone(), right.required_roots.clone()));
                    }
                }
            }
        }
        for route in &mut model.routes {
            if route.source_id == d.source_id && route.target_id == d.target_id {
                route.witness_pairs = witness_pairs.clone();
            }
        }
        d.accepted = witness.is_some();
        d.refined_evidence = Some(serde_json::json!({"witness":witness,"truncated":truncated}));
    }
    model.routes.retain(|r| {
        decisions
            .iter()
            .any(|d| d.source_id == r.source_id && d.target_id == r.target_id && d.accepted)
    });
    Ok(())
}
pub(super) fn build(
    input: &NativeInput,
    vectors: &TagVectorPool,
    artifact: &NativeArtifact,
) -> Model {
    // Use the same denominator as the observation display: maximum observed
    // positive raw flow, not a candidate-pool statistic or a second saturation.
    let maximum = input
        .query_state
        .river_edges
        .iter()
        .map(|edge| positive(edge.flow))
        .fold(0.0_f64, f64::max);
    if maximum == 0.0 {
        return Model::default();
    }
    let mut routes = BTreeMap::<(i64, i64), Route>::new();
    for edge in &input.query_state.river_edges {
        if !edge.wormhole
            || edge.immediate_return
            || edge.source_id <= 0
            || edge.target_id <= 0
            || edge.source_id == edge.target_id
            || positive(edge.flow) == 0.0
            || artifact.edge_weight(edge.source_id, edge.target_id) <= 0.0
        {
            continue;
        }
        let Some(vector) = vectors.get(&edge.target_id) else {
            continue;
        };
        if !vector.iter().all(|v| v.is_finite()) || squared_norm(vector) <= 1e-15 {
            continue;
        }
        // Dynamic routing also selects real CSR edges. Neither the persisted
        // wormhole flag nor a source-target similarity is re-tested here.
        // Duplicate serialized edges cannot increase a route's vote.
        let key = (edge.source_id, edge.target_id);
        let route = Route {
            source_id: edge.source_id,
            target_id: edge.target_id,
            witness_pairs: vec![],
            flow: edge.flow,
            relative_flow: clamp01(edge.flow / maximum),
            vector: vector.clone(),
        };
        if routes.get(&key).is_none_or(|old| old.flow < route.flow) {
            routes.insert(key, route);
        }
    }
    let mut routes: Vec<_> = routes.into_values().collect();
    routes.sort_by(|a, b| {
        b.flow
            .total_cmp(&a.flow)
            .then_with(|| a.source_id.cmp(&b.source_id))
            .then_with(|| a.target_id.cmp(&b.target_id))
    });
    let eligible_routes = routes.len();
    routes.truncate(input.config.maximum_river_edges.max(1));
    Model {
        routes,
        eligible_routes,
    }
}

#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Correction {
    pub eligible: bool,
    pub evidence: f64,
    pub source_id: Option<i64>,
    pub target_id: Option<i64>,
    pub matched_tag_id: Option<i64>,
    pub tag_similarity: f64,
    pub body_cosine: f64,
    pub relative_flow: f64,
}

pub(super) fn correction(
    model: &Model,
    curve: &Curve,
    input: &NativeInput,
    omega: &OmegaOutput,
) -> Correction {
    let mut best = Correction::default();
    if !input.query_state.complete_observation || omega.omega <= 0.0 {
        return best;
    }
    let explicit: HashSet<i64> = input
        .query_state
        .field_provenance
        .iter()
        .filter(|p| p.hop == 0 && matches!(p.source_type.as_str(), "core" | "lexical_seed"))
        .map(|p| p.id)
        .collect();
    for route in &model.routes {
        // Original observation denominator, never renormalized after admission.
        if route.relative_flow < 0.5 || !explicit.contains(&route.source_id) {
            continue;
        }
        if !route.witness_pairs.iter().any(|(a, b)| {
            !a.iter().any(|r| curve.source_ids.contains(r))
                && !b.iter().any(|r| curve.source_ids.contains(r))
        }) {
            continue;
        }
        if curve.multi_path
            && !curve.tags.iter().any(|a| {
                a.id == route.source_id
                    && a.position > 0
                    && curve
                        .tags
                        .iter()
                        .any(|b| b.id == route.target_id && b.position > a.position)
            })
        {
            continue;
        }
        let body = clamp01(cosine(&route.vector, &curve.chunk_vector));
        if body <= 0.0 {
            continue;
        }
        let matched = curve
            .tags
            .iter()
            .filter_map(|tag| {
                let similarity = if tag.id == route.target_id {
                    1.0
                } else {
                    cosine(&route.vector, &tag.vector)
                };
                (similarity > input.config.semantic_node_threshold && tag.chunk_cosine > 0.0)
                    .then_some((tag.id, similarity, tag.chunk_cosine))
            })
            .max_by(|a, b| a.1.total_cmp(&b.1).then_with(|| b.0.cmp(&a.0)));
        let Some((tag, similarity, _closure)) = matched else {
            continue;
        };
        let evidence =
            route.relative_flow * body * omega.omega.powf(input.config.omega_gamma.max(0.0));
        if best.eligible && evidence <= best.evidence {
            continue;
        }
        best = Correction {
            eligible: true,
            evidence,
            source_id: Some(route.source_id),
            target_id: Some(route.target_id),
            matched_tag_id: Some(tag),
            tag_similarity: similarity,
            body_cosine: body,
            relative_flow: route.relative_flow,
        };
    }
    best
}

/// `work` must already be sorted by the untouched baseline comparator.
#[derive(Default)]
pub(super) struct RrfFusion {
    pub active: bool,
    pub traces: HashMap<i64, Value>,
}

pub(super) fn fuse_rrf(
    work: &mut [ScoredWork],
    model: &Model,
    input: &NativeInput,
    omega: &OmegaOutput,
) -> RrfFusion {
    const K: f64 = 60.0;
    let ordinary_max = input
        .query_state
        .river_edges
        .iter()
        .filter(|e| !e.wormhole && !e.immediate_return && e.source_id != e.target_id)
        .map(|e| positive(e.flow))
        .fold(0.0_f64, f64::max);
    // With no ordinary edge use the original observation maximum, not epsilon.
    let reference = if ordinary_max > 0.0 {
        ordinary_max
    } else {
        input
            .query_state
            .river_edges
            .iter()
            .map(|e| positive(e.flow))
            .fold(0.0_f64, f64::max)
    };
    let mut traces = HashMap::new();
    for item in work.iter_mut() {
        item.ranking_score = item.final_score;
    }
    if reference <= 0.0 || work.is_empty() {
        return RrfFusion {
            active: false,
            traces,
        };
    }
    let mut contributions = vec![0.0_f64; work.len()];
    let mut winners = vec![Value::Null; work.len()];
    for route in &model.routes {
        let single = Model {
            routes: vec![route.clone()],
            eligible_routes: 1,
        };
        let mut ranked: Vec<(usize, Correction)> = work
            .iter()
            .enumerate()
            .filter_map(|(i, item)| {
                let evidence = correction(&single, &item.curve, input, omega);
                evidence.eligible.then_some((i, evidence))
            })
            .collect();
        ranked.sort_by(|a, b| {
            b.1.evidence
                .total_cmp(&a.1.evidence)
                .then_with(|| a.0.cmp(&b.0))
        });
        let weight = route.flow / reference * omega.omega.powf(input.config.omega_gamma.max(0.0));
        for (rank, (i, evidence)) in ranked.into_iter().enumerate() {
            let contribution = weight / (K + (rank + 1) as f64);
            if contribution <= contributions[i] {
                continue;
            }
            contributions[i] = contribution;
            winners[i] = serde_json::json!({
                "sourceId": route.source_id, "targetId": route.target_id,
                "flow": route.flow, "relativeFlow": route.relative_flow,
                "referenceFlow": reference, "weight": weight,
                "routeRank": rank + 1, "contribution": contribution,
                "grounding": evidence
            });
        }
    }
    let active = contributions.iter().any(|v| *v > 0.0);
    for (i, item) in work.iter_mut().enumerate() {
        let baseline = item.final_score;
        let baseline_rrf = 1.0 / (K + (i + 1) as f64);
        item.ranking_score = if active {
            baseline_rrf + contributions[i]
        } else {
            baseline
        };
        traces.insert(
            item.curve.id,
            serde_json::json!({
                "active": active, "k": K, "baselineRank": i + 1,
                "baselineScore": baseline, "baselineContribution": baseline_rrf,
                "wormholeContribution": contributions[i], "winner": winners[i],
                "fusedScore": item.ranking_score,
                "referenceMode": if ordinary_max > 0.0 { "maximum-observed-ordinary-flow" }
                    else { "maximum-observed-flow-no-ordinary-edge" }
            }),
        );
    }
    RrfFusion { active, traces }
}

#[cfg(test)]
#[path = "../../tests/support/wormhole_contract.rs"]
mod score_contract_tests;
