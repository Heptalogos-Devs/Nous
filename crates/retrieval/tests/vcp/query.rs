// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;

#[test]
fn frozen_sense_merge_and_empty_goldens_match_intermediate_contracts() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../docs/research/corpus/vcp/vcp-sense.json"
    ))
    .expect("frozen Sense matrix");
    for case in fixture["cases"].as_array().expect("cases") {
        let graph: ReferenceSenseGraph =
            serde_json::from_value(case["graph"].clone()).expect("neutral graph");
        let input: ReferenceSenseInput =
            serde_json::from_value(case["input"].clone()).expect("neutral query");
        let output = reference_sense(&graph, &input).expect("independent Sense");
        let mut actual = serde_json::to_value(output).expect("numeric output");
        let mut expected = case["expected"].clone();
        // Flow/node array ties in frozen VCP lack a total comparator. Canonical
        // key order compares every value and the exact discrete membership.
        for value in [&mut actual, &mut expected] {
            value["nodes"]
                .as_array_mut()
                .unwrap()
                .sort_by_key(|n| n["id"].as_i64().unwrap());
            value["edges"].as_array_mut().unwrap().sort_by_key(|e| {
                (
                    e["sourceId"].as_i64().unwrap(),
                    e["targetId"].as_i64().unwrap(),
                )
            });
            value["sourceField"]
                .as_array_mut()
                .unwrap()
                .sort_by_key(|entry| entry[0].as_i64().unwrap());
        }
        compare_numeric_subset(&actual, &expected, case["name"].as_str().unwrap());
    }
}

#[test]
fn native_query_tag_gating_language_core_and_layer_weights_match() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../docs/research/corpus/vcp/vcp-gating.json"
    ))
    .unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let input: ReferenceGateInput = serde_json::from_value(case["input"].clone()).unwrap();
        compare_numeric_subset(
            &serde_json::to_value(reference_gate_tags(&input)).unwrap(),
            &case["expected"],
            case["name"].as_str().unwrap(),
        );
    }
}

#[test]
fn native_query_fusion_seed_max_supplements_ghosts_and_dedup_match() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../docs/research/corpus/vcp/vcp-fusion.json"
    ))
    .unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let input: ReferenceFusionInput = serde_json::from_value(case["input"].clone()).unwrap();
        compare_numeric_subset(
            &serde_json::to_value(reference_fuse_observation(&input)).unwrap(),
            &case["expected"],
            case["name"].as_str().unwrap(),
        );
    }
}

#[test]
fn native_query_pipeline_composes_one_query_through_sense_fusion_and_dual_fields() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../docs/research/corpus/vcp/vcp-query-pipeline.json"
    ))
    .unwrap();
    verify_pipeline_fixture(&fixture);
}

#[test]
fn native_query_pipeline_state_budget_golden_matches() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../docs/research/corpus/vcp/vcp-pipeline-state-budget.json"
    ))
    .unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        verify_pipeline_fixture(case);
    }
}
