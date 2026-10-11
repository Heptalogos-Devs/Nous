// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;

#[test]
fn native_candidate_observables_and_pure_score_match_persisted_vectors() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../docs/research/corpus/vcp/vcp-observables-pure.json"
    ))
    .expect("native persisted curve matrix");
    for case in fixture["cases"].as_array().unwrap() {
        let input: ReferenceObservableInput =
            serde_json::from_value(case["input"].clone()).unwrap();
        let local: Vec<f32> = serde_json::from_value(case["local_vector"].clone()).unwrap();
        let transfer: Vec<f32> = serde_json::from_value(case["transfer_vector"].clone()).unwrap();
        let topology = serde_json::from_value(case["topology"].clone()).unwrap();
        let morphology = serde_json::from_value(case["morphology"].clone()).unwrap();
        let config = serde_json::from_value(case["config"].clone()).unwrap();
        let observables = reference_observables(&input);
        compare_numeric_subset(
            &serde_json::to_value(&observables).unwrap(),
            &case["expected"]["observables"],
            case["name"].as_str().unwrap(),
        );
        if case["expected"].get("pureScore").is_none() {
            continue;
        }
        let cosine = |a: &[f32], b: &[f32]| {
            let dot: f64 = a
                .iter()
                .zip(b)
                .map(|(a, b)| f64::from(*a) * f64::from(*b))
                .sum();
            let magnitude = |v: &[f32]| v.iter().map(|x| f64::from(*x).powi(2)).sum::<f64>().sqrt();
            dot / (magnitude(a) * magnitude(b))
        };
        let scores = reference_pure_scores(&ReferencePureInput {
            query_score: cosine(&input.query_vector, &input.curve.chunk_vector),
            local_score: cosine(&local, &input.curve.chunk_vector),
            transfer_score: cosine(&transfer, &input.curve.chunk_vector),
            geometry: &input.geometry,
            observables: &observables,
            topology: &topology,
            morphology: &morphology,
            config: &config,
        });
        compare_numeric_subset(
            &serde_json::json!(scores.pure_score),
            &case["expected"]["pureScore"],
            case["name"].as_str().unwrap(),
        );
    }
}

#[test]
fn native_candidate_superset_sources_scores_quotas_and_order_match() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../docs/research/corpus/vcp/vcp-candidate-pool.json"
    ))
    .unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let signals: Vec<ReferencePoolSignals> =
            serde_json::from_value(case["input"]["signals"].clone()).unwrap();
        let config: ReferencePoolConfig =
            serde_json::from_value(case["input"]["config"].clone()).unwrap();
        compare_numeric_subset(
            &serde_json::to_value(reference_candidate_pool(&signals, &config)).unwrap(),
            &case["expected"],
            case["name"].as_str().unwrap(),
        );
    }
}

#[test]
fn native_v3_readout_composes_one_observation_through_candidate_pool_and_final_ranking() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../docs/research/corpus/vcp/vcp-v3-readout.json"
    ))
    .unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let input: ReferenceReadoutInput = serde_json::from_value(case["input"].clone()).unwrap();
        let output = reference_v3_readout(&input).unwrap();
        let actual = serde_json::to_value(output).unwrap();
        for field in ["morphology", "omega", "selected"] {
            compare_numeric_subset(&actual[field], &case["expected"][field], field);
        }
        let expected = case["expected"]["results"].as_array().unwrap();
        let results = actual["results"].as_array().unwrap();
        assert_eq!(results.len(), expected.len());
        for (actual, expected) in results.iter().zip(expected) {
            for field in [
                "id",
                "rank",
                "score",
                "baseScore",
                "geometry",
                "relativeTopology",
                "observables",
                "anchor",
            ] {
                compare_numeric_subset(&actual[field], &expected[field], field);
            }
            for field in [
                "id",
                "role",
                "v2Bonus",
                "gatedBonus",
                "anchorBonus",
                "finalScore",
            ] {
                compare_numeric_subset(
                    &actual["scoring"][field],
                    &expected["scoring"][field],
                    field,
                );
            }
        }
    }
}

#[test]
fn native_dtsc_field_trust_retention_exact_contacts_and_sampling_match() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../docs/research/corpus/vcp/vcp-dtsc-field.json"
    ))
    .unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let input: ReferenceDtscFieldInput = serde_json::from_value(case["input"].clone()).unwrap();
        compare_numeric_subset(
            &serde_json::to_value(reference_dtsc_field(&input)).unwrap(),
            &case["expected"],
            case["name"].as_str().unwrap(),
        );
    }
}

#[test]
fn native_dtsc_curve_metrics_rewards_guards_and_full_order_match() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../docs/research/corpus/vcp/vcp-dtsc.json"
    ))
    .unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let input: ReferenceDtscInput = serde_json::from_value(case["input"].clone()).unwrap();
        if case.get("error").is_some() {
            assert!(matches!(
                reference_dtsc(&input),
                Err(nous_core::Error::Invalid(_))
            ));
            continue;
        }
        compare_numeric_subset(
            &serde_json::to_value(reference_dtsc(&input).unwrap()).unwrap(),
            &case["expected"],
            case["name"].as_str().unwrap(),
        );
    }
}
