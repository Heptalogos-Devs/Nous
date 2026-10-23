//! Copied from nous-vcp/tests/support/kernel_fixture.rs, the local numeric fixture.
//! This copy builds geometry/assets only; the integration test supplies real USearch.
//! A zero third axis preserves the source geometry in a three-dimensional fixture.
//! Kernel source provenance and licenses remain in nous-vcp/UPSTREAM.md.
use nous_vcp::{artifact::*, assets::*, geometry::*, pipeline::decode_epa_basis};
use serde_json::json;
use std::collections::BTreeMap;
use std::sync::Arc;

pub fn fixture() -> MemoAssets {
    let tags = vec![
        GeometryTag {
            id: 1,
            name: "alpha".into(),
            vector_version: 1,
            vector: vec![1.0, 0.0, 0.0],
        },
        GeometryTag {
            id: 2,
            name: "bridge".into(),
            vector_version: 1,
            vector: vec![0.8, 0.6, 0.0],
        },
        GeometryTag {
            id: 3,
            name: "omega".into(),
            vector_version: 1,
            vector: vec![0.0, 1.0, 0.0],
        },
    ];
    let rows = vec![
        FileTag {
            file_id: 1,
            tag_id: 1,
            position: 1,
        },
        FileTag {
            file_id: 1,
            tag_id: 2,
            position: 2,
        },
        FileTag {
            file_id: 2,
            tag_id: 2,
            position: 1,
        },
        FileTag {
            file_id: 2,
            tag_id: 3,
            position: 2,
        },
        FileTag {
            file_id: 3,
            tag_id: 1,
            position: 1,
        },
        FileTag {
            file_id: 3,
            tag_id: 3,
            position: 2,
        },
    ];
    let geometry=build_geometry(GeometryInput{dimension:3,model_sig:"fixture-space".into(),
        config:json!({"local":{"minNeighbors":1,"maxBasis":2,"minGain":0.0},"global":{"kMax":2,"kSem":2,"minGain":0.0,"minSupport":0.0}}),
        tags:tags.clone(),memberships:rows.clone(),recorded_at_ms:0}).unwrap();
    assert!(!geometry.basis.is_empty());
    for (i, a) in geometry.basis.iter().enumerate() {
        for (j, b) in geometry.basis.iter().enumerate() {
            let dot: f64 = a.iter().zip(b).map(|(x, y)| x * y).sum();
            assert!((dot - if i == j { 1.0 } else { 0.0 }).abs() < 1e-12);
        }
    }
    let vectors: TagVectorPool = tags
        .iter()
        .map(|tag| (tag.id, Arc::from(tag.vector.clone())))
        .collect();
    let artifact = build_artifact(ArtifactInput {
        dimension: 3,
        model_sig: "fixture-space".into(),
        generation: "g1".into(),
        geometry_sig: geometry.signature,
        rows: rows.clone(),
        tag_vectors: vectors.clone(),
        anchor_gain: geometry.anchor_gain,
        config: json!({}),
    })
    .unwrap();
    for row in artifact.graph.row_offsets.windows(2) {
        assert!(artifact.graph.weights[row[0]..row[1]].iter().sum::<f64>() <= 0.95 + 1e-12);
    }
    let candidates = BTreeMap::from([
        (
            1001,
            CandidateData {
                id: 1001,
                file_id: 1,
                source_ids: vec![1],
                vector: Arc::from(vec![1.0, 0.0, 0.0]),
            },
        ),
        (
            1002,
            CandidateData {
                id: 1002,
                file_id: 2,
                source_ids: vec![2],
                vector: Arc::from(vec![0.0, 1.0, 0.0]),
            },
        ),
    ]);
    MemoAssets::try_new(MemoAssetInput {
        dimension: 3,
        signature: artifact.metadata["artifactSig"].as_str().unwrap().into(),
        generation: "g1".into(),
        graph: artifact.graph,
        tag_vectors: vectors,
        tag_names: tags.iter().map(|tag| (tag.id, tag.name.clone())).collect(),
        epa: Some(Arc::new(
            decode_epa_basis(geometry.epa_cache_json, 3).unwrap(),
        )),
        candidates,
        files: (1..=2)
            .map(|id| {
                (
                    id,
                    FileData {
                        diary_name: "fixture".into(),
                        tags: rows
                            .iter()
                            .filter(|row| row.file_id == id)
                            .cloned()
                            .collect(),
                    },
                )
            })
            .collect(),
    })
    .unwrap()
}
