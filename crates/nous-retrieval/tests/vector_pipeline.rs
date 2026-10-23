//! Synthetic numerical composition: persisted native ANN -> VCP -> caller source refs.
//! The copied local fixture is in support/kernel_fixture.rs; no model is invoked.
#[path = "support/kernel_fixture.rs"]
mod kernel_fixture;

use anyhow::Result;
use nous_retrieval::vector::{
    VectorGeneration, VectorGenerationIdentity, VectorRecord, VectorSnapshot,
};
use nous_vcp::{
    pipeline::{PipelineInput, run_pipeline},
    river::{CandidateInput, NativeConfig, ReadoutRequest, readout},
    thinking::{Stage, ThinkingInput, plan},
};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};

#[test]
fn reopened_native_ann_drives_full_vcp_readout_sources_and_predecessor_plan() -> Result<()> {
    let assets = kernel_fixture::fixture();
    let source_refs = BTreeMap::from([
        (1001, "memory:fixture/alpha@revision-1"),
        (1002, "memory:fixture/omega@revision-1"),
    ]);
    let snapshot = VectorSnapshot {
        identity: VectorGenerationIdentity {
            space: "fixture-space".into(),
            producer: "frozen-numerical-fixture-v1".into(),
            generation: assets.generation().into(),
            signature: assets.signature().into(),
            watermark: 0,
            dimension: assets.dimension(),
        },
        candidates: assets
            .candidates()
            .iter()
            .map(|(id, candidate)| VectorRecord {
                id: *id,
                source_key: source_refs[id].into(),
                vector: candidate.vector.to_vec(),
            })
            .collect(),
        tags: assets
            .tag_vectors()
            .iter()
            .map(|(id, vector)| VectorRecord {
                id: *id,
                source_key: format!("tag:fixture/{id}@revision-1"),
                vector: vector.to_vec(),
            })
            .collect(),
    };
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("frozen-generation");
    let built = VectorGeneration::create(&path, snapshot.clone())?;
    drop(built);
    let reopened = VectorGeneration::open(&path, &snapshot)?;
    assert_eq!(reopened.identity().generation, assets.generation());
    assert_eq!(reopened.identity().signature, assets.signature());

    let input: PipelineInput = serde_json::from_value(json!({
        "queryId":"native-ann-query", "queryVector":[1.0,0.0,0.0], "coreTags":["alpha"],
        "config":{"epaGateMode":"absolute","pyramidTopK":3,"spikeRouting":{
            "maxSafeHops":2,"firingThreshold":0.0,"minimumInjectedCurrent":0.0,
            "maxTransitionRecords":128,"dynamicWormholes":false}}
    }))?;
    // No substitute search closure: residual Pyramid queries the reopened native index.
    let observation = run_pipeline(&assets, &reopened, input).map_err(anyhow::Error::msg)?;
    assert!(!observation.observation.transitions_truncated);
    assert!(!observation.observation.transitions.is_empty());

    // Both files belong to the frozen authority view. This readout narrows its return
    // projection to file 2; native filtering must retain it even with Top-K = 1.
    let query = &observation.original_query_vector;
    assert_eq!(reopened.search_candidates(query, 1, None)?[0].id, 1001);
    let hits = reopened.search_candidates(query, 1, Some(&BTreeSet::from([1002])))?;
    assert_eq!(
        hits.iter().map(|hit| hit.id).collect::<Vec<_>>(),
        vec![1002]
    );
    let candidates = hits
        .into_iter()
        .map(|hit| CandidateInput {
            id: hit.id,
            score: hit.similarity,
            vector_score: hit.similarity,
            hybrid_score: 0.0,
            bm25_score: 0.0,
            time_score: 0.0,
            anchor_score: 0.0,
        })
        .collect();
    let ranked = readout(
        &assets,
        &observation,
        ReadoutRequest {
            top_k: 1,
            include_trace: true,
            candidates,
            allowed_file_ids: Some(vec![2]),
            config: NativeConfig::default(),
        },
    )
    .map_err(anyhow::Error::msg)?;
    assert_eq!(
        ranked
            .results
            .iter()
            .map(|result| result.id)
            .collect::<Vec<_>>(),
        vec![1002]
    );
    assert!(ranked.results.iter().all(|result| result.score.is_finite()));
    let materialized: Vec<_> = ranked
        .results
        .iter()
        .map(|result| {
            assert_eq!(
                reopened.candidate_source_key(result.id),
                Some(source_refs[&result.id])
            );
            source_refs[&result.id]
        })
        .collect();
    assert_eq!(materialized, vec!["memory:fixture/omega@revision-1"]);

    // Method planning may use both authorized modules and must retain actual hop
    // and predecessor adjacency from the same native-index-driven observation.
    let planned = plan(
        &assets,
        &observation,
        ThinkingInput {
            stages: vec![Stage {
                diary_name: "fixture".into(),
                k: 2,
                candidate_ids: vec![1001, 1002],
            }],
            min_closure: 0.2,
        },
    )
    .map_err(anyhow::Error::msg)?;
    let selections = &planned.stages[0].results;
    let continuation = selections
        .iter()
        .find(|selection| !selection.parents.is_empty())
        .expect("grounded predecessor continuation");
    let edge = continuation
        .route
        .as_ref()
        .expect("continuation transition");
    let parent = selections
        .iter()
        .find(|selection| continuation.parents.contains(&selection.chunk_id))
        .expect("selected predecessor");
    let previous = parent.route.as_ref().expect("predecessor transition");
    assert_eq!(previous.hop + 1, edge.hop);
    assert_eq!(previous.target_id, edge.source_id);
    assert_eq!(edge.previous_id, Some(previous.source_id));
    assert!(!edge.immediate_return);
    assert!(
        observation
            .observation
            .transitions
            .iter()
            .any(|actual| actual.source_id == edge.source_id
                && actual.target_id == edge.target_id
                && actual.previous_id == edge.previous_id
                && actual.hop == edge.hop)
    );
    Ok(())
}
