//! Build ordered observation output from propagated energy and flows.
use super::*;

pub(super) struct ObservationData {
    pub nodes: Vec<ObservationNode>,
    pub edges: Vec<ObservationEdge>,
    pub source_field: Vec<(i64, f64)>,
    pub maximum_node_energy: f64,
    pub maximum_edge_flow: f64,
}

pub(super) fn build_observation(
    artifact: &NativeArtifact,
    config: &SenseConfig,
    accumulated: &HashMap<usize, f64>,
    provenance: &HashMap<usize, Provenance>,
    parents: &HashMap<usize, Parent>,
    edge_flows: HashMap<(usize, usize), EdgeFlow>,
) -> ObservationData {
    let maximum_node_energy = accumulated.values().copied().fold(0.0, f64::max);
    let maximum_edge_flow = edge_flows
        .values()
        .map(|edge| edge.flow)
        .fold(0.0, f64::max);

    let mut nodes: Vec<ObservationNode> = accumulated
        .iter()
        .map(|(&index, &energy)| {
            let meta = provenance.get(&index);
            ObservationNode {
                id: artifact.node_ids[index],
                energy,
                normalized_energy: if maximum_node_energy > 0.0 {
                    energy / maximum_node_energy
                } else {
                    0.0
                },
                source_type: meta
                    .map(|value| value.source_type.clone())
                    .unwrap_or_else(|| "unknown".to_string()),
                origin_type: meta.and_then(|value| value.origin_type.clone()),
                hop: meta.map(|value| value.hop).unwrap_or(0),
                seed_id: meta.and_then(|value| value.seed_id),
                strongest_parent: parents.get(&index).map(|parent| ObservationParent {
                    parent_id: parent.parent_id,
                    flow: parent.flow,
                    hop: parent.hop,
                    wormhole: parent.wormhole,
                }),
            }
        })
        .collect();
    nodes.sort_by(|left, right| {
        right
            .energy
            .partial_cmp(&left.energy)
            .unwrap_or(Ordering::Equal)
            .then_with(|| left.id.cmp(&right.id))
    });
    if config.max_output_nodes > 0 {
        nodes.truncate(config.max_output_nodes);
    }

    let mut edges: Vec<ObservationEdge> = edge_flows
        .into_values()
        .map(|edge| ObservationEdge {
            source_id: edge.source_id,
            target_id: edge.target_id,
            flow: edge.flow,
            max_flow: edge.max_flow,
            normalized_flow: if maximum_edge_flow > 0.0 {
                edge.flow / maximum_edge_flow
            } else {
                0.0
            },
            conductance: edge.conductance,
            min_hop: edge.min_hop,
            wormhole: edge.wormhole,
            immediate_return: edge.immediate_return,
        })
        .collect();
    edges.sort_by(|left, right| {
        right
            .flow
            .partial_cmp(&left.flow)
            .unwrap_or(Ordering::Equal)
            .then_with(|| left.source_id.cmp(&right.source_id))
            .then_with(|| left.target_id.cmp(&right.target_id))
    });
    if config.max_output_edges > 0 {
        edges.truncate(config.max_output_edges);
    }

    let total_mass: f64 = nodes.iter().map(|node| node.energy).sum();
    let source_field = nodes
        .iter()
        .filter(|node| node.id > 0 && node.energy > 0.0)
        .map(|node| {
            (
                node.id,
                if total_mass > 0.0 {
                    node.energy / total_mass
                } else {
                    0.0
                },
            )
        })
        .collect();

    ObservationData {
        nodes,
        edges,
        source_field,
        maximum_node_energy,
        maximum_edge_flow,
    }
}
