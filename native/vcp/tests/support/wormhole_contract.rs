// Adapted from VCP d5c019e rivermemo_wormhole.rs score_contract_fixture_tests, CC BY-NC-SA 4.0.

use super::*;
use serde_json::json;

fn candidate(id: i64, baseline: f64, eligible: bool) -> ScoredWork {
    ScoredWork {
        curve: Curve {
            id,
            file_id: id,
            source_ids: vec![id],
            tags: if eligible {
                vec![TagData {
                    id: 2,
                    name: "synthetic-target".into(),
                    position: 0,
                    vector: Arc::from(vec![1.0_f32, 0.0]),
                    chunk_cosine: 1.0,
                }]
            } else {
                vec![]
            },
            chunk_vector: Arc::from(if eligible {
                vec![1.0_f32, 0.0]
            } else {
                vec![0.0_f32, 1.0]
            }),
            query_score: baseline,
            denoised_score: baseline,
            local_score: baseline,
            transfer_score: baseline,
            bm25_score: 0.0,
            lexical_score: 0.0,
            time_score: 0.0,
            anchor_score: 0.0,
            direct_anchor_score: 0.0,
            union_score: baseline,
            union_rank: id as usize,
            sources: vec![],
        },
        geometry: GeometryOutput::default(),
        topology: TopologyOutput::default(),
        observables: ObservableOutput::default(),
        pure_score: baseline,
        field_score: baseline,
        field_resonance_bonus: 0.0,
        semantic_score: baseline,
        lexical_base: baseline,
        graph_score: 0.0,
        direct_evidence: 0.0,
        role: String::new(),
        anchor: AnchorOutput::default(),
        v2_bonus: 0.0,
        gated_bonus: 0.0,
        path_bonus: 0.0,
        anchor_bonus: 0.0,
        final_score: baseline,
        ranking_score: -1.0,
    }
}

#[test]
fn public_dataset_preserves_rrf_order_and_candidate_quality_scores() {
    let fixture: Value =
        serde_json::from_str(include_str!("../fixtures/rrf_score_contract.json")).unwrap();
    let omega_value = fixture["omega"].as_f64().unwrap();
    let omega = OmegaOutput {
        omega: omega_value,
        omega_edge: 1.0,
        omega_emerge: 1.0,
        omega_flow: 1.0,
        regime: "dense".into(),
        active_edges: 3,
        seed_nodes: 1,
        reached_nodes: 3,
        emergent_nodes: 2,
        complete_observation: true,
    };
    for scenario in fixture["scenarios"].as_array().unwrap() {
        for trace in [true, false] {
            let ordinary = scenario["ordinaryFlow"].as_f64().unwrap();
            let flows: Vec<f64> = scenario["flows"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_f64().unwrap())
                .collect();
            let mut edges = vec![];
            if ordinary > 0.0 {
                edges.push(json!({"sourceId":1,"targetId":4,"flow":ordinary,"wormhole":false}));
                edges.push(json!({"sourceId":1,"targetId":1,"flow":99,"wormhole":false}));
                edges.push(json!({"sourceId":4,"targetId":1,"flow":88,"wormhole":false,"immediateReturn":true}));
            }
            for flow in &flows {
                edges.push(json!({"sourceId":1,"targetId":2,"flow":flow,"wormhole":true}));
            }
            let mut input: NativeInput = serde_json::from_value(json!({
                "query":{"vector":[1,0]}, "queryState":{"completeObservation":true,
                "fieldProvenance":[{"id":1,"hop":0,"sourceType":"core"}],"riverEdges":edges}
            }))
            .unwrap();
            input.include_trace = trace;
            let model = Model {
                eligible_routes: flows.len(),
                routes: flows
                    .iter()
                    .map(|flow| Route {
                        source_id: 1,
                        target_id: 2,
                        flow: *flow,
                        relative_flow: 1.0,
                        vector: Arc::from(vec![1.0_f32, 0.0]),
                    })
                    .collect(),
            };
            let mut items: Vec<_> = fixture["candidates"]
                .as_array()
                .unwrap()
                .iter()
                .map(|c| {
                    candidate(
                        c["id"].as_i64().unwrap(),
                        c["baseline"].as_f64().unwrap(),
                        c["eligible"].as_bool().unwrap(),
                    )
                })
                .collect();
            let baseline: Vec<_> = items.iter().map(|c| c.final_score.to_bits()).collect();
            let fusion = fuse_rrf(&mut items, &model, &input, &omega);
            let diagnostics = fusion.traces;
            let reference = scenario["reference"].as_f64().unwrap();
            let active = !flows.is_empty();
            assert_eq!(fusion.active, active, "{}", scenario["name"]);
            let public_output = NativeOutput {
                wormhole_rrf_active: fusion.active,
                ..Default::default()
            };
            assert_eq!(
                public_output.score_kind(),
                if active {
                    "rivermemo_wormhole_rrf"
                } else {
                    "rivermemo_quality"
                }
            );
            assert_eq!(
                serde_json::to_value(public_output).unwrap()["wormholeRrfActive"],
                json!(active)
            );
            let weight = if active {
                flows.iter().copied().fold(0.0_f64, f64::max) / reference
                    * omega_value.powf(input.config.omega_gamma.max(0.0))
            } else {
                0.0
            };
            for (i, item) in items.iter().enumerate() {
                assert_eq!(item.final_score.to_bits(), baseline[i]);
                let expected = if active {
                    1.0 / (61 + i) as f64 + if i > 0 { weight / (60 + i) as f64 } else { 0.0 }
                } else {
                    item.final_score
                };
                assert!(
                    (item.ranking_score - expected).abs() <= 1e-15,
                    "{}",
                    scenario["name"]
                );
                if reference > 0.0 {
                    assert_eq!(
                        diagnostics[&item.curve.id]["fusedScore"],
                        json!(item.ranking_score)
                    );
                }
            }
            items.sort_by(|a, b| b.ranking_score.total_cmp(&a.ranking_score));
            assert_eq!(
                json!(items.iter().map(|c| c.curve.id).collect::<Vec<_>>()),
                scenario["expectedOrder"]
            );
            assert_eq!(
                items
                    .iter()
                    .filter(|c| c.final_score >= fixture["threshold"].as_f64().unwrap())
                    .count(),
                2
            );
            assert_eq!(
                items.iter().find(|c| c.curve.id == 10).unwrap().final_score,
                0.4
            );
        }
    }
    assert!(
        fuse_rrf(
            &mut [],
            &Model::default(),
            &serde_json::from_value(json!({"query":{"vector":[1,0]},"queryState":{}})).unwrap(),
            &omega
        )
        .traces
        .is_empty()
    );
}

#[test]
fn inactive_keys_keep_complete_baseline_tie_order() {
    let input: NativeInput =
        serde_json::from_value(json!({"query":{"vector":[1,0]},"queryState":{}})).unwrap();
    let omega = compute_omega(&input);
    let mut items = vec![
        candidate(30, 0.6, false),
        candidate(10, 0.6, false),
        candidate(20, 0.6, false),
    ];
    let fusion = fuse_rrf(&mut items, &Model::default(), &input, &omega);
    assert!(!fusion.active);
    items.sort_by(|a, b| b.ranking_score.total_cmp(&a.ranking_score));
    assert_eq!(
        items.iter().map(|c| c.curve.id).collect::<Vec<_>>(),
        vec![30, 10, 20]
    );
}
