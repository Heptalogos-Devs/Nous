use anyhow::Result;
use nous_retrieval::vector::*;
use nous_vcp::assets::TagSearch;
use std::{collections::BTreeSet, fs};

fn snapshot() -> VectorSnapshot {
    let record = |id, key: &str, vector| VectorRecord {
        id,
        source_key: key.into(),
        vector,
    };
    VectorSnapshot {
        identity: VectorGenerationIdentity {
            space: "orthogonal-fixture".into(),
            producer: "fixed-provider".into(),
            generation: "frozen-candidates".into(),
            signature: "authorized-input".into(),
            watermark: 42,
            dimension: 3,
        },
        candidates: vec![
            record(1, "candidate:a", vec![1., 0., 0.]),
            record(2, "candidate:b", vec![0., 1., 0.]),
        ],
        tags: vec![
            record(1, "tag:a", vec![0., 1., 0.]),
            record(2, "candidate:a", vec![1., 0., 0.]),
        ],
    }
}

#[test]
fn real_ann_scopes_top_k_and_restores_independent_lane_mappings() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("complete-generation");
    let snapshot = snapshot();
    let generation = VectorGeneration::create(&path, snapshot.clone())?;
    assert_eq!(
        generation.search_candidates(&[1., 0., 0.], 1, None)?[0].id,
        1
    );
    assert_eq!(generation.search_tags(&[1., 0., 0.], 1)?[0].id, 2);
    assert_eq!(generation.search_tags(&[0., 1., 0.], 1)?[0].id, 1);
    assert_eq!(generation.candidate_source_key(1), Some("candidate:a"));
    assert_eq!(generation.tag_source_key(1), Some("tag:a"));
    let scoped = generation.search_candidates(&[1., 0., 0.], 1, Some(&BTreeSet::from([2])))?;
    assert_eq!(scoped[0].id, 2);
    assert!(
        generation
            .search_candidates(&[1., 0., 0.], 2, Some(&BTreeSet::new()))?
            .is_empty()
    );
    assert!(
        generation
            .search_candidates(&[1., 0., 0.], 1, Some(&BTreeSet::from([99])))
            .is_err()
    );
    let tags = TagSearch::search(&generation, &[1., 0., 0.], 2).map_err(anyhow::Error::msg)?;
    assert!((tags[0].1 - 1.).abs() < 1e-6);
    drop(generation);
    let reopened = VectorGeneration::open(&path, &snapshot)?;
    assert_eq!(
        tags,
        TagSearch::search(&reopened, &[1., 0., 0.], 2).map_err(anyhow::Error::msg)?
    );
    assert_eq!(
        scoped,
        reopened.search_candidates(&[1., 0., 0.], 1, Some(&BTreeSet::from([2])))?
    );
    assert!(VectorGeneration::create(&path, snapshot).is_err());
    Ok(())
}

#[test]
fn changed_snapshots_incomplete_pairs_and_invalid_vectors_are_rejected() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("generation");
    let original = snapshot();
    drop(VectorGeneration::create(&path, original.clone())?);
    let mut changed = original.clone();
    changed.identity.watermark += 1;
    assert!(VectorGeneration::open(&path, &changed).is_err());
    changed = original.clone();
    changed.candidates[0].source_key = "different-owner".into();
    assert!(VectorGeneration::open(&path, &changed).is_err());
    changed = original.clone();
    changed.candidates[0].vector[1] = 0.5;
    assert!(VectorGeneration::open(&path, &changed).is_err());
    fs::write(path.join("tags.usearch"), b"partial")?;
    assert!(VectorGeneration::open(&path, &original).is_err());
    fs::remove_file(path.join("tags.usearch"))?;
    assert!(VectorGeneration::open(&path, &original).is_err());
    let mut duplicate = original.clone();
    duplicate.tags.push(duplicate.tags[0].clone());
    assert!(VectorGeneration::build(duplicate).is_err());
    for vector in [vec![0., 0., 0.], vec![f32::NAN, 0., 0.], vec![1.]] {
        let mut invalid = original.clone();
        invalid.tags[0].vector = vector.clone();
        assert!(VectorGeneration::build(invalid).is_err());
        assert!(
            VectorGeneration::build(original.clone())?
                .search_tags(&vector, 1)
                .is_err()
        );
    }
    Ok(())
}
