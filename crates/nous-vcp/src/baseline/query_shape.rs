use super::{positive, unit};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReferenceRiverNode {
    pub id: i64,
    pub energy: f64,
    pub normalized_energy: f64,
    pub hop: i64,
}
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReferenceRiverEdge {
    pub source_id: i64,
    pub target_id: i64,
    pub flow: f64,
    pub normalized_flow: f64,
}
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReferenceRiverShapeInput {
    pub river_nodes: Vec<ReferenceRiverNode>,
    pub river_edges: Vec<ReferenceRiverEdge>,
    pub complete_observation: bool,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReferenceMorphology {
    pub atomic_weight: f64,
    pub propositional_weight: f64,
    pub narrative_weight: f64,
    pub confidence: f64,
    pub effective_depth: f64,
    pub depth_variance: f64,
    pub energy_concentration: f64,
    pub shallow_energy_ratio: f64,
    pub forward_flow_ratio: f64,
    pub same_level_flow_ratio: f64,
    pub chainness: f64,
    pub branching: f64,
    pub merging: f64,
    pub growth_persistence: f64,
    pub dominant_mode: String,
}
#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(default, rename_all = "camelCase")]
pub struct ReferenceOmegaConfig {
    pub kappa_edge: f64,
    pub kappa_ratio: f64,
    pub omega_epsilon: f64,
    pub collapsed_threshold: f64,
    pub sparse_threshold: f64,
}
impl Default for ReferenceOmegaConfig {
    fn default() -> Self {
        Self {
            kappa_edge: 0.5,
            kappa_ratio: 0.3,
            omega_epsilon: 0.02,
            collapsed_threshold: 0.12,
            sparse_threshold: 0.45,
        }
    }
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReferenceOmega {
    pub omega: f64,
    pub omega_edge: f64,
    pub omega_emerge: f64,
    pub omega_flow: f64,
    pub regime: String,
    pub active_edges: usize,
    pub seed_nodes: usize,
    pub reached_nodes: usize,
    pub emergent_nodes: usize,
    pub complete_observation: bool,
}

pub fn reference_omega(
    input: &ReferenceRiverShapeInput,
    config: &ReferenceOmegaConfig,
) -> ReferenceOmega {
    let seeds = input.river_nodes.iter().filter(|n| n.hop == 0).count();
    let reached = input.river_nodes.len();
    let emerge = reached.saturating_sub(seeds);
    let edges = input.river_edges.len();
    let edge = unit(edges as f64 / (config.kappa_edge * seeds.max(1) as f64));
    let emergence = unit(emerge as f64 / (config.kappa_ratio * seeds.max(1) as f64));
    let flows = input
        .river_edges
        .iter()
        .map(|e| positive(e.flow))
        .filter(|f| *f > 0.0)
        .collect::<Vec<_>>();
    let flow = match flows.len() {
        0 => 0.0,
        1 => 0.5,
        _ => {
            let total: f64 = flows.iter().sum();
            unit(
                flows
                    .iter()
                    .map(|f| {
                        let p = f / total;
                        -p * p.ln()
                    })
                    .sum::<f64>()
                    / (flows.len() as f64).ln(),
            )
        }
    };
    let geometric = (edge.max(config.omega_epsilon)
        * emergence.max(config.omega_epsilon)
        * flow.max(config.omega_epsilon))
    .cbrt();
    let omega = unit(geometric * if input.complete_observation { 1.0 } else { 0.5 });
    let regime = if omega < config.collapsed_threshold {
        "collapsed"
    } else if omega < config.sparse_threshold {
        "sparse"
    } else {
        "dense"
    };
    ReferenceOmega {
        omega,
        omega_edge: edge,
        omega_emerge: emergence,
        omega_flow: flow,
        regime: regime.into(),
        active_edges: edges,
        seed_nodes: seeds,
        reached_nodes: reached,
        emergent_nodes: emerge,
        complete_observation: input.complete_observation,
    }
}

struct FlowStructure {
    forward: f64,
    same: f64,
    chain_fit: f64,
    branching: f64,
    merging: f64,
}
fn flow_structure(input: &ReferenceRiverShapeInput, hops: &BTreeMap<i64, usize>) -> FlowStructure {
    let mut incoming = BTreeMap::<i64, usize>::new();
    let mut outgoing = BTreeMap::<i64, usize>::new();
    let mut flow = 0.0;
    let mut forward = 0.0;
    let mut same = 0.0;
    for edge in &input.river_edges {
        let value = positive(if edge.normalized_flow != 0.0 {
            edge.normalized_flow
        } else {
            edge.flow
        });
        if value <= 0.0 {
            continue;
        }
        flow += value;
        *outgoing.entry(edge.source_id).or_default() += 1;
        *incoming.entry(edge.target_id).or_default() += 1;
        let a = hops.get(&edge.source_id).copied().unwrap_or(0);
        let b = hops.get(&edge.target_id).copied().unwrap_or(0);
        if b > a {
            forward += value;
        } else if a == b {
            same += value;
        }
    }
    let reached = input
        .river_nodes
        .iter()
        .filter(|n| n.hop > 0)
        .map(|n| n.id)
        .collect::<Vec<_>>();
    let count = reached.len().max(1) as f64;
    FlowStructure {
        forward: if flow > 1e-12 {
            unit(forward / flow)
        } else {
            0.0
        },
        same: if flow > 1e-12 { unit(same / flow) } else { 0.0 },
        chain_fit: reached
            .iter()
            .filter(|id| {
                incoming.get(id).copied().unwrap_or(0) <= 1
                    && outgoing.get(id).copied().unwrap_or(0) <= 1
            })
            .count() as f64
            / count,
        branching: unit(
            reached
                .iter()
                .map(|id| outgoing.get(id).copied().unwrap_or(0).saturating_sub(1))
                .sum::<usize>() as f64
                / count,
        ),
        merging: unit(
            reached
                .iter()
                .map(|id| incoming.get(id).copied().unwrap_or(0).saturating_sub(1))
                .sum::<usize>() as f64
                / count,
        ),
    }
}

pub fn reference_morphology(input: &ReferenceRiverShapeInput) -> ReferenceMorphology {
    let nodes = &input.river_nodes;
    let energies = nodes
        .iter()
        .map(|n| {
            positive(if n.normalized_energy != 0.0 {
                n.normalized_energy
            } else {
                n.energy
            })
        })
        .collect::<Vec<_>>();
    let total: f64 = energies.iter().sum();
    let hops = nodes
        .iter()
        .map(|n| (n.id, n.hop.max(0) as usize))
        .collect::<BTreeMap<_, _>>();
    let mean = if total > 1e-12 {
        nodes
            .iter()
            .zip(&energies)
            .map(|(n, e)| n.hop.max(0) as f64 * e)
            .sum::<f64>()
            / total
    } else {
        0.0
    };
    let variance = if total > 1e-12 {
        nodes
            .iter()
            .zip(&energies)
            .map(|(n, e)| e * (n.hop.max(0) as f64 - mean).powi(2))
            .sum::<f64>()
            / total
    } else {
        0.0
    };
    let depth = unit(1.0 - (-mean / 1.75).exp());
    let spread = unit(1.0 - (-variance.sqrt() / 1.5).exp());
    let shallow = if total > 1e-12 {
        nodes
            .iter()
            .zip(&energies)
            .filter(|(n, _)| n.hop <= 1)
            .map(|(_, e)| *e)
            .sum::<f64>()
            / total
    } else {
        1.0
    };
    let hhi = if total > 1e-12 {
        energies.iter().map(|e| (e / total).powi(2)).sum::<f64>()
    } else {
        1.0
    };
    let uniform = 1.0 / nodes.len().max(1) as f64;
    let concentration = if nodes.len() > 1 {
        unit((hhi - uniform) / (1.0 - uniform))
    } else {
        1.0
    };
    let flow = flow_structure(input, &hops);
    let chain = unit(
        flow.chain_fit * flow.forward.sqrt() * (0.35 + 0.65 * depth) * (1.0 - 0.5 * flow.branching),
    );
    let max_hop = hops.values().copied().max().unwrap_or(0);
    let occupied = hops.values().copied().collect::<BTreeSet<_>>();
    let growth = unit(
        if max_hop > 0 {
            occupied.len().saturating_sub(1) as f64 / max_hop as f64
        } else {
            0.0
        } * depth.sqrt(),
    );
    let relational = unit(0.35 * flow.same + 0.35 * flow.branching + 0.3 * flow.merging);
    let middle = unit(1.0 - (2.0 * depth - 1.0).abs());
    let confidence = unit(
        unit(
            (1.0 - (-(nodes.len() as f64) / 8.0).exp())
                * (1.0 - (-(input.river_edges.len() as f64) / 8.0).exp()),
        )
        .sqrt()
            * if input.complete_observation { 1.0 } else { 0.5 },
    );
    let logits = [
        1.45 * shallow + 0.9 * concentration - 1.25 * depth - 0.65 * growth - 0.45 * chain,
        1.25 * relational + 0.7 * middle + 0.35 * spread - 0.25 * chain,
        1.4 * depth + 1.15 * chain + 0.8 * flow.forward + 0.65 * growth
            - 0.65 * flow.branching
            - 0.3 * concentration,
    ];
    let maximum = logits.into_iter().fold(f64::NEG_INFINITY, f64::max);
    let exponentials = logits.map(|l| (l - maximum).exp());
    let sum: f64 = exponentials.iter().sum();
    let weights = exponentials.map(|p| confidence * p / sum.max(1e-12) + (1.0 - confidence) / 3.0);
    let mode = if weights[0] >= weights[1] && weights[0] >= weights[2] {
        "atomic"
    } else if weights[2] >= weights[1] {
        "narrative"
    } else {
        "propositional"
    };
    ReferenceMorphology {
        atomic_weight: weights[0],
        propositional_weight: weights[1],
        narrative_weight: weights[2],
        confidence,
        effective_depth: depth,
        depth_variance: spread,
        energy_concentration: concentration,
        shallow_energy_ratio: shallow,
        forward_flow_ratio: flow.forward,
        same_level_flow_ratio: flow.same,
        chainness: chain,
        branching: flow.branching,
        merging: flow.merging,
        growth_persistence: growth,
        dominant_mode: mode.into(),
    }
}
