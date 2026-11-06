#![cfg(windows)]

use std::{fs::File, io::Read, path::Path};

use nous_retrieval::{IndexDocument, RetrievalIndex};

#[test]
fn commit_replaces_metadata_while_previous_readers_remain_open() -> anyhow::Result<()> {
    let temporary_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../temp");
    std::fs::create_dir_all(&temporary_root)?;
    let directory = tempfile::tempdir_in(temporary_root)?;
    let index = RetrievalIndex::open(directory.path())?;
    let meta_path = directory.path().join("meta.json");
    let managed_path = directory.path().join(".managed.json");
    let initial_meta = std::fs::read(&meta_path)?;
    let initial_managed = std::fs::read(&managed_path)?;
    let mut meta_reader = File::open(&meta_path)?;
    let mut managed_reader = File::open(&managed_path)?;

    index.rebuild(
        &[IndexDocument {
            id: "record-one".into(),
            group_row: 0,
            namespace_id: "snapshot".into(),
            subject_id: "snapshot".into(),
            commit_seq: 1,
            text: "Resource ownership remains searchable.".into(),
            deleted: false,
        }],
        1,
    )?;
    index.close()?;

    let mut old_meta = Vec::new();
    let mut old_managed = Vec::new();
    meta_reader.read_to_end(&mut old_meta)?;
    managed_reader.read_to_end(&mut old_managed)?;
    assert_eq!(old_meta, initial_meta);
    assert_eq!(old_managed, initial_managed);
    assert_ne!(std::fs::read(&meta_path)?, initial_meta);
    assert_ne!(std::fs::read(&managed_path)?, initial_managed);
    let reopened = RetrievalIndex::open(directory.path())?;
    assert_eq!(reopened.coverage()?.document_count, 1);
    let result = reopened.search("ownership", "snapshot", "snapshot", 10)?;
    assert_eq!(result.hits.len(), 1);
    assert_eq!(result.hits[0].id, "record-one");
    reopened.close()?;
    Ok(())
}
