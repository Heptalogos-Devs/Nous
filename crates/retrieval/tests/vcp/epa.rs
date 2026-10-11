// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;

#[test]
fn native_epa_and_dual_field_intermediate_values_match() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../docs/research/corpus/vcp/vcp-epa-dual-fields.json"
    ))
    .expect("numeric fixture");
    let input: FixtureInput =
        serde_json::from_value(fixture["input"].clone()).expect("neutral DTO");
    let expected = &fixture["expected"];
    let epa = reference_epa_analysis(&input.epa).expect("EPA");
    for (key, value) in [
        ("logic_depth", epa.logic_depth),
        ("entropy", epa.entropy),
        ("resonance", epa.resonance),
    ] {
        near(value, expected["epa"][key].as_f64().expect("scalar"));
    }
    for axis in expected["epa"]["axis_probabilities"]
        .as_array()
        .expect("axes")
    {
        near(
            epa.axis_probabilities[axis["index"].as_u64().expect("index") as usize],
            axis["energy"].as_f64().expect("energy"),
        );
    }
    let fields = reference_dual_fields(&input.transport, &input.source_field, &input.config)
        .expect("dual fields");
    field(&fields.local_field, &expected["local_field"]);
    field(&fields.transfer_field, &expected["transfer_field"]);
    assert_eq!(
        serde_json::to_value(fields.local_domain).unwrap(),
        expected["local_domain"]
    );
    assert_eq!(
        serde_json::to_value(fields.transfer_domain).unwrap(),
        expected["transfer_domain"]
    );
    let diagnostics = &expected["diagnostics"];
    assert_eq!(
        fields.iterations,
        diagnostics["iterations"].as_u64().unwrap() as usize
    );
    assert_eq!(
        fields.local_converged,
        diagnostics["localConverged"].as_bool().unwrap()
    );
    assert_eq!(
        fields.transfer_converged,
        diagnostics["transferConverged"].as_bool().unwrap()
    );
    near(
        fields.local_residual,
        diagnostics["localResidual"].as_f64().unwrap(),
    );
    near(
        fields.transfer_residual,
        diagnostics["transferResidual"].as_f64().unwrap(),
    );
    let empty = reference_dual_fields(&input.transport, &[], &input.config).expect("empty source");
    assert_eq!(empty.iterations, 0);
    assert!(empty.local_field.is_empty() && empty.transfer_field.is_empty());
}

#[test]
fn native_field_vector_projection_matches_available_index_vectors() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../docs/research/corpus/vcp/vcp-field-projection.json"
    ))
    .unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let input: ReferenceFieldProjectionInput =
            serde_json::from_value(case["input"].clone()).unwrap();
        let actual = reference_field_projection(&input);
        let expected: Vec<f32> = serde_json::from_value(case["expected"].clone()).unwrap();
        assert_eq!(actual.len(), expected.len());
        for (a, b) in actual.iter().zip(expected) {
            assert!(
                (a - b).abs() <= 1e-7 + 1e-7 * b.abs(),
                "{}: {a} != {b}",
                case["name"]
            );
        }
    }
}

#[test]
fn native_epa_density_sampling_weighted_basis_and_publication_values_match() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../docs/research/corpus/vcp/vcp-epa-training.json"
    ))
    .unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let input: ReferenceEpaTrainingInput =
            serde_json::from_value(case["input"].clone()).unwrap();
        let output = reference_train_epa(&input);
        let e = &case["expected"];
        assert_eq!(
            output.success,
            e["success"].as_bool().unwrap(),
            "{}",
            case["name"]
        );
        if !output.success {
            continue;
        }
        let s = &e["samples"];
        assert_eq!(
            output.bucket_keys,
            serde_json::from_value::<Vec<u16>>(s["bucket_keys"].clone()).unwrap()
        );
        assert_eq!(
            output.weights,
            serde_json::from_value::<Vec<usize>>(s["weights"].clone()).unwrap(),
            "{}",
            case["name"]
        );
        assert_eq!(
            output.labels,
            serde_json::from_value::<Vec<String>>(s["labels"].clone()).unwrap()
        );
        assert_eq!(
            output.representative_count,
            s["representative_count"].as_u64().unwrap() as usize
        );
        assert_eq!(
            output.bucket_count,
            s["bucket_count"].as_u64().unwrap() as usize
        );
        for (a, e) in [
            (&output.density_mean, &s["density_mean"]),
            (&output.mean, &e["cache"]["mean"]),
        ] {
            let b: Vec<f32> = serde_json::from_value(e.clone()).unwrap();
            for (a, b) in a.iter().zip(b) {
                assert!((a - b).abs() <= 1e-7, "{} mean {a}!={b}", case["name"]);
            }
        }
        let centroids: Vec<Vec<f32>> = serde_json::from_value(s["centroids"].clone()).unwrap();
        for (a, b) in output.centroids.iter().zip(centroids) {
            for (a, b) in a.iter().zip(b) {
                assert!((a - b).abs() <= 1e-7);
            }
        }
        let basis: Vec<Vec<f32>> = serde_json::from_value(e["cache"]["basis"].clone()).unwrap();
        assert_eq!(output.basis.len(), basis.len());
        for (a, b) in output.basis.iter().zip(basis) {
            let dot: f64 = a
                .iter()
                .zip(&b)
                .map(|(a, b)| f64::from(*a) * f64::from(*b))
                .sum();
            let sign = if dot >= 0.0 { 1.0 } else { -1.0 };
            for (a, b) in a.iter().zip(b) {
                assert!(
                    (f64::from(*a) - sign * f64::from(b)).abs() <= 1e-5,
                    "{} axis {a}!={b}",
                    case["name"]
                );
            }
        }
        let energies: Vec<f64> = serde_json::from_value(e["cache"]["energies"].clone()).unwrap();
        for (a, b) in output.energies.iter().zip(energies) {
            assert!((a - b).abs() <= 1e-5 + 1e-5 * b.abs());
        }
    }
}

#[test]
fn native_epa_and_dual_field_boundary_goldens_match() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../docs/research/corpus/vcp/vcp-epa-field-boundaries.json"
    ))
    .unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let input: FixtureInput = serde_json::from_value(case["input"].clone()).unwrap();
        let expected = &case["expected"];
        let epa = reference_epa_analysis(&input.epa).unwrap();
        for (key, value) in [
            ("logicDepth", epa.logic_depth),
            ("entropy", epa.entropy),
            ("resonance", epa.resonance),
        ] {
            near(value, expected["epa"][key].as_f64().unwrap());
        }
        assert_eq!(
            epa.cache_available,
            expected["epa"]["cacheAvailable"].as_bool().unwrap(),
            "{}",
            case["name"]
        );
        for axis in expected["epa"]["dominantAxes"].as_array().unwrap() {
            near(
                epa.axis_probabilities[axis["index"].as_u64().unwrap() as usize],
                axis["energy"].as_f64().unwrap(),
            );
        }
        let fields =
            reference_dual_fields(&input.transport, &input.source_field, &input.config).unwrap();
        let actual = serde_json::to_value(fields).unwrap();
        let mut field_expected = expected.clone();
        field_expected.as_object_mut().unwrap().remove("epa");
        compare_numeric_subset(&actual, &field_expected, case["name"].as_str().unwrap());
    }
}
