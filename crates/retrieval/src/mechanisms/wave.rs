// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use crate::*;
use std::collections::{BTreeSet, HashMap};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceSeed {
    pub node: u32,
    pub weight: f64,
    pub seed_family: String,
    pub origin_cue: String,
    pub hop_zero: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SeedFamily {
    Explicit,
    Exact,
    Entity,
    Tag,
    Schema,
    Resource,
    Runtime,
    Relation,
    Residual,
}
impl SeedFamily {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Explicit => "explicit_query",
            Self::Exact => "exact_target",
            Self::Entity => "entity_cue",
            Self::Tag => "tag_cue",
            Self::Schema => "schema_cue",
            Self::Resource => "resource_cue",
            Self::Runtime => "runtime_situation",
            Self::Relation => "relation_cue",
            Self::Residual => "residual_discovery",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SeedOrigin {
    ExplicitQuery,
    ExactTarget,
    EntityCue,
    TagCue,
    SchemaCue,
    ResourceCue,
    RuntimeSituation,
    RelationCue,
    ResidualDiscovery,
}
impl SeedOrigin {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::ExplicitQuery => "explicit_query",
            Self::ExactTarget => "exact_target",
            Self::EntityCue => "entity_cue",
            Self::TagCue => "tag_cue",
            Self::SchemaCue => "schema_cue",
            Self::ResourceCue => "resource_cue",
            Self::RuntimeSituation => "runtime_situation",
            Self::RelationCue => "relation_cue",
            Self::ResidualDiscovery => "residual_discovery",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WeightedCognitiveSeed {
    pub node: u32,
    pub weight: f64,
    pub family: SeedFamily,
    pub origin: SeedOrigin,
    pub provenance: Option<String>,
    pub embedding_space: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RiverEdgeFlow {
    pub from: u32,
    pub to: u32,
    pub flow: f64,
    pub max_single_hop_flow: f64,
    pub minimum_hop: usize,
    pub immediate_return: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeWaveProvenance {
    pub node: u32,
    pub potential: f64,
    pub first_hop: usize,
    pub emergent: bool,
    pub strongest_parent: Option<u32>,
    pub origin_seeds: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryRiver {
    pub source_field: SparseField,
    pub node_potential: SparseField,
    pub edges: Vec<RiverEdgeFlow>,
    pub provenance: Vec<NodeWaveProvenance>,
    pub total_edge_flow: f64,
    pub discarded_state_mass: f64,
    pub generated_state_mass: f64,
    pub complete: bool,
    pub max_hops: usize,
    pub max_hop_observed: usize,
}

#[derive(Debug, Clone)]
struct PropagationState {
    previous: Option<u32>,
    current: u32,
    hop: usize,
    remaining_budget_steps: u32,
    energy: f64,
    origins: BTreeSet<String>,
}

pub fn propagate(graph: &WaveGraphGeneration, seeds: &[SourceSeed]) -> QueryRiver {
    propagate_with_budget(graph, seeds, graph.config.max_hops, graph.config.max_states)
}
pub fn propagate_weighted(
    graph: &WaveGraphGeneration,
    seeds: &[WeightedCognitiveSeed],
) -> QueryRiver {
    propagate_weighted_with_budget(graph, seeds, graph.config.max_hops, graph.config.max_states)
}
pub fn propagate_weighted_with_budget(
    graph: &WaveGraphGeneration,
    seeds: &[WeightedCognitiveSeed],
    max_hops: usize,
    max_states: usize,
) -> QueryRiver {
    let converted = seeds
        .iter()
        .map(|seed| SourceSeed {
            node: seed.node,
            weight: seed.weight,
            seed_family: seed.family.as_str().into(),
            origin_cue: seed
                .provenance
                .clone()
                .unwrap_or_else(|| seed.origin.as_str().into()),
            hop_zero: true,
        })
        .collect::<Vec<_>>();
    propagate_with_budget(graph, &converted, max_hops, max_states)
}
#[expect(
    clippy::collapsible_if,
    reason = "bounded Wave propagation keeps the pruning predicate readable"
)]
#[expect(
    clippy::too_many_lines,
    reason = "bounded Wave state propagation is one semantic kernel"
)]
pub fn propagate_with_budget(
    graph: &WaveGraphGeneration,
    seeds: &[SourceSeed],
    max_hops: usize,
    max_states: usize,
) -> QueryRiver {
    let mut source_field = SparseField::new();
    let total = seeds
        .iter()
        .filter(|seed| seed.weight.is_finite() && seed.weight > 0.0)
        .map(|seed| seed.weight)
        .sum::<f64>();
    let mut states = Vec::new();
    if total > 0.0 {
        for seed in seeds
            .iter()
            .filter(|seed| seed.weight.is_finite() && seed.weight > 0.0)
        {
            let energy = seed.weight / total;
            *source_field.entry(seed.node).or_default() += energy;
            states.push(PropagationState {
                previous: None,
                current: seed.node,
                hop: 0,
                remaining_budget_steps: graph.config.initial_budget_steps,
                energy,
                origins: BTreeSet::from([seed.origin_cue.clone()]),
            });
        }
    }
    let max_hops = max_hops.min(graph.config.max_hops).max(1);
    let max_states = max_states.min(graph.config.max_states).max(1);
    let fir_sum = (0..=max_hops)
        .map(|hop| graph.config.fir_gamma.powi(hop as i32))
        .sum::<f64>();
    let mut potential = SparseField::new();
    let mut first = HashMap::new();
    let mut origins: HashMap<u32, BTreeSet<String>> = HashMap::new();
    for state in &states {
        *potential.entry(state.current).or_default() += state.energy / fir_sum;
        first.entry(state.current).or_insert(0);
        origins
            .entry(state.current)
            .or_default()
            .extend(state.origins.iter().cloned());
    }
    let mut edges = HashMap::<(u32, u32), RiverEdgeFlow>::new();
    let mut discarded = 0.0;
    let mut generated = states.iter().map(|state| state.energy).sum::<f64>();
    let mut complete = true;
    let mut max_hop_observed = 0;
    let mut parents = HashMap::<u32, (u32, f64)>::new();
    for hop in 0..max_hops {
        let mut next = HashMap::<(Option<u32>, u32, usize, u32), PropagationState>::new();
        for state in states.drain(..) {
            if state.hop != hop {
                continue;
            }
            let Some(remaining) = state
                .remaining_budget_steps
                .checked_sub(graph.config.normal_edge_cost)
            else {
                continue;
            };
            for (target, conductance) in graph.outgoing(state.current) {
                let immediate = state.previous == Some(target);
                let energy = state.energy
                    * conductance
                    * if immediate {
                        graph.config.immediate_return_multiplier
                    } else {
                        1.0
                    };
                if energy < graph.config.minimum_state_energy {
                    continue;
                }
                let next_state = PropagationState {
                    previous: Some(state.current),
                    current: target,
                    hop: hop + 1,
                    remaining_budget_steps: remaining,
                    energy,
                    origins: state.origins.clone(),
                };
                let key = (
                    next_state.previous,
                    next_state.current,
                    next_state.hop,
                    next_state.remaining_budget_steps,
                );
                next.entry(key)
                    .and_modify(|existing| {
                        existing.energy += next_state.energy;
                        existing.origins.extend(next_state.origins.iter().cloned());
                    })
                    .or_insert(next_state);
                let edge = edges
                    .entry((state.current, target))
                    .or_insert(RiverEdgeFlow {
                        from: state.current,
                        to: target,
                        flow: 0.0,
                        max_single_hop_flow: 0.0,
                        minimum_hop: hop + 1,
                        immediate_return: immediate,
                    });
                edge.flow += energy;
                edge.max_single_hop_flow = edge.max_single_hop_flow.max(energy);
                edge.minimum_hop = edge.minimum_hop.min(hop + 1);
                edge.immediate_return |= immediate;
                generated += energy;
            }
        }
        let mut next = next.into_values().collect::<Vec<_>>();
        next.sort_by(|left, right| {
            right
                .energy
                .total_cmp(&left.energy)
                .then_with(|| left.current.cmp(&right.current))
                .then_with(|| {
                    left.remaining_budget_steps
                        .cmp(&right.remaining_budget_steps)
                })
                .then_with(|| left.previous.cmp(&right.previous))
                .then_with(|| left.origins.cmp(&right.origins))
        });
        if next.len() > max_states {
            complete = false;
            discarded += next
                .drain(max_states..)
                .map(|state| state.energy)
                .sum::<f64>();
        }
        for state in &next {
            max_hop_observed = max_hop_observed.max(state.hop);
            let weight = graph.config.fir_gamma.powi(state.hop as i32) / fir_sum;
            *potential.entry(state.current).or_default() += state.energy * weight;
            let entry = first.entry(state.current).or_insert(state.hop);
            *entry = (*entry).min(state.hop);
            origins
                .entry(state.current)
                .or_default()
                .extend(state.origins.iter().cloned());
            if let Some(parent) = state.previous {
                if parents.get(&state.current).is_none_or(|(previous, mass)| {
                    *mass < state.energy || (*mass == state.energy && parent < *previous)
                }) {
                    parents.insert(state.current, (parent, state.energy));
                }
            }
        }
        states = next;
        if states.is_empty() {
            break;
        }
    }
    if !states.is_empty() {
        complete = false;
    }
    let mut provenance = potential
        .iter()
        .map(|(&node, &value)| NodeWaveProvenance {
            node,
            potential: value,
            first_hop: first.get(&node).copied().unwrap_or(max_hops),
            emergent: !source_field.contains_key(&node),
            strongest_parent: parents.get(&node).map(|(parent, _)| *parent),
            origin_seeds: origins
                .remove(&node)
                .unwrap_or_default()
                .into_iter()
                .collect(),
        })
        .collect::<Vec<_>>();
    provenance.sort_by_key(|value| value.node);
    let mut edges = edges.into_values().collect::<Vec<_>>();
    edges.sort_by_key(|value| (value.from, value.to));
    let total_edge_flow = edges.iter().map(|value| value.flow).sum();
    QueryRiver {
        source_field,
        node_potential: potential,
        edges,
        total_edge_flow,
        provenance,
        discarded_state_mass: discarded,
        generated_state_mass: generated,
        complete,
        max_hops,
        max_hop_observed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nous_core::MemoryRevisionId;

    fn converging_graph(config: WaveConfig) -> WaveGraphGeneration {
        WaveGraphGeneration::from_artifact(TopologyArtifact {
            generation_id: nous_core::ServingGenerationId::new(),
            cognitive_profile: nous_runtime::CognitiveProfile::default(),
            nodes: (0..5)
                .map(|node| WaveNode {
                    serving_id: node,
                    reference: CognitiveRef::MemoryRevision(MemoryRevisionId::new()),
                    node_kind: WaveNodeKind::Memory,
                    embedding_key: None,
                    posting_key: None,
                    intrinsic_residual_gain: None,
                })
                .collect(),
            evidence: Vec::new(),
            edges: vec![
                (0, 2, 0.8, 1.0),
                (1, 2, 0.8, 1.0),
                (2, 3, 0.8, 1.0),
                (3, 4, 0.8, 1.0),
            ],
            config,
        })
        .expect("converging transport")
    }
    fn seed(node: u32, origin: &str) -> SourceSeed {
        SourceSeed {
            node,
            weight: 1.0,
            seed_family: "exact_target".into(),
            origin_cue: origin.into(),
            hop_zero: true,
        }
    }

    #[test]
    fn remaining_budget_prevents_unaffordable_transitions() {
        let graph = converging_graph(WaveConfig {
            initial_budget_steps: 1,
            ..Default::default()
        });
        let river = propagate(&graph, &[seed(0, "a")]);
        assert!(river.node_potential.contains_key(&2));
        assert!(!river.node_potential.contains_key(&3));
        assert!(river.complete);
        let graph = converging_graph(WaveConfig {
            initial_budget_steps: 1,
            normal_edge_cost: 2,
            ..Default::default()
        });
        let river = propagate(&graph, &[seed(0, "a")]);
        assert_eq!(river.node_potential.len(), 1);
        assert!(river.edges.is_empty());
    }

    #[test]
    fn merged_paths_keep_all_source_origins_downstream() {
        let graph = converging_graph(WaveConfig::default());
        for seeds in [
            vec![seed(0, "a"), seed(1, "b")],
            vec![seed(1, "b"), seed(0, "a")],
        ] {
            let river = propagate(&graph, &seeds);
            for node in [3, 4] {
                let origins = &river
                    .provenance
                    .iter()
                    .find(|p| p.node == node)
                    .expect("downstream node")
                    .origin_seeds;
                assert_eq!(origins, &["a".to_owned(), "b".to_owned()]);
            }
        }
    }

    #[test]
    fn state_budget_is_part_of_merge_and_truncation_is_reported() {
        let root = CognitiveRef::MemoryRevision(MemoryRevisionId::new());
        let left = CognitiveRef::MemoryRevision(MemoryRevisionId::new());
        let right = CognitiveRef::MemoryRevision(MemoryRevisionId::new());
        let graph = WaveGraphGeneration::build(
            vec![
                WaveNode {
                    serving_id: 0,
                    reference: root.clone(),
                    node_kind: WaveNodeKind::Memory,
                    embedding_key: None,
                    posting_key: None,
                    intrinsic_residual_gain: None,
                },
                WaveNode {
                    serving_id: 1,
                    reference: left.clone(),
                    node_kind: WaveNodeKind::Memory,
                    embedding_key: None,
                    posting_key: None,
                    intrinsic_residual_gain: None,
                },
                WaveNode {
                    serving_id: 2,
                    reference: right.clone(),
                    node_kind: WaveNodeKind::Memory,
                    embedding_key: None,
                    posting_key: None,
                    intrinsic_residual_gain: None,
                },
            ],
            &[
                WaveEdgeEvidence {
                    from: root.clone(),
                    to: left,
                    basis_class: "host_explicit".into(),
                    association_kind: "assoc.related".into(),
                    polarity: "positive".into(),
                    support_mass: 1.0,
                    provenance_root: None,
                },
                WaveEdgeEvidence {
                    from: root,
                    to: right,
                    basis_class: "host_explicit".into(),
                    association_kind: "assoc.related".into(),
                    polarity: "positive".into(),
                    support_mass: 1.0,
                    provenance_root: None,
                },
            ],
            WaveConfig {
                max_states: 1,
                ..WaveConfig::default()
            },
        )
        .expect("graph");
        let river = propagate(
            &graph,
            &[SourceSeed {
                node: 0,
                weight: 1.0,
                seed_family: "exact_target".into(),
                origin_cue: "test".into(),
                hop_zero: true,
            }],
        );
        assert!(!river.complete);
        assert!(river.discarded_state_mass > 0.0);
    }
}
