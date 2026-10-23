// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::{
    ReferenceCurve, ReferenceProvenanceEdge, ReferenceRiverShapeInput, anchors::reference_cosine,
};
use super::{positive, unit};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Deserialize)]
pub struct ReferenceTopologyInput {
    pub curve: ReferenceCurve,
    pub file_id: i64,
    pub river: ReferenceRiverShapeInput,
    pub query_tag_vectors: Vec<(i64, Vec<f32>)>,
    pub provenance: Vec<ReferenceProvenanceEdge>,
    pub config: ReferenceTopologyConfig,
}
#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(default, rename_all = "camelCase")]
pub struct ReferenceTopologyConfig {
    pub semantic_node_threshold: f64,
    pub relative_distance_temperature: f64,
    pub reverse_direction_credit: f64,
    pub minimum_river_edge_flow: f64,
    pub maximum_river_edges: usize,
    pub node_only_reliability_cap: f64,
}
impl Default for ReferenceTopologyConfig {
    fn default() -> Self {
        Self {
            semantic_node_threshold: 0.48,
            relative_distance_temperature: 0.35,
            reverse_direction_credit: 0.25,
            minimum_river_edge_flow: 0.015,
            maximum_river_edges: 96,
            node_only_reliability_cap: 0.2,
        }
    }
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReferenceRelativeTopology {
    pub score: f64,
    pub reliability: f64,
    pub reliability_mode: String,
    pub node_alignment_score: f64,
    pub edge_graph_score: f64,
    pub node_graph_score: f64,
    pub relative_distance_score: f64,
    pub direction_score: f64,
    pub edge_topology_score: f64,
    pub motif_score: f64,
    pub matched_node_coverage: f64,
    pub matched_edge_coverage: f64,
    pub mean_closure: f64,
    pub matched_nodes: usize,
    pub matched_edges: usize,
    pub query_nodes: usize,
    pub query_edges: usize,
}
struct Alignment {
    index: usize,
    position: i64,
    quality: f64,
    closure: f64,
}
fn align(input: &ReferenceTopologyInput, id: i64, query: Option<&[f32]>) -> Option<Alignment> {
    if let Some((index, tag)) = input
        .curve
        .tags
        .iter()
        .enumerate()
        .find(|(_, t)| t.id == id)
    {
        let closure = unit(reference_cosine(&tag.vector, &input.curve.chunk_vector));
        return Some(Alignment {
            index,
            position: tag.position,
            quality: closure.sqrt(),
            closure,
        });
    }
    input
        .curve
        .tags
        .iter()
        .enumerate()
        .filter_map(|(index, tag)| {
            let similarity = query.map_or(0.0, |q| unit(reference_cosine(q, &tag.vector)));
            if similarity < input.config.semantic_node_threshold {
                return None;
            }
            let closure = unit(reference_cosine(&tag.vector, &input.curve.chunk_vector));
            let semantic = unit(
                (similarity - input.config.semantic_node_threshold)
                    / (1.0 - input.config.semantic_node_threshold).max(1e-9),
            );
            Some(Alignment {
                index,
                position: tag.position,
                quality: (semantic * closure).sqrt(),
                closure,
            })
        })
        .max_by(|a, b| {
            a.quality
                .total_cmp(&b.quality)
                .then_with(|| b.index.cmp(&a.index))
        })
}
fn independence(input: &ReferenceTopologyInput, from: i64, to: i64) -> f64 {
    let Some(edge) = input
        .provenance
        .iter()
        .find(|e| e.source_id == from && e.target_id == to)
    else {
        return 1.0;
    };
    let total: f64 = edge.file_contributions.iter().map(|(_, mass)| *mass).sum();
    if total <= 0.0 {
        return 1.0;
    }
    let own: f64 = edge
        .file_contributions
        .iter()
        .filter(|(id, _)| *id == input.file_id)
        .map(|(_, mass)| *mass)
        .sum();
    unit(1.0 - own / total).max(0.15)
}

pub fn reference_relative_topology(input: &ReferenceTopologyInput) -> ReferenceRelativeTopology {
    let mut output = ReferenceRelativeTopology {
        query_nodes: input.river.river_nodes.len(),
        ..Default::default()
    };
    let matches = node_alignments(input, &mut output);
    let hops = input
        .river
        .river_nodes
        .iter()
        .map(|n| (n.id, n.hop))
        .collect::<BTreeMap<_, _>>();
    let maximum = input
        .river
        .river_nodes
        .iter()
        .map(|n| n.hop.max(0))
        .max()
        .unwrap_or(1)
        .max(1) as f64;
    let min_position = input.curve.tags.first().map_or(0, |t| t.position);
    let max_position = input.curve.tags.last().map_or(min_position, |t| t.position);
    let span = (max_position - min_position).max(1) as f64;
    let edges = input
        .river
        .river_edges
        .iter()
        .filter(|e| {
            unit(if e.normalized_flow != 0.0 {
                e.normalized_flow
            } else {
                e.flow
            }) >= input.config.minimum_river_edge_flow
        })
        .take(input.config.maximum_river_edges.max(1))
        .collect::<Vec<_>>();
    output.query_edges = edges.len();
    let mut total = 0.0;
    let mut matched = 0.0;
    let mut distance = 0.0;
    let mut direction = 0.0;
    let mut topology = 0.0;
    for edge in edges {
        let mass = positive(if edge.normalized_flow != 0.0 {
            edge.normalized_flow
        } else {
            edge.flow
        })
        .max(1e-9);
        total += mass;
        let (Some(a), Some(b)) = (matches.get(&edge.source_id), matches.get(&edge.target_id))
        else {
            continue;
        };
        if a.index == b.index {
            continue;
        }
        let ahop = hops.get(&edge.source_id).copied().unwrap_or(0);
        let bhop = hops.get(&edge.target_id).copied().unwrap_or(0);
        let qdistance = (bhop - ahop).abs().max(1) as f64 / maximum;
        let cdistance = (b.position - a.position).abs() as f64 / span;
        let d = (-(qdistance - cdistance).abs()
            / input.config.relative_distance_temperature.max(1e-6))
        .exp();
        let orientation = if b.position > a.position {
            1.0
        } else {
            unit(input.config.reverse_direction_credit)
        };
        let value = unit(
            (a.quality * b.quality).sqrt()
                * d
                * orientation
                * independence(input, edge.source_id, edge.target_id),
        );
        matched += mass;
        distance += mass * d;
        direction += mass * orientation;
        topology += mass * value;
        output.matched_edges += 1;
    }
    output.matched_edge_coverage = if total > 0.0 {
        unit(matched / total)
    } else {
        0.0
    };
    if matched > 0.0 {
        output.relative_distance_score = unit(distance / matched);
        output.direction_score = unit(direction / matched);
        output.edge_topology_score = unit(topology / matched);
    }
    output.motif_score = output.edge_topology_score;
    output.edge_graph_score = unit(
        0.18 * output.node_alignment_score
            + 0.22 * output.relative_distance_score
            + 0.18 * output.direction_score
            + 0.28 * output.edge_topology_score
            + 0.14 * output.motif_score,
    );
    output.node_graph_score = unit((output.node_alignment_score * output.mean_closure).sqrt());
    if output.matched_edges > 0 {
        output.score = output.edge_graph_score;
        output.reliability = unit(
            (output.matched_node_coverage * output.matched_edge_coverage * output.mean_closure)
                .cbrt(),
        );
        output.reliability_mode = "edge_topology".into();
    } else if output.matched_nodes > 0 {
        output.score = output.node_graph_score;
        output.reliability = input.config.node_only_reliability_cap.min(unit(
            (output.matched_node_coverage * output.mean_closure).sqrt(),
        ));
        output.reliability_mode = "node_alignment_fallback".into();
    } else {
        output.reliability_mode = "unavailable".into();
    }
    output
}

fn node_alignments(
    input: &ReferenceTopologyInput,
    output: &mut ReferenceRelativeTopology,
) -> BTreeMap<i64, Alignment> {
    let vectors = input
        .query_tag_vectors
        .iter()
        .map(|(id, v)| (*id, v.as_slice()))
        .collect::<BTreeMap<_, _>>();
    let mut matches = BTreeMap::new();
    let mut node_mass = 0.0;
    let mut matched_mass = 0.0;
    let mut quality = 0.0;
    for node in &input.river.river_nodes {
        let mass = positive(if node.normalized_energy != 0.0 {
            node.normalized_energy
        } else {
            node.energy
        })
        .max(1e-9);
        node_mass += mass;
        if let Some(alignment) = align(input, node.id, vectors.get(&node.id).copied()) {
            matched_mass += mass;
            quality += mass * alignment.quality;
            matches.insert(node.id, alignment);
        }
    }
    output.matched_nodes = matches.len();
    output.matched_node_coverage = if node_mass > 0.0 {
        unit(matched_mass / node_mass)
    } else {
        0.0
    };
    output.node_alignment_score = if matched_mass > 0.0 {
        unit(quality / matched_mass)
    } else {
        0.0
    };
    output.mean_closure = if matches.is_empty() {
        0.0
    } else {
        matches.values().map(|m| m.closure).sum::<f64>() / matches.len() as f64
    };
    matches
}
