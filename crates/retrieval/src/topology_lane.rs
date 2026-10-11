// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use crate::observation::NATIVE_MECHANISM_ID;
use crate::{QueryObservation, ServingSnapshot, SourceSeed};
use nous_core::EvidenceFamily;
use nous_runtime::{
    BoundQuery, LaneCandidate, LaneOutput, LaneStatus, QueryPlan, TopologyWorkSummary,
};
use std::collections::BTreeMap;

pub(crate) fn topology_lane(
    snapshot: &ServingSnapshot,
    bound: &BoundQuery,
    plan: &QueryPlan,
    signals: &crate::PreparedQuerySignals,
) -> nous_core::Result<LaneOutput> {
    let mut output = LaneOutput::empty(EvidenceFamily::TopologyWave, LaneStatus::Ready);
    if !plan.expand_topology {
        output.status = LaneStatus::Disabled;
        return Ok(output);
    }
    let Some(graph) = snapshot.topology.as_ref() else {
        output.status = LaneStatus::Unavailable;
        output
            .diagnostics
            .push("topology serving generation is unavailable".into());
        return Ok(output);
    };
    if graph.cognitive_profile != plan.cognitive_profile {
        output.status = LaneStatus::Unavailable;
        output
            .diagnostics
            .push("topology generation does not match bound cognitive profile".into());
        return Ok(output);
    }
    if plan.cognitive_profile.requirements().query_embedding && signals.embedding().is_none() {
        output.status = LaneStatus::Unavailable;
        output
            .diagnostics
            .push("bound cognitive profile requires an available permitted query embedding".into());
        return Ok(output);
    }
    if plan.cognitive_profile != nous_runtime::CognitiveProfile::NousNodePotential {
        output.status = LaneStatus::Unavailable;
        output.diagnostics.push(format!(
            "cognitive kernel not implemented: {}",
            plan.cognitive_profile.id()
        ));
        return Ok(output);
    }
    output.generation_ref = Some(graph.generation_id);
    let readout = crate::resolve_wave_readout_policy(&bound.config_snapshot)?;
    let seeds = prepared_source_seeds(graph, bound, signals, &readout);
    let observation = QueryObservation::native(graph, bound, plan, seeds)?;
    let river = observation.river();
    output.topology_work = Some(TopologyWorkSummary {
        mechanism: NATIVE_MECHANISM_ID.into(),
        profile_id: observation.profile_id().into(),
        profile_digest: observation.profile_digest().into(),
        activated_edges: river.edges.len(),
        max_hop_observed: river.max_hop_observed,
        seed_count: observation.source_seeds().len(),
        visited_nodes: river.node_potential.len(),
        complete: river.complete,
        discarded_mass: Some(river.discarded_state_mass),
    });
    if !river.complete {
        output.status = LaneStatus::Truncated;
    }
    let routes = crate::activated_routes::activated_routes(
        observation
            .source_seeds()
            .iter()
            .filter(|seed| seed.weight > 0.0)
            .map(|seed| seed.node),
        river
            .edges
            .iter()
            .filter(|edge| edge.flow > 0.0)
            .map(|edge| (edge.from, edge.to)),
        river.max_hops,
    );
    let mut values = river
        .node_potential
        .iter()
        .filter_map(|(node, potential)| {
            graph
                .nodes
                .get(*node as usize)
                .map(|value| (value.reference.clone(), *potential, *node))
        })
        .collect::<Vec<_>>();
    values.sort_by(|left, right| {
        right
            .1
            .total_cmp(&left.1)
            .then_with(|| left.0.to_string().cmp(&right.0.to_string()))
    });
    output.candidates = values
        .into_iter()
        .take(plan.lane_budget(EvidenceFamily::TopologyWave))
        .enumerate()
        .map(|(index, (reference, potential, node))| LaneCandidate {
            reference,
            rank: (index + 1) as u32,
            variants: vec!["topology:wave".into()],
            provider_metadata: serde_json::json!({
                "mechanism": NATIVE_MECHANISM_ID,
                "node_potential": potential,
                "complete": river.complete,
                "activated_route": routes.get(&node).map(|path| path.iter().map(|id|
                    graph.nodes[*id as usize].reference.to_string()).collect::<Vec<_>>()),
                "route_hop_limit": river.max_hops.min(32),
                "route_seed": routes.get(&node).and_then(|path| path.first()).map(|seed_node|
                    observation.source_seeds().iter().filter(|seed| seed.node == *seed_node).collect::<Vec<_>>()),
                "route_evidence": routes.get(&node).map(|path| path.windows(2).map(|pair| {
                    let basis = graph.edge_evidence(pair[0], pair[1]).take(16).collect::<Vec<_>>();
                    serde_json::json!({
                        "from": graph.nodes[pair[0] as usize].reference,
                        "to": graph.nodes[pair[1] as usize].reference,
                        "flow": river.edges.iter().find(|edge| edge.from == pair[0] && edge.to == pair[1]).map(|edge| edge.flow),
                        "support": basis,
                        "support_truncated": graph.edge_evidence(pair[0], pair[1]).count() > 16,
                    })
                }).collect::<Vec<_>>()),
                "route_semantics": "shortest witnessed route through activated edges; not score attribution",
                "provenance": river.provenance.iter().find(|value| value.node == node),
            }),
        })
        .collect();
    Ok(output)
}

fn prepared_source_seeds(
    graph: &crate::WaveGraphGeneration,
    bound: &BoundQuery,
    signals: &crate::PreparedQuerySignals,
    readout: &crate::WaveReadoutPolicy,
) -> Vec<SourceSeed> {
    let mut seeds = source_seeds(graph, bound, readout);
    // Closed graph anchors lead associative queries. When none maps into the
    // graph, use the already-prepared retrieval signals as weak source cues.
    if seeds.is_empty() {
        for (lane, family) in [
            (signals.lexical(), "lexical_promoted"),
            (signals.dense(), "dense_promoted"),
        ] {
            if let Some(lane) = lane {
                seeds.extend(promoted_seeds(graph, lane, family, readout));
            }
        }
    }
    seeds
}

fn source_seeds(
    graph: &crate::WaveGraphGeneration,
    bound: &BoundQuery,
    readout: &crate::WaveReadoutPolicy,
) -> Vec<SourceSeed> {
    let seed_weight =
        |name: &str, default: f64| readout.seed_weights.get(name).copied().unwrap_or(default);
    let mut seeds = Vec::new();
    for seed in &bound.activation.seeds {
        if let Some(node) = graph.node_id(&seed.reference) {
            let default = match seed.origin.as_str() {
                "runtime_situation" => 0.85,
                "entity_cue" => 0.90,
                "tag_cue" | "existing_semantic_match" => 0.75,
                _ => 1.0,
            };
            seeds.push((
                node,
                seed_weight(&seed.origin, default) * seed.strength,
                seed.origin.as_str(),
            ));
        }
    }
    let mut merged = BTreeMap::<(u32, String), SourceSeed>::new();
    for (node, weight, family) in seeds {
        merged
            .entry((node, family.into()))
            .and_modify(|seed| seed.weight += weight)
            .or_insert(SourceSeed {
                node,
                weight,
                seed_family: family.into(),
                origin_cue: format!("{family}:{}", graph.nodes[node as usize].reference),
                hop_zero: true,
            });
    }
    merged.into_values().collect()
}

fn promoted_seeds(
    graph: &crate::WaveGraphGeneration,
    lane: &LaneOutput,
    family: &str,
    readout: &crate::WaveReadoutPolicy,
) -> Vec<SourceSeed> {
    if !matches!(lane.status, LaneStatus::Ready | LaneStatus::Truncated) {
        return Vec::new();
    }
    let weight = readout.seed_weights.get(family).copied().unwrap_or(0.6);
    let mut seen = std::collections::BTreeSet::new();
    lane.candidates
        .iter()
        .filter_map(|candidate| {
            let node = graph.node_id(&candidate.reference)?;
            if candidate.rank == 0 || !seen.insert(node) {
                return None;
            }
            Some(SourceSeed {
                node,
                weight: weight / f64::from(candidate.rank),
                seed_family: family.into(),
                origin_cue: format!("{family}:{}", candidate.reference),
                hop_zero: true,
            })
        })
        .take(4.min(graph.build_policy.max_neighbors_per_node))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{WaveBuildPolicy, WaveGraphGeneration, WaveNode, WaveNodeKind, WaveReadoutPolicy};
    use nous_core::{CognitiveRef, MemoryRevisionId};
    #[test]
    fn promotion_uses_only_bounded_current_graph_signals_and_configured_weight() {
        let references = (0..8)
            .map(|_| CognitiveRef::MemoryRevision(MemoryRevisionId::new()))
            .collect::<Vec<_>>();
        let mut readout = WaveReadoutPolicy::default();
        readout.seed_weights.insert("lexical_promoted".into(), 0.4);
        let graph = WaveGraphGeneration::build(
            references
                .iter()
                .enumerate()
                .map(|(id, reference)| WaveNode {
                    serving_id: id as u32,
                    reference: reference.clone(),
                    node_kind: WaveNodeKind::Memory,
                    embedding_key: None,
                    posting_key: None,
                    intrinsic_residual_gain: None,
                })
                .collect(),
            &[],
            WaveBuildPolicy::default(),
        )
        .unwrap();
        let mut lane = LaneOutput::empty(EvidenceFamily::Lexical, LaneStatus::Ready);
        lane.candidates = references
            .iter()
            .enumerate()
            .map(|(index, reference)| LaneCandidate {
                reference: reference.clone(),
                rank: index as u32 + 1,
                variants: Vec::new(),
                provider_metadata: serde_json::Value::Null,
            })
            .collect();
        lane.candidates.insert(
            0,
            LaneCandidate {
                reference: CognitiveRef::MemoryRevision(MemoryRevisionId::new()),
                rank: 1,
                variants: Vec::new(),
                provider_metadata: serde_json::Value::Null,
            },
        );
        let seeds = promoted_seeds(&graph, &lane, "lexical_promoted", &readout);
        assert_eq!(seeds.len(), 4);
        assert_eq!(seeds[0].weight, 0.4);
        assert_eq!(seeds[1].weight, 0.2);
        lane.status = LaneStatus::Unavailable;
        assert!(promoted_seeds(&graph, &lane, "lexical_promoted", &readout).is_empty());
    }
}
