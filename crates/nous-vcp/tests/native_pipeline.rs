use nous_vcp::thinking::plan as plan_methods;
use nous_vcp::{
    assets::*, geometry::*, graph::NativeArtifact, pipeline::*, river::*, sensing::*, thinking::*,
};
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;

#[path = "support/kernel_fixture.rs"]
mod kernel_fixture;
use kernel_fixture::fixture;

#[test]
fn ags_keeps_analytic_tiny_residual_and_ignores_zero_prior_overflow() {
    let candidate = [AgsCandidate { id: 1, prior: 1.0 }];
    let vector = [1.0, 0.0];
    let output = anchored_gs(
        AgsTarget::Explain(&[1.0, 3e-7]),
        &candidate,
        &[&vector],
        1,
        0.0,
    );
    assert!((output.residual_norm.unwrap() - 3e-7).abs() < 1e-20);
    let vectors = [vec![2.0, 0.0], vec![1e308, 1e308], vec![0.0, 4.0]];
    let refs: Vec<_> = vectors.iter().map(Vec::as_slice).collect();
    let candidates = [
        AgsCandidate { id: 0, prior: 1.0 },
        AgsCandidate { id: 1, prior: 0.0 },
        AgsCandidate { id: 2, prior: 1.0 },
    ];
    let result = anchored_gs(AgsTarget::Span, &candidates, &refs, 3, 0.0);
    assert!(result.basis.iter().flatten().all(|value| value.is_finite()));
    assert!(!result.picked.iter().any(|(id, _)| *id == 1));
}

#[test]
fn dynamic_residual_wormhole_keeps_real_historical_edge() {
    let graph = NativeArtifact {
        node_ids: (1..=26).collect(),
        node_index: (1..=26).enumerate().map(|(i, id)| (id, i)).collect(),
        row_offsets: std::iter::once(0)
            .chain(std::iter::repeat_n(25, 26))
            .collect(),
        targets: (1..=25).collect(),
        weights: (0..25).map(|i| if i < 20 { 0.05 } else { 0.04 }).collect(),
        ..NativeArtifact::default()
    }
    .try_prepared()
    .unwrap();
    let vectors: TagVectorPool = (2..=26)
        .map(|id| {
            (
                id,
                Arc::from(if id == 22 {
                    vec![0.0, 1.0]
                } else {
                    vec![1.0, 0.0]
                }),
            )
        })
        .collect();
    let mut pool = TagVectorPool::new();
    let mut context = DynamicWormholeContext::try_new(
        &vectors,
        &mut pool,
        vec![vec![1.0, 0.0], vec![0.0, 1.0]],
        HashMap::from([(1, 0)]),
    )
    .unwrap();
    let output = sense_with_context(
        &graph,
        "fixture",
        SenseInput {
            query_id: None,
            seeds: vec![SenseSeed {
                id: 1,
                energy: 1.0,
                source_type: "seed".into(),
            }],
            config: SenseConfig {
                max_safe_hops: 1,
                ..SenseConfig::default()
            },
        },
        Some(&mut context),
    )
    .unwrap();
    let rescued = output
        .edges
        .iter()
        .find(|edge| edge.target_id == 22)
        .unwrap();
    assert!(rescued.wormhole);
    assert!((rescued.flow - 0.028).abs() < 1e-12);
    assert_eq!(output.edges.len(), 20);
    assert_eq!(output.diagnostics.dynamic_wormhole_edges_fired, 1);
    assert!(
        output
            .edges
            .iter()
            .all(|edge| (2..=26).contains(&edge.target_id))
    );
}

#[test]
fn complete_assets_pipeline_river_and_thinking_are_connected() {
    let (assets, output) = fixture();
    assert!(!output.observation.nodes.is_empty());
    assert!(!output.observation.transitions.is_empty());
    assert!(output.enhanced_vector.iter().all(|value| value.is_finite()));
    assert!(
        output.metadata["diagnostics"]["dualField"]["localConverged"]
            .as_bool()
            .unwrap()
    );
    let request: ReadoutRequest =
        serde_json::from_value(json!({"topK":2,"includeTrace":true,"allowedFileIds":[1],
        "candidates":[{"id":1001,"score":0.8,"bm25Score":0.5},{"id":1002,"score":0.8}]}))
        .unwrap();
    let ranked = readout(&assets, &output, request).unwrap();
    assert_eq!(ranked.results.len(), 1);
    assert_eq!(ranked.results[0].id, 1001);
    assert!(ranked.results[0].score.is_finite());
    assert!((0.0..=1.0).contains(&ranked.omega.omega));
    let plan:ThinkingInput=serde_json::from_value(json!({"stages":[{"diaryName":"fixture","k":2,"candidateIds":[1001,1002]}],"minClosure":0.2})).unwrap();
    let planned = plan_methods(&assets, &output, plan).unwrap();
    assert!(
        planned
            .stages
            .iter()
            .flat_map(|stage| &stage.results)
            .any(|selection| selection.route.is_some())
    );
    let mut stale = assets.snapshot();
    stale.generation = "g2".into();
    let stale = MemoAssets::try_new(stale).unwrap();
    let request = serde_json::from_value(json!({"candidates":[{"id":1001}]})).unwrap();
    assert!(
        readout(&stale, &output, request)
            .err()
            .unwrap()
            .contains("STALE")
    );
}

#[test]
fn immutable_assets_and_query_boundaries_reject_invalid_state() {
    let (assets, mut output) = fixture();
    let mut input = assets.snapshot();
    input.tag_vectors.insert(1, Arc::from(vec![f32::NAN, 0.0]));
    assert!(
        MemoAssets::try_new(input)
            .err()
            .unwrap()
            .contains("TAG_VECTOR_INVALID")
    );
    let mut input = assets.snapshot();
    input.files.get_mut(&1).unwrap().tags[0].tag_id = 999;
    assert!(
        MemoAssets::try_new(input)
            .err()
            .unwrap()
            .contains("MEMBERSHIP_INVALID")
    );
    let mut input = assets.snapshot();
    input.tag_vectors.remove(&3);
    assert_eq!(
        MemoAssets::try_new(input)
            .unwrap()
            .diagnostics()
            .missing_vector_tag_ids,
        vec![3]
    );
    let graph = NativeArtifact {
        node_ids: vec![1],
        node_index: HashMap::from([(1, 0)]),
        row_offsets: vec![0, 1],
        targets: vec![2],
        weights: vec![1.0],
        ..Default::default()
    };
    assert!(graph.try_prepared().err().unwrap().contains("CSR_INVALID"));

    let input = || serde_json::from_value(json!({"queryVector":[1.0,0.0]})).unwrap();
    let bad_search = |_: &[f32], _: usize| Ok(vec![(999, 0.5)]);
    assert!(
        run_pipeline(&assets, &bad_search, input())
            .err()
            .unwrap()
            .contains("TAG_SEARCH_INVALID")
    );
    let bad_score = |_: &[f32], _: usize| Ok(vec![(1, f64::NAN)]);
    assert!(
        run_pipeline(&assets, &bad_score, input())
            .err()
            .unwrap()
            .contains("TAG_SEARCH_INVALID")
    );
    let mut bad_config: PipelineInput = input();
    bad_config.config.layer_decay = f64::INFINITY;
    assert!(
        run_pipeline(&assets, &bad_search, bad_config)
            .err()
            .unwrap()
            .contains("CONFIG_NONFINITE")
    );

    let empty_scope =
        serde_json::from_value(json!({"allowedFileIds":[],"candidates":[{"id":1001}]})).unwrap();
    assert!(
        readout(&assets, &output, empty_scope)
            .unwrap()
            .results
            .is_empty()
    );
    output.local_vector[0] = f32::NAN;
    let request = serde_json::from_value(json!({"candidates":[{"id":1001}]})).unwrap();
    assert!(
        readout(&assets, &output, request)
            .err()
            .unwrap()
            .contains("QUERY_VECTOR_INVALID")
    );
}
