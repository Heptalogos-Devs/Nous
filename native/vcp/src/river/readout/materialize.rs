//! Convert ranked RiverMemo candidates into result values.
use super::*;

pub(super) struct ResultContext<'a> {
    pub input: &'a NativeInput,
    pub core_tag_names: &'a HashSet<String>,
    pub mode: &'a str,
    pub omega: &'a OmegaOutput,
    pub wormhole_traces: &'a HashMap<i64, Value>,
    pub original_score_by_id: &'a HashMap<i64, f64>,
    pub path_diagnostics: HashMap<i64, Vec<Value>>,
}

pub(super) fn materialize_results(
    work: Vec<ScoredWork>,
    context: ResultContext<'_>,
) -> Vec<NativeResultItem> {
    let ResultContext {
        input,
        core_tag_names,
        mode,
        omega,
        wormhole_traces,
        original_score_by_id,
        mut path_diagnostics,
    } = context;
    work
        .into_iter()
        .take(input.top_k.max(1))
        .enumerate()
        .map(|(index, item)| {
            // Same set as before, listed in curve order instead of hash order.
            let mut seen_names = HashSet::new();
            let matched_tags: Vec<String> = item
                .curve
                .tags
                .iter()
                .filter(|tag| seen_names.insert(tag.name.as_str()))
                .map(|tag| tag.name.clone())
                .collect();
            let core_tags_matched = matched_tags
                .iter()
                .filter(|name| core_tag_names.contains(&name.to_lowercase()))
                .cloned()
                .collect();
            let topology_v3 = if input.include_trace {
                Some(serde_json::json!({
                    "mode": "conservative_candidate_self_topology_v2.2",
                    "branchNodeCoverage": clamp01(item.topology.matched_node_coverage / 0.35),
                    "branchEdgeCoverage": clamp01(item.topology.matched_edge_coverage / 0.20),
                    "branchCoverage": branch_coverage(&item.topology, mode),
                    "branchConfidence": branch_confidence(&item.topology, mode),
                    "strongestNodeMatchQuality": item.topology.strongest_node_match_quality,
                    "precisionGrounding": precision_grounding(&item.topology),
                    "fieldSupport": item.observables.field_support,
                    "fieldResonanceBeforeSupport": field_resonance(&item.curve, &input, 1.0).2,
                    "queryScore": clamp01(item.curve.query_score),
                    "fieldScore": item.field_score,
                    "fieldResonanceBonus": item.field_resonance_bonus,
                    "directEvidence": item.direct_evidence,
                    "omega": omega.omega,
                    "regime": omega.regime,
                    "omegaEdge": omega.omega_edge,
                    "omegaEmerge": omega.omega_emerge,
                    "omegaFlow": omega.omega_flow,
                    "omegaGamma": input.config.omega_gamma,
                    "graphGate": omega.omega.powf(input.config.omega_gamma.max(0.0)),
                    "v2Bonus": item.v2_bonus,
                    "gatedV2Bonus": item.gated_bonus,
                    "anchorScore": item.anchor.score,
                    "anchorReliability": item.anchor.reliability,
                    "anchorStrength": item.anchor.strength,
                    "implicitOnlyAnchor": item.anchor.implicit_only,
                    "explicitExactContacts": item.anchor.explicit_exact_contacts,
                    "explicitSemanticContacts": item.anchor.explicit_semantic_contacts,
                    "hasExplicitFocus": item.anchor.has_explicit_focus,
                    "anchorBonus": item.anchor_bonus,
                    "contactedSeeds": item.anchor.contacted_seeds,
                    "exactContacts": item.anchor.exact_contacts,
                    "semanticContacts": item.anchor.semantic_contacts,
                    "meanClosure": item.anchor.mean_closure,
                    "role": item.role,
                    "pureScore": item.pure_score,
                    "pathBonus": item.path_bonus,
                    "graphBonus": item.gated_bonus,
                    "requestedGraphBonus": item.v2_bonus * omega.omega.powf(input.config.omega_gamma.max(0.0)),
                    "finalScore": item.final_score,
                    "nativeBackend": "rust-rayon"
                })).map(|mut trace| {
                    trace["wormholeRrf"] = wormhole_traces
                        .get(&item.curve.id).cloned().unwrap_or(Value::Null);
                    trace
                })
            } else {
                None
            };
            NativeResultItem {
                chosen_path:item.curve.path_index,
                available_paths:path_diagnostics.remove(&item.curve.id).unwrap_or_default(),
                id: item.curve.id,
                chunk_id: item.curve.id,
                rank: index + 1,
                score: item.final_score,
                ranking_score: item.ranking_score,
                base_score: item.pure_score,
                field_score: item.field_score,
                field_resonance_bonus: item.field_resonance_bonus,
                topology_bonus: item.path_bonus + item.gated_bonus,
                anchor_bonus: item.anchor_bonus,
                path_bonus: item.path_bonus,
                graph_bonus: item.gated_bonus,
                score_clipping: positive(item.pure_score + item.path_bonus + item.gated_bonus
                    + item.anchor_bonus - wormhole_traces.get(&item.curve.id)
                        .and_then(|v| v["baselineScore"].as_f64()).unwrap_or(item.final_score)),
                role: item.role,
                omega: omega.omega,
                river_regime: omega.regime.clone(),
                matched_tags,
                core_tags_matched,
                candidate_sources: item.curve.sources,
                original_score: original_score_by_id
                    .get(&item.curve.id)
                    .copied()
                    .unwrap_or(0.0),
                topology_v3,
                relative_topology: input.include_trace.then_some(item.topology),
                geometry: input.include_trace.then_some(item.geometry),
                observables: input.include_trace.then_some(item.observables),
            }
        })
        .collect()
}
