// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

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
