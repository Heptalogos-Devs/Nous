// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;
use nous_core::MemoryRevisionId;

fn graph() -> WaveGraphGeneration {
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
        build_policy: WaveBuildPolicy::default(),
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
    let graph = graph();
    let readout = WaveReadoutPolicy {
        initial_budget_steps: 1,
        ..Default::default()
    };
    let river = propagate(&graph, &[seed(0, "a")], &readout);
    assert!(river.node_potential.contains_key(&2));
    assert!(!river.node_potential.contains_key(&3));
    assert!(river.complete);
    let readout = WaveReadoutPolicy {
        initial_budget_steps: 1,
        normal_edge_cost: 2,
        ..Default::default()
    };
    let river = propagate(&graph, &[seed(0, "a")], &readout);
    assert_eq!(river.node_potential.len(), 1);
    assert!(river.edges.is_empty());
}

#[test]
fn merged_paths_keep_all_source_origins_downstream() {
    let graph = graph();
    for seeds in [
        vec![seed(0, "a"), seed(1, "b")],
        vec![seed(1, "b"), seed(0, "a")],
    ] {
        let river = propagate(&graph, &seeds, &WaveReadoutPolicy::default());
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
fn state_budget_truncation_is_reported() {
    let graph = graph();
    let river = propagate_with_budget(
        &graph,
        &[seed(0, "test-a"), seed(1, "test-b")],
        &WaveReadoutPolicy::default(),
        4,
        1,
    );
    assert!(!river.complete);
    assert!(river.discarded_state_mass > 0.0);
}
