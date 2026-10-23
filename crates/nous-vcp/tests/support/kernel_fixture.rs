//! Small deterministic numerical replay. The exact scan is a fixture oracle;
//! production callers provide their actual ANN adapter through TagSearch.
use nous_vcp::{artifact::*, assets::*, geometry::*, pipeline::*};
use serde_json::json;
use std::collections::BTreeMap;
use std::sync::Arc;

pub fn fixture() -> (MemoAssets, PipelineOutput) {
    let tags = vec![
        GeometryTag {
            id: 1,
            name: "alpha".into(),
            vector_version: 1,
            vector: vec![1.0, 0.0],
        },
        GeometryTag {
            id: 2,
            name: "bridge".into(),
            vector_version: 1,
            vector: vec![0.8, 0.6],
        },
        GeometryTag {
            id: 3,
            name: "omega".into(),
            vector_version: 1,
            vector: vec![0.0, 1.0],
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
    let geometry=build_geometry(GeometryInput{dimension:2,model_sig:"fixture-space".into(),
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
        dimension: 2,
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
                vector: Arc::from(vec![1.0, 0.0]),
            },
        ),
        (
            1002,
            CandidateData {
                id: 1002,
                file_id: 2,
                source_ids: vec![2],
                vector: Arc::from(vec![0.0, 1.0]),
            },
        ),
    ]);
    let assets = MemoAssets::try_new(MemoAssetInput {
        dimension: 2,
        signature: artifact.metadata["artifactSig"].as_str().unwrap().into(),
        generation: "g1".into(),
        graph: artifact.graph,
        tag_vectors: vectors,
        tag_names: tags.iter().map(|tag| (tag.id, tag.name.clone())).collect(),
        epa: Some(Arc::new(
            decode_epa_basis(geometry.epa_cache_json, 2).unwrap(),
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
    .unwrap();
    let input:PipelineInput=serde_json::from_value(json!({"queryId":"fixture-query","queryVector":[1.0,0.0],"coreTags":["alpha"],
        "config":{"epaGateMode":"absolute","pyramidTopK":3,"spikeRouting":{"maxSafeHops":2,"firingThreshold":0.0,
            "minimumInjectedCurrent":0.0,"maxTransitionRecords":128,"dynamicWormholes":false}}})).unwrap();
    // An exact fixture index supplies the replaceable ANN boundary; the original
    // pipeline performs all projections, propagation, fusion and field solves.
    let search = |query: &[f32], limit: usize| -> Result<Vec<(i64, f64)>, String> {
        let qn = query
            .iter()
            .map(|v| f64::from(*v).powi(2))
            .sum::<f64>()
            .sqrt();
        let mut hits: Vec<_> = assets
            .tag_vectors()
            .iter()
            .map(|(id, v)| {
                let norm = v.iter().map(|x| f64::from(*x).powi(2)).sum::<f64>().sqrt();
                let dot = v
                    .iter()
                    .zip(query)
                    .map(|(a, b)| f64::from(*a) * f64::from(*b))
                    .sum::<f64>();
                (
                    *id,
                    if norm * qn > 1e-12 {
                        dot / (norm * qn)
                    } else {
                        0.0
                    },
                )
            })
            .collect();
        hits.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        hits.truncate(limit);
        Ok(hits)
    };
    let output = run_pipeline(&assets, &search, input).unwrap();
    (assets, output)
}
