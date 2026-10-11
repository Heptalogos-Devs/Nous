// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;

#[test]
fn native_ordered_graph_transport_and_provenance_match() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../docs/research/corpus/vcp/vcp-graph.json"
    ))
    .expect("native graph");
    let input: ReferenceGraphInput =
        serde_json::from_value(fixture["input"].clone()).expect("neutral graph DTO");
    let graph = reference_graph(&input).expect("reference graph build");
    let value = serde_json::to_value(&graph).unwrap();
    for field in [
        "fact_matrix",
        "transport",
        "wormholes",
        "inbound",
        "provenance",
    ] {
        compare_numeric_subset(&value[field], &fixture["expected"][field], field);
    }
    for range in graph.transport.row_offsets.windows(2) {
        let mass: f64 = graph.transport.weights[range[0]..range[1]].iter().sum();
        assert!(mass <= input.config.outbound_mass + 1e-12);
    }
}

#[test]
fn native_residual_pyramid_projection_handshake_and_features_match() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../docs/research/corpus/vcp/vcp-pyramid.json"
    ))
    .expect("native pyramid matrix");
    for case in fixture["cases"].as_array().unwrap() {
        let query: Vec<f32> = serde_json::from_value(case["query"].clone()).unwrap();
        let config: ReferencePyramidConfig =
            serde_json::from_value(case["config"].clone()).unwrap();
        let layers: Vec<Vec<ReferenceResidualCandidate>> =
            serde_json::from_value(case["search_results"].clone()).unwrap();
        let mut level = 0;
        let output = reference_pyramid(&query, &config, |_, limit| {
            let hits = layers.get(level).cloned().unwrap_or_default();
            assert!(hits.len() <= limit);
            level += 1;
            Ok(hits)
        })
        .expect("independent projection");
        compare_numeric_subset(
            &serde_json::to_value(output).unwrap(),
            &case["expected"],
            case["name"].as_str().unwrap(),
        );
    }
}

#[test]
fn native_query_morphology_and_all_omega_components_match() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../docs/research/corpus/vcp/vcp-query-shape.json"
    ))
    .expect("query shape matrix");
    for case in fixture["cases"].as_array().unwrap() {
        let input: ReferenceRiverShapeInput =
            serde_json::from_value(case["input"].clone()).unwrap();
        let config: ReferenceOmegaConfig = serde_json::from_value(case["config"].clone()).unwrap();
        let morphology = reference_morphology(&input);
        let omega = reference_omega(&input, &config);
        let total = morphology.atomic_weight
            + morphology.propositional_weight
            + morphology.narrative_weight;
        assert!((total - 1.0).abs() <= 1e-12);
        let actual = serde_json::json!({"morphology":morphology,"omega":omega});
        compare_numeric_subset(&actual, &case["expected"], case["name"].as_str().unwrap());
    }
}

#[test]
fn native_direct_anchor_pool_contacts_and_fallback_match() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../docs/research/corpus/vcp/vcp-anchors.json"
    ))
    .expect("native anchor matrix");
    for case in fixture["cases"].as_array().unwrap() {
        let input: ReferenceAnchorInput = serde_json::from_value(case["input"].clone()).unwrap();
        let anchors = reference_anchors(&input);
        compare_numeric_subset(
            &serde_json::to_value(anchors).unwrap(),
            &case["expected"],
            case["name"].as_str().unwrap(),
        );
    }
}

#[test]
fn native_dual_field_path_geometry_components_match() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../docs/research/corpus/vcp/vcp-path.json"
    ))
    .expect("native path matrix");
    for case in fixture["cases"].as_array().unwrap() {
        let input: ReferencePathInput = serde_json::from_value(case["input"].clone()).unwrap();
        compare_numeric_subset(
            &serde_json::to_value(reference_path_geometry(&input)).unwrap(),
            &case["expected"],
            case["name"].as_str().unwrap(),
        );
    }
}

#[test]
fn native_relative_topology_alignment_distance_and_source_independence_match() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../docs/research/corpus/vcp/vcp-relative-topology.json"
    ))
    .expect("native relative topology");
    for case in fixture["cases"].as_array().unwrap() {
        let input: ReferenceTopologyInput = serde_json::from_value(case["input"].clone()).unwrap();
        compare_numeric_subset(
            &serde_json::to_value(reference_relative_topology(&input)).unwrap(),
            &case["expected"],
            case["name"].as_str().unwrap(),
        );
    }
}

#[test]
fn native_conditional_peer_statistics_role_caps_and_anchor_activation_match() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../docs/research/corpus/vcp/vcp-scoring.json"
    ))
    .expect("native scoring matrix");
    for case in fixture["cases"].as_array().unwrap() {
        let input: ReferenceScoreInput = serde_json::from_value(case["input"].clone()).unwrap();
        compare_numeric_subset(
            &serde_json::to_value(reference_v3_scores(&input)).unwrap(),
            &case["expected"],
            case["name"].as_str().unwrap(),
        );
    }
}

#[test]
fn native_intrinsic_residual_ratios_fixed_anchor_gains_and_statuses_match() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../docs/research/corpus/vcp/vcp-intrinsic.json"
    ))
    .unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let input: ReferenceIntrinsicInput = serde_json::from_value(case["input"].clone()).unwrap();
        let tolerance = if input.config.method.trim() == "svd" {
            1e-6
        } else {
            1e-10
        };
        let actual = reference_intrinsic_residual(&input);
        let expected = case["expected"].as_array().unwrap();
        assert_eq!(actual.len(), expected.len());
        for (a, b) in actual.iter().zip(expected) {
            assert_eq!(a.id, b["id"].as_i64().unwrap(), "{}", case["name"]);
            assert_eq!(
                a.status,
                b["status"].as_str().unwrap(),
                "{} id{}",
                case["name"],
                a.id
            );
            assert_eq!(
                a.neighbor_count,
                b["neighbor_count"].as_u64().unwrap() as usize
            );
            for (value, key) in [
                (a.raw_residual_ratio, "raw_residual_ratio"),
                (a.anchor_gain, "anchor_gain"),
            ] {
                match (value, b[key].as_f64()) {
                    (Some(value), Some(expected)) => assert!(
                        (value - expected).abs() <= tolerance + tolerance * expected.abs(),
                        "{} id{} {key}: {value} != {expected}",
                        case["name"],
                        a.id
                    ),
                    (None, None) => {}
                    _ => panic!("{} id{} {key}", case["name"], a.id),
                }
            }
        }
    }
}

#[test]
fn native_graph_builder_boundary_and_merge_goldens_match() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../docs/research/corpus/vcp/vcp-graph-boundaries.json"
    ))
    .unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let input: ReferenceGraphInput = serde_json::from_value(case["input"].clone()).unwrap();
        let actual = serde_json::to_value(reference_graph(&input).unwrap()).unwrap();
        for key in ["fact_matrix", "transport", "wormholes", "provenance"] {
            compare_numeric_subset(
                &actual[key],
                &case["expected"][key],
                &format!("{}/{key}", case["name"].as_str().unwrap()),
            );
        }
    }
}
