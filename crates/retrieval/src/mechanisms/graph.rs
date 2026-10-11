// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use crate::*;
use nous_core::{Error, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WaveNodeKind {
    Memory,
    Tag,
    Schema,
    Entity,
    Resource,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WaveNode {
    pub serving_id: u32,
    pub reference: CognitiveRef,
    pub node_kind: WaveNodeKind,
    pub embedding_key: Option<u64>,
    pub posting_key: Option<u64>,
    pub intrinsic_residual_gain: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WaveEdgeEvidence {
    pub from: CognitiveRef,
    pub to: CognitiveRef,
    pub basis_class: String,
    pub association_kind: String,
    pub polarity: String,
    pub support_mass: f64,
    #[serde(default)]
    pub provenance_root: Option<String>,
}

#[derive(Debug, Clone)]
pub struct WaveGraphGeneration {
    pub generation_id: ServingGenerationId,
    pub cognitive_profile: nous_runtime::CognitiveProfile,
    pub nodes: Vec<WaveNode>,
    node_by_ref: std::collections::HashMap<CognitiveRef, u32>,
    adjacency: Vec<Vec<(u32, f64)>>,
    raw_support: std::collections::HashMap<(u32, u32), f64>,
    evidence: std::collections::BTreeMap<(u32, u32), Vec<WaveEdgeEvidence>>,
    pub config: WaveConfig,
}

impl WaveGraphGeneration {
    pub fn artifact(&self) -> TopologyArtifact {
        let mut edges = Vec::new();
        for (from, row) in self.adjacency.iter().enumerate() {
            for (to, weight) in row {
                edges.push((
                    from as u32,
                    *to,
                    *weight,
                    *self.raw_support.get(&(from as u32, *to)).unwrap_or(&0.0),
                ));
            }
        }
        TopologyArtifact {
            generation_id: self.generation_id,
            cognitive_profile: self.cognitive_profile,
            nodes: self.nodes.clone(),
            evidence: self.evidence.values().flatten().cloned().collect(),
            edges,
            config: self.config.clone(),
        }
    }

    pub fn from_artifact(artifact: TopologyArtifact) -> Result<Self> {
        artifact.config.validate()?;
        let mut adjacency = vec![Vec::new(); artifact.nodes.len()];
        let mut raw_support = std::collections::HashMap::new();
        for (from, to, weight, raw) in artifact.edges {
            if from as usize >= artifact.nodes.len()
                || to as usize >= artifact.nodes.len()
                || !weight.is_finite()
                || weight < 0.0
                || !raw.is_finite()
            {
                return Err(Error::Infrastructure("topology artifact is corrupt".into()));
            }
            adjacency[from as usize].push((to, weight));
            raw_support.insert((from, to), raw);
        }
        let node_by_ref = artifact
            .nodes
            .iter()
            .map(|node| (node.reference.clone(), node.serving_id))
            .collect::<std::collections::HashMap<_, _>>();
        let evidence = index_evidence(artifact.evidence, &node_by_ref);
        Ok(Self {
            generation_id: artifact.generation_id,
            cognitive_profile: artifact.cognitive_profile,
            nodes: artifact.nodes,
            evidence,
            node_by_ref,
            adjacency,
            raw_support,
            config: artifact.config,
        })
    }

    pub fn build(
        mut nodes: Vec<WaveNode>,
        evidence: &[WaveEdgeEvidence],
        config: WaveConfig,
    ) -> Result<Self> {
        config.validate()?;
        nodes.sort_by_key(|node| node.serving_id);
        for (index, node) in nodes.iter_mut().enumerate() {
            node.serving_id = index as u32;
        }
        let node_by_ref = nodes
            .iter()
            .map(|node| (node.reference.clone(), node.serving_id))
            .collect::<std::collections::HashMap<_, _>>();
        let mut by_root = std::collections::HashMap::<(u32, u32, Option<String>), f64>::new();
        for item in evidence {
            if item.polarity.eq_ignore_ascii_case("negative")
                || !supported_relation(&item.association_kind)
                || !item.support_mass.is_finite()
                || item.support_mass <= 0.0
            {
                continue;
            }
            let (Some(&from), Some(&to)) = (node_by_ref.get(&item.from), node_by_ref.get(&item.to))
            else {
                continue;
            };
            let quality =
                class_quality(&item.basis_class, &config.class_quality) * item.support_mass;
            let entry = by_root
                .entry((from, to, item.provenance_root.clone()))
                .or_default();
            *entry = entry.max(quality);
        }
        let mut aggregate = std::collections::HashMap::<(u32, u32), f64>::new();
        for ((from, to, _root), mass) in by_root {
            *aggregate.entry((from, to)).or_default() += mass;
        }
        let mut inflow = vec![0.0; nodes.len()];
        for (&(_, to), mass) in &aggregate {
            inflow[to as usize] += *mass;
        }
        let mut positive = inflow
            .iter()
            .copied()
            .filter(|value| *value > 0.0)
            .collect::<Vec<_>>();
        positive.sort_by(f64::total_cmp);
        let median = positive
            .get(positive.len() / 2)
            .copied()
            .unwrap_or(1.0)
            .max(f64::EPSILON);
        let mut rows = vec![Vec::new(); nodes.len()];
        let mut raw_support = std::collections::HashMap::new();
        for (&(from, to), &raw) in &aggregate {
            let penalty = (inflow[to as usize] / median)
                .powf(-config.hub_beta)
                .clamp(config.hub_penalty_min, config.hub_penalty_max);
            rows[from as usize].push((to, raw * penalty, raw));
        }
        let mut adjacency = vec![Vec::new(); nodes.len()];
        for (from, row) in rows.into_iter().enumerate() {
            let mut row = row;
            row.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
            row.truncate(config.max_neighbors_per_node);
            let total: f64 = row.iter().map(|(_, weight, _)| *weight).sum();
            for (to, weight, raw) in row {
                let conductance = if total > 0.0 {
                    config.outbound_budget * weight / total
                } else {
                    0.0
                };
                if conductance > 0.0 {
                    adjacency[from].push((to, conductance));
                    raw_support.insert((from as u32, to), raw);
                }
            }
        }
        let retained_evidence = evidence
            .iter()
            .filter(|item| {
                let (Some(from), Some(to)) =
                    (node_by_ref.get(&item.from), node_by_ref.get(&item.to))
                else {
                    return false;
                };
                raw_support.contains_key(&(*from, *to))
                    && !item.polarity.eq_ignore_ascii_case("negative")
                    && supported_relation(&item.association_kind)
                    && item.support_mass.is_finite()
                    && item.support_mass > 0.0
            })
            .cloned()
            .collect();
        Ok(Self {
            generation_id: ServingGenerationId::new(),
            evidence: index_evidence(retained_evidence, &node_by_ref),
            cognitive_profile: nous_runtime::CognitiveProfile::default(),
            nodes,
            node_by_ref,
            adjacency,
            raw_support,
            config,
        })
    }

    pub fn edge_evidence(&self, from: u32, to: u32) -> impl Iterator<Item = &WaveEdgeEvidence> {
        self.evidence.get(&(from, to)).into_iter().flatten()
    }
    pub fn node_id(&self, reference: &CognitiveRef) -> Option<u32> {
        self.node_by_ref.get(reference).copied()
    }
    pub fn outgoing(&self, node: u32) -> Vec<(u32, f64)> {
        self.adjacency
            .get(node as usize)
            .cloned()
            .unwrap_or_default()
    }
    pub fn outbound_mass(&self, node: u32) -> f64 {
        self.outgoing(node).iter().map(|(_, mass)| *mass).sum()
    }
    pub fn edge_raw_support(&self, from: u32, to: u32) -> Option<f64> {
        self.raw_support.get(&(from, to)).copied()
    }
}

fn index_evidence(
    evidence: Vec<WaveEdgeEvidence>,
    nodes: &std::collections::HashMap<CognitiveRef, u32>,
) -> std::collections::BTreeMap<(u32, u32), Vec<WaveEdgeEvidence>> {
    let mut indexed = std::collections::BTreeMap::<_, Vec<_>>::new();
    for item in evidence {
        if let (Some(from), Some(to)) = (nodes.get(&item.from), nodes.get(&item.to)) {
            indexed.entry((*from, *to)).or_default().push(item);
        }
    }
    indexed
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TopologyArtifact {
    pub generation_id: ServingGenerationId,
    pub cognitive_profile: nous_runtime::CognitiveProfile,
    pub nodes: Vec<WaveNode>,
    pub edges: Vec<(u32, u32, f64, f64)>,
    pub evidence: Vec<WaveEdgeEvidence>,
    pub config: WaveConfig,
}

fn supported_relation(kind: &str) -> bool {
    matches!(
        kind,
        "cognition_basis"
            | "aboutness"
            | "tag_attachment"
            | "schema_support"
            | "derived_from"
            | "temporal_successor"
            | "elaborates"
            | "assoc.related"
            | "assoc.co_occurs"
            | "assoc.sequence"
            | "assoc.procedural"
            | "assoc.shared_outcome"
    )
}
fn class_quality(class: &str, values: &std::collections::BTreeMap<String, f64>) -> f64 {
    values.get(class).copied().unwrap_or(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use nous_core::MemoryRevisionId;

    fn node(id: CognitiveRef, serving_id: u32) -> WaveNode {
        WaveNode {
            serving_id,
            reference: id,
            node_kind: WaveNodeKind::Memory,
            embedding_key: None,
            posting_key: None,
            intrinsic_residual_gain: None,
        }
    }

    #[test]
    fn unknown_negative_and_contradictory_edges_do_not_enter_nonnegative_graph() {
        let a = CognitiveRef::MemoryRevision(MemoryRevisionId::new());
        let b = CognitiveRef::MemoryRevision(MemoryRevisionId::new());
        let graph = WaveGraphGeneration::build(
            vec![node(a.clone(), 0), node(b.clone(), 1)],
            &[
                WaveEdgeEvidence {
                    from: a.clone(),
                    to: b.clone(),
                    basis_class: "host_explicit".into(),
                    association_kind: "contradicts".into(),
                    polarity: "positive".into(),
                    support_mass: 1.0,
                    provenance_root: None,
                },
                WaveEdgeEvidence {
                    from: a.clone(),
                    to: b.clone(),
                    basis_class: "host_explicit".into(),
                    association_kind: "assoc.related".into(),
                    polarity: "negative".into(),
                    support_mass: 1.0,
                    provenance_root: None,
                },
            ],
            WaveConfig::default(),
        )
        .expect("graph");
        assert!(graph.outgoing(0).is_empty());
    }

    #[test]
    fn provenance_root_duplicates_keep_only_the_best_support_per_root() {
        let a = CognitiveRef::MemoryRevision(MemoryRevisionId::new());
        let b = CognitiveRef::MemoryRevision(MemoryRevisionId::new());
        let graph = WaveGraphGeneration::build(
            vec![node(a.clone(), 0), node(b.clone(), 1)],
            &[
                WaveEdgeEvidence {
                    from: a.clone(),
                    to: b.clone(),
                    basis_class: "host_explicit".into(),
                    association_kind: "assoc.related".into(),
                    polarity: "positive".into(),
                    support_mass: 0.6,
                    provenance_root: Some("artifact:one".into()),
                },
                WaveEdgeEvidence {
                    from: a.clone(),
                    to: b.clone(),
                    basis_class: "host_explicit".into(),
                    association_kind: "assoc.related".into(),
                    polarity: "positive".into(),
                    support_mass: 0.9,
                    provenance_root: Some("artifact:one".into()),
                },
                WaveEdgeEvidence {
                    from: a,
                    to: b,
                    basis_class: "host_explicit".into(),
                    association_kind: "assoc.related".into(),
                    polarity: "positive".into(),
                    support_mass: 0.4,
                    provenance_root: Some("artifact:two".into()),
                },
            ],
            WaveConfig::default(),
        )
        .expect("graph");
        assert_eq!(graph.edge_raw_support(0, 1), Some(1.3));
    }

    #[test]
    fn outbound_mass_is_bounded() {
        let a = CognitiveRef::MemoryRevision(MemoryRevisionId::new());
        let b = CognitiveRef::MemoryRevision(MemoryRevisionId::new());
        let c = CognitiveRef::MemoryRevision(MemoryRevisionId::new());
        let graph = WaveGraphGeneration::build(
            vec![node(a.clone(), 0), node(b.clone(), 1), node(c.clone(), 2)],
            &[
                WaveEdgeEvidence {
                    from: a.clone(),
                    to: b,
                    basis_class: "host_explicit".into(),
                    association_kind: "assoc.related".into(),
                    polarity: "positive".into(),
                    support_mass: 1.0,
                    provenance_root: None,
                },
                WaveEdgeEvidence {
                    from: a,
                    to: c,
                    basis_class: "host_explicit".into(),
                    association_kind: "assoc.related".into(),
                    polarity: "positive".into(),
                    support_mass: 1.0,
                    provenance_root: None,
                },
            ],
            WaveConfig::default(),
        )
        .expect("graph");
        assert!(graph.outbound_mass(0) <= 0.90 + f64::EPSILON);
    }
}
