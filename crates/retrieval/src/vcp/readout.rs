// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use crate::{reference::*, *};
use std::collections::BTreeSet;

/// Offered candidate and its known self-evidence roots, supplied by the
/// Authority-scoped caller. The numerical adapter grants no visibility.
#[derive(Debug, Clone)]
pub struct VcpReadoutCandidate {
    pub reference: CognitiveRef,
    pub base_score: f64,
    pub bm25_score: f64,
    pub time_score: f64,
    pub anchor_score: f64,
    pub self_evidence_roots: BTreeSet<String>,
}
fn check_view(
    generation: &VcpServingGeneration,
    observation: &VcpQueryObservation,
    offered: &[VcpReadoutCandidate],
) -> Result<()> {
    if generation.generation_id != observation.generation_id() {
        return Err(Error::Unavailable(
            "VCP readout observation generation/profile mismatch".into(),
        ));
    }
    let mut seen = BTreeSet::new();
    for candidate in offered {
        let id = generation.identities.id(&candidate.reference)?;
        if !seen.insert(id)
            || !generation.curves.iter().any(|curve| curve.id == id)
            || [
                candidate.base_score,
                candidate.bm25_score,
                candidate.time_score,
                candidate.anchor_score,
            ]
            .iter()
            .any(|v| !v.is_finite())
        {
            return Err(Error::Invalid("invalid VCP offered candidate view".into()));
        }
        if candidate
            .self_evidence_roots
            .iter()
            .any(|root| !generation.graph.provenance_roots.contains(root))
        {
            return Err(Error::Invalid(
                "VCP self-evidence root is outside generation".into(),
            ));
        }
    }
    Ok(())
}
fn tag_vectors(generation: &VcpServingGeneration) -> Vec<(i64, Vec<f32>)> {
    generation
        .vectors
        .iter()
        .filter(|(id, _)| {
            generation
                .labels
                .binary_search_by_key(id, |(id, _)| *id)
                .is_ok()
        })
        .cloned()
        .collect()
}
fn transport_edges(generation: &VcpServingGeneration) -> Vec<(i64, i64, f64)> {
    let t = &generation.graph.graph.transport;
    t.node_ids
        .iter()
        .enumerate()
        .flat_map(|(row, source)| {
            (t.row_offsets[row]..t.row_offsets[row + 1])
                .map(move |offset| (*source, t.node_ids[t.targets[offset]], t.weights[offset]))
        })
        .collect()
}
fn source_type(observation: &VcpQueryObservation, id: i64, hop: usize) -> &'static str {
    if hop != 0 {
        "emergent"
    } else if observation
        .numerical()
        .gating
        .tags
        .iter()
        .any(|tag| tag.id == id && tag.is_core)
    {
        "core"
    } else {
        "seed"
    }
}

pub fn vcp_dtsc_readout(
    generation: &VcpServingGeneration,
    observation: &VcpQueryObservation,
    offered: &[VcpReadoutCandidate],
    config: &ReferenceDtscConfig,
    top_k: usize,
) -> Result<ReferenceDtscOutput> {
    check_view(generation, observation, offered)?;
    let n = observation.numerical();
    reference_dtsc(&ReferenceDtscInput {
        dimension: generation.space.dimension as usize,
        nodes: n
            .sense
            .nodes
            .iter()
            .map(|node| ReferenceDtscNode {
                id: node.id,
                energy: node.energy,
                normalized_energy: node.normalized_energy,
                source_type: source_type(observation, node.id, node.hop).into(),
            })
            .collect(),
        source_field: n.sense.source_field.clone(),
        tag_vectors: tag_vectors(generation),
        inbound: generation.graph.graph.inbound.clone(),
        anchor_gain: generation
            .intrinsic
            .iter()
            .filter_map(|r| r.anchor_gain.map(|gain| (r.id, gain)))
            .collect(),
        edges: transport_edges(generation),
        candidates: offered
            .iter()
            .map(|candidate| {
                let id = generation.identities.id(&candidate.reference)?;
                Ok(ReferenceDtscCandidate {
                    curve: generation
                        .curves
                        .iter()
                        .find(|c| c.id == id)
                        .unwrap()
                        .clone(),
                    score: candidate.base_score,
                })
            })
            .collect::<Result<Vec<_>>>()?,
        original_vector: observation.original_vector().to_vec(),
        enhanced_vector: n.fusion.vector.iter().map(|v| *v as f32).collect(),
        query_geometry_state: ReferenceDtscGeometryState {
            epa: ReferenceDtscEpaState {
                logic_depth: n.epa.logic_depth,
                entropy: n.epa.entropy,
                resonance: n.epa.resonance,
            },
            pyramid: ReferenceDtscPyramidState {
                coverage: n.pyramid.features.coverage,
                novelty: n.pyramid.features.novelty,
                depth: n.pyramid.features.depth as f64,
            },
        },
        config: config.clone(),
        top_k,
    })
}

/// Sum actual root mass into self/other buckets for this candidate only. The
/// tokens 1/2 represent mass classes, not fabricated provenance identities.
fn candidate_provenance(
    generation: &VcpServingGeneration,
    candidate: &VcpReadoutCandidate,
) -> Vec<ReferenceProvenanceEdge> {
    let id = generation
        .identities
        .id(&candidate.reference)
        .expect("validated candidate identity");
    let document_roots = generation
        .candidate_evidence_roots
        .iter()
        .map(|(id, roots)| {
            (
                generation
                    .identities
                    .reference(*id)
                    .expect("validated asset identity")
                    .to_string(),
                roots,
            )
        })
        .collect::<std::collections::BTreeMap<_, _>>();
    let mut self_roots = candidate.self_evidence_roots.clone();
    if let Some((_, roots)) = generation
        .candidate_evidence_roots
        .iter()
        .find(|(candidate_id, _)| *candidate_id == id)
    {
        self_roots.extend(roots.iter().cloned());
    }
    self_roots.insert(candidate.reference.to_string());
    generation
        .graph
        .graph
        .provenance
        .iter()
        .map(|edge| {
            let (mut own, mut other) = (0.0, 0.0);
            for (id, mass) in &edge.file_contributions {
                let root = &generation.graph.provenance_roots[*id as usize - 1];
                let (self_mass, independent_mass) =
                    root_mass_ownership(root, *mass, &self_roots, &document_roots);
                own += self_mass;
                other += independent_mass;
            }
            ReferenceProvenanceEdge {
                source_id: edge.source_id,
                target_id: edge.target_id,
                file_contributions: [(1, own), (2, other)]
                    .into_iter()
                    .filter(|(_, mass)| *mass > 0.0)
                    .collect(),
            }
        })
        .collect()
}

/// A cooccurrence contribution is one mass even when its document depends on
/// several roots. Known roots divide that mass evenly; unknown lineage supplies
/// no independent credit. This is explicit Nous ontology adaptation.
fn root_mass_ownership(
    root: &str,
    mass: f64,
    self_roots: &BTreeSet<String>,
    document_roots: &std::collections::BTreeMap<String, &BTreeSet<String>>,
) -> (f64, f64) {
    if self_roots.contains(root) || root.starts_with("unknown-dependency:") {
        return (mass, 0.0);
    }
    let Some(roots) = document_roots.get(root) else {
        return (0.0, mass);
    };
    if roots.is_empty() {
        return (mass, 0.0);
    }
    let own_count = roots
        .iter()
        .filter(|root| self_roots.contains(*root) || root.starts_with("unknown-dependency:"))
        .count();
    let own = mass * own_count as f64 / roots.len() as f64;
    (own, mass - own)
}

pub fn vcp_v3_readout(
    generation: &VcpServingGeneration,
    observation: &VcpQueryObservation,
    offered: &[VcpReadoutCandidate],
    config: &ReferenceReadoutConfig,
    top_k: usize,
) -> Result<ReferenceReadoutOutput> {
    check_view(generation, observation, offered)?;
    let n = observation.numerical();
    let candidates = offered
        .iter()
        .map(|candidate| {
            let id = generation.identities.id(&candidate.reference)?;
            Ok(ReferenceCandidateCurve {
                curve: generation
                    .curves
                    .iter()
                    .find(|c| c.id == id)
                    .unwrap()
                    .clone(),
                file_id: 1,
                bm25_score: candidate.bm25_score,
                time_score: candidate.time_score,
                anchor_score: candidate.anchor_score,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let provenance = n
        .sense
        .nodes
        .iter()
        .filter(|node| node.hop == 0)
        .map(|node| ReferenceSourceProvenance {
            id: node.id,
            hop: 0,
            source_type: source_type(observation, node.id, node.hop).into(),
        })
        .collect();
    reference_v3_readout(&ReferenceReadoutInput {
        observation: ReferenceReadoutObservation {
            original_vector: observation.original_vector().to_vec(),
            enhanced_vector: n.fusion.vector.iter().map(|v| *v as f32).collect(),
            local_vector: n.local_vector.clone(),
            transfer_vector: n.transfer_vector.clone(),
            source_field: n.sense.source_field.clone(),
            local_field: n.fields.local_field.clone(),
            transfer_field: n.fields.transfer_field.clone(),
            local_domain: n.fields.local_domain.clone(),
            transfer_domain: n.fields.transfer_domain.clone(),
            river: ReferenceRiverShapeInput {
                river_nodes: n
                    .sense
                    .nodes
                    .iter()
                    .map(|node| ReferenceRiverNode {
                        id: node.id,
                        energy: node.energy,
                        normalized_energy: node.normalized_energy,
                        hop: node.hop as i64,
                    })
                    .collect(),
                river_edges: n
                    .sense
                    .edges
                    .iter()
                    .map(|edge| ReferenceRiverEdge {
                        source_id: edge.source_id,
                        target_id: edge.target_id,
                        flow: edge.flow,
                        normalized_flow: edge.normalized_flow,
                    })
                    .collect(),
                complete_observation: !n.sense.source_field.is_empty(),
            },
            provenance,
        },
        candidates,
        transport: generation.graph.graph.transport.clone(),
        edge_provenance: Vec::new(),
        candidate_provenance: offered
            .iter()
            .map(|candidate| {
                Ok((
                    generation.identities.id(&candidate.reference)?,
                    candidate_provenance(generation, candidate),
                ))
            })
            .collect::<Result<Vec<_>>>()?,
        tag_vectors: tag_vectors(generation),
        inbound: generation.graph.graph.inbound.clone(),
        allowed_file_ids: vec![1],
        config: config.clone(),
        top_k,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn document_source_overlap_conserves_mass_and_unknown_lineage_is_not_independent() {
        let own = BTreeSet::from(["external:shared".into()]);
        let same = BTreeSet::from(["external:shared".into()]);
        let partial = BTreeSet::from(["external:shared".into(), "external:independent".into()]);
        let unknown = BTreeSet::from(["unknown-dependency:memory_revision:x".into()]);
        let empty = BTreeSet::new();
        let docs = std::collections::BTreeMap::from([
            ("memory_revision:same".into(), &same),
            ("memory_revision:partial".into(), &partial),
            ("memory_revision:unknown".into(), &unknown),
            ("memory_revision:empty".into(), &empty),
        ]);
        for (root, expected) in [
            ("memory_revision:same", (0.8, 0.0)),
            ("memory_revision:partial", (0.4, 0.4)),
            ("memory_revision:unknown", (0.8, 0.0)),
            ("memory_revision:empty", (0.8, 0.0)),
            ("external:independent", (0.0, 0.8)),
            ("external:shared", (0.8, 0.0)),
        ] {
            let actual = root_mass_ownership(root, 0.8, &own, &docs);
            assert_eq!(actual, expected);
            assert!((actual.0 + actual.1 - 0.8).abs() < 1e-12);
        }
    }
}
