// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use nous_retrieval::reference::*;
use serde::Deserialize;
use serde_json::Value;

#[derive(Deserialize)]
struct FixtureInput {
    epa: ReferenceEpaInput,
    transport: ReferenceTransport,
    source_field: Vec<(i64, f64)>,
    config: ReferenceFieldConfig,
}
fn near(a: f64, b: f64) {
    assert!((a - b).abs() <= 1e-10 + 1e-10 * b.abs(), "{a} != {b}");
}
fn field(actual: &[(i64, f64)], expected: &Value) {
    let expected: Vec<(i64, f64)> = serde_json::from_value(expected.clone()).expect("native field");
    assert_eq!(actual.len(), expected.len());
    for ((a, av), (b, bv)) in actual.iter().zip(expected) {
        assert_eq!(*a, b);
        near(*av, bv);
    }
}
fn compare_numeric_subset(actual: &Value, expected: &Value, path: &str) {
    match (actual, expected) {
        (Value::Number(a), Value::Number(b)) => {
            let a = a.as_f64().unwrap();
            let b = b.as_f64().unwrap();
            assert!(
                (a - b).abs() <= 1e-12 + 1e-12 * b.abs(),
                "{path}: {a} != {b}"
            );
        }
        (Value::Array(a), Value::Array(b)) => {
            assert_eq!(a.len(), b.len(), "{path}");
            for (index, (a, b)) in a.iter().zip(b).enumerate() {
                compare_numeric_subset(a, b, &format!("{path}/{index}"));
            }
        }
        (Value::Object(a), Value::Object(b)) => {
            for (key, value) in a {
                compare_numeric_subset(
                    value,
                    b.get(key).unwrap_or_else(|| panic!("{path}/{key}")),
                    &format!("{path}/{key}"),
                );
            }
        }
        _ => assert_eq!(actual, expected, "{path}"),
    }
}

fn verify_pipeline_fixture(fixture: &Value) {
    let input: ReferencePipelineInput = serde_json::from_value(fixture["input"].clone()).unwrap();
    let layers: Vec<Vec<ReferenceResidualCandidate>> =
        serde_json::from_value(fixture["search_results"].clone()).unwrap();
    let mut level = 0;
    let output = reference_query_pipeline(&input, |_, limit| {
        let hits = layers.get(level).cloned().unwrap_or_default();
        level += 1;
        assert!(hits.len() <= limit);
        Ok(hits)
    })
    .unwrap();
    let expected = &fixture["expected"];
    for (key, value) in [
        ("logicDepth", output.epa.logic_depth),
        ("entropy", output.epa.entropy),
        ("resonance", output.epa.resonance),
    ] {
        near(value, expected["epa"][key].as_f64().unwrap());
    }
    compare_numeric_subset(
        &serde_json::to_value(&output.pyramid).unwrap(),
        &expected["pyramid"],
        "pyramid",
    );
    compare_numeric_subset(
        &serde_json::to_value(&output.gating).unwrap(),
        &expected["gating"],
        "gating",
    );
    let mut sense = serde_json::to_value(&output.sense).unwrap();
    let mut native = expected["sense"].clone();
    for field in ["nodes", "edges"] {
        for data in [&mut sense, &mut native] {
            data[field].as_array_mut().unwrap().sort_by_key(|n| {
                if field == "nodes" {
                    (n["id"].as_i64().unwrap(), 0)
                } else {
                    (
                        n["sourceId"].as_i64().unwrap(),
                        n["targetId"].as_i64().unwrap(),
                    )
                }
            });
        }
    }
    for field in ["sourceField", "nodes", "edges", "diagnostics"] {
        compare_numeric_subset(&sense[field], &native[field], field);
    }
    compare_numeric_subset(
        &serde_json::to_value(&output.fusion.diagnostics).unwrap(),
        &expected["fusion_diagnostics"],
        "fusion",
    );
    assert_eq!(
        output.fusion.vector.len(),
        expected["enhanced_vector"].as_array().unwrap().len()
    );
    for (a, b) in output
        .fusion
        .vector
        .iter()
        .zip(expected["enhanced_vector"].as_array().unwrap())
    {
        assert!((a - b.as_f64().unwrap()).abs() <= 1e-7);
    }
    compare_numeric_subset(
        &serde_json::to_value(output.fields).unwrap(),
        &expected["fields"],
        "fields",
    );
    for (a, e) in [
        (&output.local_vector, &expected["local_vector"]),
        (&output.transfer_vector, &expected["transfer_vector"]),
    ] {
        let b: Vec<f32> = serde_json::from_value(e.clone()).unwrap();
        assert_eq!(a.len(), b.len());
        for (a, b) in a.iter().zip(b) {
            assert!((a - b).abs() <= 1e-7);
        }
    }
}

#[path = "vcp/epa.rs"]
mod epa;
#[path = "vcp/geometry.rs"]
mod geometry;
#[path = "vcp/query.rs"]
mod query;
#[path = "vcp/readout.rs"]
mod readout;
