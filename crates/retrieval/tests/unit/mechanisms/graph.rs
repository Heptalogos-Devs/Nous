// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

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
        WaveBuildPolicy::default(),
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
        WaveBuildPolicy::default(),
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
        WaveBuildPolicy::default(),
    )
    .expect("graph");
    assert!(graph.outbound_mass(0) <= 0.90 + f64::EPSILON);
}
