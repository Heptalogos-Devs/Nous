//! Complete ANN pair publication and verified restoration.
use super::*;

impl VectorGeneration {
    /// Publish both indexes and metadata by renaming a complete same-parent directory.
    /// `path` must name a new generation; its parent is provided/owned by the caller.
    pub fn create(
        path: &Path,
        snapshot: VectorSnapshot,
        cancelled: nous_vcp::CancellationCheck<'_>,
    ) -> Result<Self> {
        let generation = Self::build(snapshot, cancelled)?;
        Self::publish(path, generation)
    }

    fn publish(path: &Path, generation: Self) -> Result<Self> {
        ensure!(
            !path.try_exists()?,
            "VECTOR_GENERATION_EXISTS: destination already exists"
        );
        let parent = path
            .parent()
            .context("VECTOR_PATH_INVALID: generation needs a parent")?;
        ensure!(
            parent.is_dir(),
            "VECTOR_PATH_INVALID: parent directory must exist"
        );
        let staging = tempfile::Builder::new()
            .prefix(".nous-ann-")
            .tempdir_in(parent)
            .context("create native ANN staging directory")?;
        let candidates_path = staging.path().join(CANDIDATES_FILE);
        let tags_path = staging.path().join(TAGS_FILE);
        generation
            .candidates
            .save(utf8_path(&candidates_path)?)
            .context("USearch candidate save")?;
        generation
            .tags
            .save(utf8_path(&tags_path)?)
            .context("USearch Tag save")?;
        File::options()
            .write(true)
            .open(&candidates_path)
            .context("open native ANN candidate file for sync")?
            .sync_all()
            .context("sync native ANN candidate file")?;
        File::options()
            .write(true)
            .open(&tags_path)
            .context("open native ANN Tag file for sync")?
            .sync_all()
            .context("sync native ANN Tag file")?;
        let manifest = Manifest {
            engine: ENGINE.into(),
            frozen: generation.frozen.clone(),
            candidates_file: stamp(&candidates_path)?,
            tags_file: stamp(&tags_path)?,
        };
        let mut file = File::create(staging.path().join(MANIFEST_FILE))
            .context("create native ANN manifest")?;
        file.write_all(&serde_json::to_vec(&manifest)?)
            .context("write native ANN manifest")?;
        file.sync_all().context("sync native ANN manifest")?;
        drop(file);
        sync_directory(staging.path())?;
        fs::rename(staging.path(), path)
            .context("VECTOR_PUBLISH_FAILED: complete generation rename")?;
        sync_directory(parent)?;
        Ok(generation)
    }

    /// Copy the previous generation in memory and change only affected vector IDs.
    pub fn create_incremental(
        path: &Path,
        snapshot: VectorSnapshot,
        previous_path: &Path,
        previous: &VectorSnapshot,
        cancelled: nous_vcp::CancellationCheck<'_>,
    ) -> Result<(Self, serde_json::Value)> {
        let next = snapshot_metadata(&snapshot)?;
        let mut generation = Self::open(previous_path, previous)?;
        let before = &generation.frozen;
        ensure!(
            before.identity.space == next.identity.space
                && before.identity.signature == next.identity.signature
                && before.identity.dimension == next.identity.dimension,
            "VECTOR_IDENTITY_MISMATCH: incremental generation is incompatible"
        );
        let candidates = update_lane(
            &mut generation.candidates,
            &before.candidates,
            &next.candidates,
            &snapshot.candidates,
            &previous_path.join(CANDIDATES_FILE),
            cancelled,
        )?;
        let tags = update_lane(
            &mut generation.tags,
            &before.tags,
            &next.tags,
            &snapshot.tags,
            &previous_path.join(TAGS_FILE),
            cancelled,
        )?;
        generation.frozen = next;
        Ok((
            Self::publish(path, generation)?,
            serde_json::json!({
                "incremental": true, "candidates": candidates, "tags": tags
            }),
        ))
    }

    /// Reopen only the exact expected snapshot, including opaque ID mappings and vectors.
    pub fn open(path: &Path, expected: &VectorSnapshot) -> Result<Self> {
        let expected = snapshot_metadata(expected)?;
        let manifest: Manifest = serde_json::from_reader(
            File::open(path.join(MANIFEST_FILE))
                .context("VECTOR_GENERATION_INCOMPLETE: manifest missing")?,
        )
        .context("VECTOR_MANIFEST_INVALID")?;
        ensure!(
            manifest.engine == ENGINE,
            "VECTOR_FORMAT_MISMATCH: unsupported metadata or engine format"
        );
        ensure!(
            manifest.frozen == expected,
            "VECTOR_SNAPSHOT_MISMATCH: identity, ID mapping or vectors changed"
        );
        let candidates_path = path.join(CANDIDATES_FILE);
        let tags_path = path.join(TAGS_FILE);
        verify_stamp(&candidates_path, &manifest.candidates_file)?;
        verify_stamp(&tags_path, &manifest.tags_file)?;
        let candidates = load_index(&candidates_path, &expected.identity, &expected.candidates)?;
        let tags = load_index(&tags_path, &expected.identity, &expected.tags)?;
        Ok(Self {
            frozen: expected,
            candidates,
            tags,
        })
    }
}

pub(super) fn stamp(path: &Path) -> Result<FileStamp> {
    let mut file = File::open(path)
        .with_context(|| format!("VECTOR_GENERATION_INCOMPLETE: {}", path.display()))?;
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    let mut byte_len = 0;
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hash.update(&buffer[..read]);
        byte_len += read as u64;
    }
    Ok(FileStamp {
        byte_len,
        sha256: format!("{:x}", hash.finalize()),
    })
}
pub(super) fn verify_stamp(path: &Path, expected: &FileStamp) -> Result<()> {
    let actual = stamp(path)?;
    ensure!(
        actual.byte_len == expected.byte_len && actual.sha256 == expected.sha256,
        "VECTOR_INDEX_CHECKSUM_MISMATCH: incomplete or changed index file"
    );
    Ok(())
}
pub(super) fn sync_directory(path: &Path) -> Result<()> {
    #[cfg(not(windows))]
    File::open(path)?.sync_all()?;
    #[cfg(windows)]
    let _ = path;
    Ok(())
}

/// Replace changed IDs in a private loaded index; the borrowed generation remains immutable.
fn update_lane(
    index: &mut Index,
    previous: &BTreeMap<i64, RecordIdentity>,
    next: &BTreeMap<i64, RecordIdentity>,
    records: &[VectorRecord],
    previous_path: &Path,
    cancelled: nous_vcp::CancellationCheck<'_>,
) -> Result<serde_json::Value> {
    let changed = |id: &i64, old: &RecordIdentity| {
        next.get(id)
            .is_none_or(|new| new.index_vector_sha256 != old.index_vector_sha256)
    };
    let mut removed = 0u64;
    for (id, old) in previous {
        nous_vcp::check_cancelled(cancelled).map_err(anyhow::Error::msg)?;
        if changed(id, old) {
            removed += 1;
        }
    }
    let needs_add = |record: &VectorRecord| {
        let new = &next[&record.id];
        previous
            .get(&record.id)
            .is_none_or(|old| old.index_vector_sha256 != new.index_vector_sha256)
    };
    let mut added = 0u64;
    for record in records {
        nous_vcp::check_cancelled(cancelled).map_err(anyhow::Error::msg)?;
        if needs_add(record) {
            added += 1;
        }
    }
    let old_deleted = Index::metadata(utf8_path(previous_path)?)?.count_deleted;
    let rebuilt = old_deleted.saturating_add(removed) > (records.len() as u64 / 2).max(512);
    if rebuilt {
        // Decide before mutating: mass deletion should never remove and re-add
        // thousands of rows only to discard that work during compaction.
        *index = build_index(index.dimensions(), records, cancelled)?;
    } else {
        for (id, old) in previous {
            nous_vcp::check_cancelled(cancelled).map_err(anyhow::Error::msg)?;
            if changed(id, old) {
                ensure!(
                    index.remove(*id as u64)? == 1,
                    "VECTOR_INDEX_CONTENT_MISMATCH: missing previous ID"
                );
            }
        }
        index.reserve(records.len().max(1))?;
        for record in records {
            nous_vcp::check_cancelled(cancelled).map_err(anyhow::Error::msg)?;
            if needs_add(record) {
                index.add(
                    record.id as u64,
                    &unit_vector(&record.vector, record.vector.len())?,
                )?;
            }
        }
    }
    ensure!(
        index.size() == records.len(),
        "VECTOR_INDEX_CONTENT_MISMATCH: updated count"
    );
    Ok(serde_json::json!({"removed": removed, "added": added, "rebuilt": rebuilt}))
}
