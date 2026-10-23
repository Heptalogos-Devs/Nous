use anyhow::Result;
use nous_retrieval::{serving::*, vector::VectorGenerationIdentity};
use serde_json::{Value, json};
use std::fs;

fn snapshot() -> ServingSnapshot {
    let candidate = |id: &str, vector: Vec<f32>, tags: Vec<(&str, Vec<f32>)>| {
        let root = format!("material:{id}");
        ServingCandidate {
            id: id.into(),
            revision: 1,
            commit_seq: 5,
            vector,
            source_refs: vec![root.clone()],
            tags: tags
                .into_iter()
                .map(|(key, vector)| ServingTag {
                    key: key.into(),
                    label: key.into(),
                    vector,
                    source_ref: root.clone(),
                })
                .collect(),
        }
    };
    let alpha = vec![1., 0., 0.];
    let bridge = vec![0.8, 0.6, 0.];
    let omega = vec![0., 1., 0.];
    let mut snapshot = ServingSnapshot::new(
        VectorGenerationIdentity {
            space: "three-dimensional-fixture".into(),
            producer: "adopted-provider".into(),
            generation: String::new(),
            signature: String::new(),
            watermark: 10,
            dimension: 3,
        },
        vec![
            candidate(
                "a",
                alpha.clone(),
                vec![("alpha", alpha.clone()), ("bridge", bridge.clone())],
            ),
            candidate(
                "b",
                omega.clone(),
                vec![("bridge", bridge), ("omega", omega.clone())],
            ),
            candidate(
                "c",
                vec![0.7, 0.7, 0.],
                vec![("alpha", alpha), ("omega", omega)],
            ),
        ],
    );
    snapshot.geometry_config = json!({"local":{"minNeighbors":1,"maxBasis":2,"minGain":0.0},"global":{"kMax":2,"kSem":2,"minGain":0.0,"minSupport":0.0}});
    snapshot
}
fn profile() -> ServingProfile {
    ServingProfile::Vcp(Box::new(VcpConfig{pipeline:serde_json::from_value(json!({"epaGateMode":"absolute","pyramidTopK":3,"spikeRouting":{"maxSafeHops":2,"firingThreshold":0.0,"minimumInjectedCurrent":0.0,"maxTransitionRecords":128,"dynamicWormholes":false}})).unwrap(),core_tags:vec!["alpha".into()],include_trace:true,include_fields:true,thinking:true,..Default::default()}))
}

#[test]
fn multiple_source_cognition_reopens_complete_pipeline_without_double_graph_evidence() -> Result<()>
{
    let directory = tempfile::tempdir()?;
    let mut snapshot = snapshot();
    let baseline = ServingGeneration::create_or_open(directory.path(), &snapshot)?;
    let graph: Value = serde_json::from_slice(&fs::read(baseline.path().join("graph.json"))?)?;
    let mut joint = snapshot.candidates[0].clone();
    joint.id = "schema:combined".into();
    joint
        .source_refs
        .extend(snapshot.candidates[1].source_refs.clone());
    joint.tags.extend(snapshot.candidates[1].tags.clone());
    snapshot.candidates.push(joint);
    let generation = ServingGeneration::create_or_open(directory.path(), &snapshot)?;
    let derived_graph: Value =
        serde_json::from_slice(&fs::read(generation.path().join("graph.json"))?)?;
    assert_eq!(graph, derived_graph);
    assert_eq!(generation.asset_diagnostics()["source_root_count"], 3);
    assert_eq!(generation.asset_diagnostics()["candidate_count"], 4);
    let before = generation.search(&[1., 0., 0.], 4, profile())?;
    assert_eq!(
        before.source_mappings["schema:combined"].source_refs,
        vec!["material:a", "material:b"]
    );
    assert_eq!(before.diagnostics["thinking"]["status"], "ready");
    assert_eq!(
        before.diagnostics["candidate_lanes"]
            .as_object()
            .unwrap()
            .len(),
        4
    );
    assert!(
        before
            .ranked
            .iter()
            .all(|hit| hit.score.is_finite() && hit.trace.is_some())
    );
    assert!(
        before.diagnostics["fields"]["observation"]["transitions"]
            .as_array()
            .unwrap()
            .len()
            > 1
    );
    drop(generation);
    let restored = ServingGeneration::create_or_open(directory.path(), &snapshot)?;
    assert!(restored.restored());
    let after = restored.search(&[1., 0., 0.], 4, profile())?;
    assert_eq!(before.source_mappings, after.source_mappings);
    assert_eq!(
        before
            .ranked
            .iter()
            .map(|h| (&h.id, h.score))
            .collect::<Vec<_>>(),
        after
            .ranked
            .iter()
            .map(|h| (&h.id, h.score))
            .collect::<Vec<_>>()
    );
    Ok(())
}

#[test]
fn raw_provider_scale_preserves_numeric_readout_and_snapshot_bytes() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let original = snapshot();
    let mut scaled = original.clone();
    for candidate in &mut scaled.candidates {
        for value in &mut candidate.vector {
            *value *= 1.002;
        }
        for tag in &mut candidate.tags {
            for value in &mut tag.vector {
                *value *= 1.002;
            }
        }
    }
    let raw = serde_json::to_vec(&scaled)?;
    let baseline = ServingGeneration::create_or_open(directory.path(), &original)?;
    let derived = ServingGeneration::create_or_open(directory.path(), &scaled)?;
    assert_ne!(
        baseline.identity().generation,
        derived.identity().generation
    );
    let left = baseline.search(&[1., 0., 0.], 3, profile())?;
    let right = derived.search(&[1.002, 0., 0.], 3, profile())?;
    for (a, b) in left.ranked.iter().zip(&right.ranked) {
        assert_eq!(a.id, b.id);
        assert!((a.score - b.score).abs() < 2e-6);
    }
    assert_eq!(raw, serde_json::to_vec(&scaled)?);
    drop(derived);
    let reopened = ServingGeneration::create_or_open(directory.path(), &scaled)?;
    let replay = reopened.search(&[1.002, 0., 0.], 3, profile())?;
    assert_eq!(
        right.ranked.iter().map(|h| h.score).collect::<Vec<_>>(),
        replay.ranked.iter().map(|h| h.score).collect::<Vec<_>>()
    );
    Ok(())
}

#[test]
fn frozen_view_and_explicit_roots_bound_assets_even_without_tags() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let mut scoped = snapshot();
    scoped.candidates.truncate(1);
    let generation = ServingGeneration::create_or_open(directory.path(), &scoped)?;
    assert_eq!(generation.asset_diagnostics()["source_root_count"], 1);
    assert_eq!(generation.asset_diagnostics()["tag_count"], 2);
    assert!(
        generation
            .search(&[0., 0., 1.], 10, ServingProfile::Dense)?
            .ranked
            .iter()
            .all(|hit| hit.id == "a")
    );
    scoped.candidates[0].source_refs.clear();
    assert!(ServingGeneration::create_or_open(directory.path(), &scoped).is_err());
    scoped.candidates[0].source_refs = vec!["material:a".into()];
    scoped.candidates[0].tags.clear();
    let generation = ServingGeneration::create_or_open(directory.path(), &scoped)?;
    let result = generation.search(&[1., 0., 0.], 1, ServingProfile::Vcp(Box::default()))?;
    assert_eq!(result.ranked[0].id, "a");
    assert_eq!(result.diagnostics["profile"], "vcp");
    assert_eq!(result.diagnostics["assets"]["tag_count"], 0);
    let path = generation.path().to_owned();
    drop(generation);
    assert!(ServingGeneration::create_or_open(directory.path(), &scoped)?.restored());
    fs::write(path.join("graph.json"), b"partial")?;
    assert!(ServingGeneration::create_or_open(directory.path(), &scoped).is_err());
    scoped.candidates[0].revision += 1;
    let changed = ServingGeneration::create_or_open(directory.path(), &scoped)?;
    assert_ne!(changed.path(), path);
    assert!(!changed.restored());
    Ok(())
}
