// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use nous_core::{CognitiveRef, MemoryRevisionId};
use nous_retrieval::{
    SourceSeed, WaveConfig, WaveEdgeEvidence, WaveGraphGeneration, WaveNode, WaveNodeKind,
    propagate_with_budget,
};
use serde_json::{Value, json};

fn reference(index: usize) -> CognitiveRef {
    let id: MemoryRevisionId =
        serde_json::from_value(json!(format!("00000000-0000-0000-0000-{:012}", index + 1)))
            .expect("fixed UUID");
    CognitiveRef::MemoryRevision(id)
}

fn graph() -> WaveGraphGeneration {
    let nodes = (0..4)
        .map(|index| WaveNode {
            serving_id: index as u32,
            reference: reference(index),
            node_kind: WaveNodeKind::Memory,
            embedding_key: None,
            posting_key: None,
            intrinsic_residual_gain: None,
        })
        .collect();
    let edge = |from, to, mass, root: &str| WaveEdgeEvidence {
        from: reference(from),
        to: reference(to),
        basis_class: "host_explicit".into(),
        association_kind: "assoc.related".into(),
        polarity: "positive".into(),
        support_mass: mass,
        provenance_root: Some(root.into()),
    };
    WaveGraphGeneration::build(
        nodes,
        &[
            edge(0, 1, 0.6, "shared"),
            edge(0, 1, 0.9, "shared"),
            edge(0, 1, 0.4, "independent"),
            edge(0, 2, 1.0, "other"),
            edge(1, 0, 1.0, "return"),
            edge(1, 3, 1.0, "chain"),
            edge(2, 3, 1.0, "merge"),
            edge(3, 1, 0.5, "cycle"),
        ],
        WaveConfig::default(),
    )
    .expect("baseline graph")
}

fn observation(graph: &WaveGraphGeneration, max_states: usize, seeded: bool) -> Value {
    let seeds = if seeded {
        vec![SourceSeed {
            node: 0,
            weight: 1.0,
            seed_family: "exact_target".into(),
            origin_cue: "fixed-root".into(),
            hop_zero: true,
        }]
    } else {
        Vec::new()
    };
    let river = propagate_with_budget(graph, &seeds, 4, max_states);
    let mut provenance = river.provenance.clone();
    for node in &mut provenance {
        node.origin_seeds.sort();
    }
    let mut ordering = river
        .node_potential
        .iter()
        .map(|(node, potential)| (*node, *potential))
        .collect::<Vec<_>>();
    ordering.sort_by(|left, right| {
        right.1.total_cmp(&left.1).then_with(|| {
            reference(left.0 as usize)
                .to_string()
                .cmp(&reference(right.0 as usize).to_string())
        })
    });
    json!({"source_field":river.source_field,"node_potential":river.node_potential,"edges":river.edges,
        "provenance":provenance,"total_edge_flow":river.total_edge_flow,"discarded_state_mass":river.discarded_state_mass,
        "generated_state_mass":river.generated_state_mass,"complete":river.complete,"max_hops":river.max_hops,
        "candidate_order":ordering.iter().map(|(node,_)|reference(*node as usize).to_string()).collect::<Vec<_>>()})
}

fn compare(actual: &Value, expected: &Value, path: &str) {
    match (actual, expected) {
        (Value::Number(left), Value::Number(right)) if left.is_f64() || right.is_f64() => {
            let left = left.as_f64().expect("number");
            let right = right.as_f64().expect("number");
            assert!(
                (left - right).abs() <= 1e-12 + 1e-12 * right.abs(),
                "{path}: {left} != {right}"
            );
        }
        (Value::Array(left), Value::Array(right)) => {
            assert_eq!(left.len(), right.len(), "{path}");
            for (index, (left, right)) in left.iter().zip(right).enumerate() {
                compare(left, right, &format!("{path}/{index}"));
            }
        }
        (Value::Object(left), Value::Object(right)) => {
            assert_eq!(left.len(), right.len(), "{path}");
            for (key, left) in left {
                compare(
                    left,
                    right.get(key).expect("golden key"),
                    &format!("{path}/{key}"),
                );
            }
        }
        _ => assert_eq!(actual, expected, "{path}"),
    }
}

#[test]
fn frozen_experimental_node_potential_contract() {
    let graph = graph();
    let actual = json!({
        "conductance":(0..4).map(|node|graph.outgoing(node)).collect::<Vec<_>>(),
        "deduplicated_root_support":graph.edge_raw_support(0,1),
        "full":observation(&graph,4096,true),"state_truncated":observation(&graph,1,true),
        "empty_seed":observation(&graph,4096,false),
    });
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/research/corpus/vcp/nous-wave-v1.json");
    let expected: Value =
        serde_json::from_slice(&std::fs::read(path).expect("frozen baseline fixture"))
            .expect("fixture JSON");
    compare(&actual, &expected["expected"], "wave");
}
