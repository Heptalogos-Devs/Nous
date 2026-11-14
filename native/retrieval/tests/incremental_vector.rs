//! Incremental ANN generations must preserve both the old and the new snapshot.
use anyhow::Result;
use nous_retrieval::vector::{
    VectorGeneration, VectorGenerationIdentity, VectorRecord, VectorSnapshot,
};

fn snapshot(generation: &str, records: &[(&str, [f32; 3])]) -> VectorSnapshot {
    VectorSnapshot {
        identity: VectorGenerationIdentity {
            space: "space".into(),
            generation: generation.into(),
            signature: "settings".into(),
            watermark: 0,
            dimension: 3,
        },
        candidates: records
            .iter()
            .enumerate()
            .map(|(i, (key, vector))| VectorRecord {
                id: i as i64 + 1,
                source_key: (*key).into(),
                vector: vector.to_vec(),
            })
            .collect(),
        tags: vec![],
    }
}

#[test]
fn incremental_update_and_delete_preserve_borrowed_generation() -> Result<()> {
    let root = tempfile::tempdir()?;
    let old = snapshot(
        "old",
        &[
            ("a", [1., 0., 0.]),
            ("b", [0., 1., 0.]),
            ("c", [0., 0., 1.]),
        ],
    );
    let newer = snapshot(
        "new",
        &[
            ("a", [0.9, 0.1, 0.]),
            ("c", [0., 0., 1.]),
            ("d", [0., 1., 0.]),
        ],
    );
    let old_path = root.path().join("old");
    let new_path = root.path().join("new");
    let borrowed = VectorGeneration::create(&old_path, old.clone(), None)?;
    let (updated, stats) =
        VectorGeneration::create_incremental(&new_path, newer.clone(), &old_path, &old, None)?;
    assert_eq!(stats["incremental"], true);
    assert_eq!(borrowed.candidate_source_key(2), Some("b"));
    assert_eq!(updated.candidate_source_key(2), Some("c"));
    assert_eq!(updated.candidate_source_key(3), Some("d"));
    let result = updated.search_candidates(&[0., 0., 1.], 1, None, 32)?;
    assert_eq!(result[0].id, 2);
    assert_eq!(
        borrowed.search_candidates(&[0., 1., 0.], 1, None, 32)?[0].id,
        2
    );
    let reopened = VectorGeneration::open(&new_path, &newer)?;
    assert_eq!(
        reopened.search_candidates(&[0., 0., 1.], 1, None, 32)?[0].id,
        2
    );
    assert_eq!(
        VectorGeneration::open(&old_path, &old)?.candidate_source_key(2),
        Some("b")
    );
    Ok(())
}

#[test]
fn mass_deletion_rebuilds_lane_and_reopens_without_changing_old_generation() -> Result<()> {
    let root = tempfile::tempdir()?;
    let mut old = snapshot("large-old", &[]);
    for row in 0..520 {
        let angle = row as f32 * 0.01;
        old.candidates.push(VectorRecord {
            id: row + 1,
            source_key: format!("item-{row}"),
            vector: vec![angle.cos(), angle.sin(), 0.25],
        });
    }
    let mut next = snapshot("large-new", &[]);
    next.candidates.push(old.candidates[0].clone());
    let old_path = root.path().join("old");
    let new_path = root.path().join("new");
    let borrowed = VectorGeneration::create(&old_path, old.clone(), None)?;
    let (_, stats) =
        VectorGeneration::create_incremental(&new_path, next.clone(), &old_path, &old, None)?;
    assert_eq!(stats["candidates"]["rebuilt"], true);
    assert_eq!(borrowed.candidate_count(), 520);
    assert_eq!(
        VectorGeneration::open(&new_path, &next)
            .map_err(|e| anyhow::anyhow!("reopen new: {e:#}"))?
            .candidate_count(),
        1
    );
    assert_eq!(
        VectorGeneration::open(&old_path, &old)?.candidate_count(),
        520
    );
    Ok(())
}

#[test]
fn different_search_efforts_can_share_a_frozen_generation() -> Result<()> {
    let temporary_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../temp");
    std::fs::create_dir_all(&temporary_root)?;
    let root = tempfile::tempdir_in(temporary_root)?;
    let source = snapshot(
        "parallel",
        &[
            ("a", [1., 0., 0.]),
            ("b", [0., 1., 0.]),
            ("c", [0., 0., 1.]),
        ],
    );
    let path = root.path().join("generation");
    let generation = VectorGeneration::create(&path, source.clone(), None)?;
    let barrier = std::sync::Barrier::new(2);
    std::thread::scope(|scope| -> Result<()> {
        let workers = [([1., 0., 0.], 1, 8), ([0., 1., 0.], 2, 128)]
            .into_iter()
            .map(|(query, expected, effort)| {
                let generation = &generation;
                let barrier = &barrier;
                scope.spawn(move || -> Result<()> {
                    barrier.wait();
                    for _ in 0..40 {
                        assert_eq!(
                            generation.search_candidates(&query, 1, None, effort)?[0].id,
                            expected
                        );
                    }
                    Ok(())
                })
            })
            .collect::<Vec<_>>();
        for worker in workers {
            worker
                .join()
                .map_err(|_| anyhow::anyhow!("ANN search thread panicked"))??;
        }
        Ok(())
    })?;
    let reopened = VectorGeneration::open(&path, &source)?;
    assert_eq!(
        reopened.search_candidates(&[0., 0., 1.], 1, None, 64)?[0].id,
        3
    );
    Ok(())
}
