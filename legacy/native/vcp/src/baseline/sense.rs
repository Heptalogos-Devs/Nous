use super::positive;
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Deserialize)]
pub struct ReferenceSenseGraph {
    pub node_ids: Vec<i64>,
    pub edges: Vec<(i64, i64, f64)>,
    #[serde(default)]
    pub anchor_gain: Vec<(i64, f64)>,
    #[serde(default)]
    pub wormholes: Vec<(i64, i64)>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReferenceSenseSeed {
    pub id: i64,
    pub energy: f64,
    pub source_type: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(default, rename_all = "camelCase")]
pub struct ReferenceSenseConfig {
    pub max_safe_hops: usize,
    pub base_momentum: f64,
    pub firing_threshold: f64,
    pub base_decay: f64,
    pub wormhole_decay: f64,
    pub tension_threshold: f64,
    pub max_neighbors_per_node: usize,
    pub return_flow_factor: f64,
    pub fir_gamma: f64,
    pub max_propagation_states: usize,
    pub minimum_injected_current: f64,
    pub max_output_nodes: usize,
    pub max_output_edges: usize,
    pub max_transition_records: usize,
}
impl Default for ReferenceSenseConfig {
    fn default() -> Self {
        Self {
            max_safe_hops: 4,
            base_momentum: 2.0,
            firing_threshold: 0.1,
            base_decay: 0.25,
            wormhole_decay: 0.7,
            tension_threshold: 1.0,
            max_neighbors_per_node: 20,
            return_flow_factor: 0.15,
            fir_gamma: 0.6,
            max_propagation_states: 2000,
            minimum_injected_current: 0.01,
            max_output_nodes: 0,
            max_output_edges: 0,
            max_transition_records: 0,
        }
    }
}
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReferenceSenseInput {
    pub seeds: Vec<ReferenceSenseSeed>,
    pub config: ReferenceSenseConfig,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReferenceSenseNode {
    pub id: i64,
    pub energy: f64,
    pub normalized_energy: f64,
    pub hop: usize,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReferenceSenseEdge {
    pub source_id: i64,
    pub target_id: i64,
    pub flow: f64,
    pub max_flow: f64,
    pub normalized_flow: f64,
    pub conductance: f64,
    pub min_hop: usize,
    pub wormhole: bool,
    pub immediate_return: bool,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReferenceSenseTransition {
    pub source_id: i64,
    pub target_id: i64,
    pub previous_id: Option<i64>,
    pub hop: usize,
    pub flow: f64,
    pub wormhole: bool,
    pub immediate_return: bool,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReferenceSenseDiagnostics {
    pub reached_nodes: usize,
    pub active_edges: usize,
    pub maximum_node_energy: f64,
    pub maximum_edge_flow: f64,
    pub return_flow_suppressed_mass: f64,
    pub state_truncations: usize,
    pub hop_in_flight_mass: Vec<f64>,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReferenceSenseOutput {
    pub source_field: Vec<(i64, f64)>,
    pub nodes: Vec<ReferenceSenseNode>,
    pub edges: Vec<ReferenceSenseEdge>,
    pub transitions: Vec<ReferenceSenseTransition>,
    pub transitions_truncated: bool,
    pub diagnostics: ReferenceSenseDiagnostics,
}
#[derive(Clone)]
struct State {
    energy: f64,
    momentum: f64,
}

/// Neutral numerical Sense contract. Merged states do not claim a unique root
/// lineage: frozen VCP's first-encounter lineage is nondeterministic.
#[expect(
    clippy::too_many_lines,
    reason = "Finite-hop state admission, edge accounting and FIR accumulation form one reference solver"
)]
pub fn reference_sense(
    graph: &ReferenceSenseGraph,
    input: &ReferenceSenseInput,
) -> Result<ReferenceSenseOutput> {
    let config = &input.config;
    let ids = graph.node_ids.iter().copied().collect::<BTreeSet<_>>();
    if ids.len() != graph.node_ids.len()
        || ids.iter().any(|id| *id <= 0)
        || graph
            .edges
            .iter()
            .any(|(a, b, w)| !ids.contains(a) || !ids.contains(b) || !w.is_finite() || *w < 0.0)
    {
        return Err(Error::Invalid("invalid reference Sense graph".into()));
    }
    let mut rows = BTreeMap::<i64, Vec<(i64, f64)>>::new();
    for (from, to, weight) in &graph.edges {
        rows.entry(*from).or_default().push((*to, *weight));
    }
    for row in rows.values_mut() {
        row.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        row.truncate(config.max_neighbors_per_node.max(1));
    }
    let anchors = graph
        .anchor_gain
        .iter()
        .copied()
        .collect::<BTreeMap<_, _>>();
    let wormholes = graph.wormholes.iter().copied().collect::<BTreeSet<_>>();
    let gamma = config.fir_gamma.clamp(0.05, 0.95);
    let mut fir = (0..=config.max_safe_hops)
        .map(|hop| gamma.powi(hop as i32))
        .collect::<Vec<_>>();
    let fir_total: f64 = fir.iter().sum();
    for weight in &mut fir {
        *weight /= fir_total;
    }
    let mut active = BTreeMap::<(Option<i64>, i64), State>::new();
    let mut potential = BTreeMap::<i64, f64>::new();
    let mut first = BTreeMap::<i64, usize>::new();
    for seed in &input.seeds {
        if seed.energy <= 0.0 || !ids.contains(&seed.id) {
            continue;
        }
        let energy = positive(seed.energy);
        active
            .entry((None, seed.id))
            .and_modify(|s| s.energy += energy)
            .or_insert(State {
                energy,
                momentum: config.base_momentum,
            });
        *potential.entry(seed.id).or_default() += energy * fir[0];
        first.entry(seed.id).or_insert(0);
    }
    let mut edges = BTreeMap::<(i64, i64), ReferenceSenseEdge>::new();
    let mut transitions = Vec::new();
    let mut transitions_truncated = false;
    let mut suppressed = 0.0;
    let mut truncations = 0;
    let mut hop_mass = Vec::new();
    let transition_limit = config.max_transition_records.min(16000);
    for hop in 0..config.max_safe_hops {
        let mut next = BTreeMap::<(Option<i64>, i64), State>::new();
        let mut transfers = Vec::new();
        for ((previous, node), state) in &active {
            if state.energy < config.firing_threshold || state.momentum < 0.0 {
                continue;
            }
            for (target, conductance) in rows.get(node).into_iter().flatten() {
                if *conductance <= 0.0 {
                    continue;
                }
                let wormhole = wormholes.contains(&(*node, *target))
                    || conductance * anchors.get(target).copied().unwrap_or(1.0)
                        >= config.tension_threshold;
                let immediate = *previous == Some(*target);
                let raw = state.energy
                    * conductance
                    * if wormhole {
                        config.wormhole_decay
                    } else {
                        config.base_decay
                    };
                let flow = raw
                    * if immediate {
                        config.return_flow_factor.clamp(0.0, 1.0)
                    } else {
                        1.0
                    };
                if immediate {
                    suppressed += positive(raw - flow);
                }
                if flow < config.minimum_injected_current {
                    continue;
                }
                let edge = edges.entry((*node, *target)).or_insert(ReferenceSenseEdge {
                    source_id: *node,
                    target_id: *target,
                    flow: 0.0,
                    max_flow: 0.0,
                    normalized_flow: 0.0,
                    conductance: *conductance,
                    min_hop: hop + 1,
                    wormhole,
                    immediate_return: immediate,
                });
                edge.flow += flow;
                edge.max_flow = edge.max_flow.max(flow);
                edge.min_hop = edge.min_hop.min(hop + 1);
                edge.wormhole |= wormhole;
                edge.immediate_return |= immediate;
                let momentum = state.momentum - if wormhole { 0.0 } else { 1.0 };
                if momentum < 0.0 && !wormhole {
                    continue;
                }
                if transition_limit > 0 && transfers.len() < transition_limit {
                    transfers.push(ReferenceSenseTransition {
                        source_id: *node,
                        target_id: *target,
                        previous_id: *previous,
                        hop: hop + 1,
                        flow,
                        wormhole,
                        immediate_return: immediate,
                    });
                } else if transition_limit > 0 {
                    transitions_truncated = true;
                }
                next.entry((Some(*node), *target))
                    .and_modify(|s| {
                        s.energy += flow;
                        s.momentum = s.momentum.max(momentum);
                    })
                    .or_insert(State {
                        energy: flow,
                        momentum,
                    });
            }
        }
        let cap = config.max_propagation_states.max(100);
        if next.len() > cap {
            let mut ranked = next.into_iter().collect::<Vec<_>>();
            ranked.sort_by(|a, b| {
                b.1.energy
                    .total_cmp(&a.1.energy)
                    .then_with(|| a.0.cmp(&b.0))
            });
            truncations += ranked.len() - cap;
            ranked.truncate(cap);
            next = ranked.into_iter().collect();
        }
        transfers.retain(|t| next.contains_key(&(Some(t.source_id), t.target_id)));
        transfers.sort_by(|a, b| {
            b.flow
                .total_cmp(&a.flow)
                .then_with(|| a.source_id.cmp(&b.source_id))
                .then_with(|| a.target_id.cmp(&b.target_id))
                .then_with(|| a.previous_id.cmp(&b.previous_id))
        });
        let remaining = transition_limit.saturating_sub(transitions.len());
        if transfers.len() > remaining {
            transitions_truncated = true;
            transfers.truncate(remaining);
        }
        transitions.extend(transfers);
        let mut energies = BTreeMap::<i64, f64>::new();
        let mut mass = 0.0;
        for ((_, node), state) in &next {
            *energies.entry(*node).or_default() += state.energy;
            mass += state.energy;
            first.entry(*node).or_insert(hop + 1);
        }
        hop_mass.push(mass);
        let mut propagated = false;
        for (node, energy) in energies {
            *potential.entry(node).or_default() += energy * fir[hop + 1];
            propagated |= energy > config.minimum_injected_current;
        }
        if !propagated {
            break;
        }
        active = next;
    }
    let maximum_node_energy = potential.values().copied().fold(0.0, f64::max);
    let maximum_edge_flow = edges.values().map(|e| e.flow).fold(0.0, f64::max);
    let mut nodes = potential
        .into_iter()
        .map(|(id, energy)| ReferenceSenseNode {
            id,
            energy,
            normalized_energy: if maximum_node_energy > 0.0 {
                energy / maximum_node_energy
            } else {
                0.0
            },
            hop: first[&id],
        })
        .collect::<Vec<_>>();
    nodes.sort_by(|a, b| b.energy.total_cmp(&a.energy).then_with(|| a.id.cmp(&b.id)));
    if config.max_output_nodes > 0 {
        nodes.truncate(config.max_output_nodes);
    }
    let total: f64 = nodes.iter().map(|n| n.energy).sum();
    let source_field = nodes
        .iter()
        .filter(|n| n.energy > 0.0)
        .map(|n| (n.id, n.energy / total))
        .collect();
    let mut edges = edges.into_values().collect::<Vec<_>>();
    for edge in &mut edges {
        edge.normalized_flow = if maximum_edge_flow > 0.0 {
            edge.flow / maximum_edge_flow
        } else {
            0.0
        };
    }
    edges.sort_by(|a, b| {
        b.flow
            .total_cmp(&a.flow)
            .then_with(|| (a.source_id, a.target_id).cmp(&(b.source_id, b.target_id)))
    });
    if config.max_output_edges > 0 {
        edges.truncate(config.max_output_edges);
    }
    Ok(ReferenceSenseOutput {
        source_field,
        diagnostics: ReferenceSenseDiagnostics {
            reached_nodes: nodes.len(),
            active_edges: edges.len(),
            maximum_node_energy,
            maximum_edge_flow,
            return_flow_suppressed_mass: suppressed,
            state_truncations: truncations,
            hop_in_flight_mass: hop_mass,
        },
        nodes,
        edges,
        transitions,
        transitions_truncated,
    })
}
