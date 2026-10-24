//! Complete ANN pair publication and verified restoration.
use super::*;

impl VectorGeneration {
    /// Publish both indexes and metadata by renaming a complete same-parent directory.
    /// `path` must name a new generation; its parent is provided/owned by the caller.
    pub fn create(path: &Path, snapshot: VectorSnapshot) -> Result<Self> {
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
        let generation = Self::build(snapshot)?;
        let staging = tempfile::Builder::new()
            .prefix(".nous-ann-")
            .tempdir_in(parent)?;
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
            .open(&candidates_path)?
            .sync_all()?;
        File::options().write(true).open(&tags_path)?.sync_all()?;
        let manifest = Manifest {
            engine: ENGINE.into(),
            frozen: generation.frozen.clone(),
            candidates_file: stamp(&candidates_path)?,
            tags_file: stamp(&tags_path)?,
        };
        let mut file = File::create(staging.path().join(MANIFEST_FILE))?;
        file.write_all(&serde_json::to_vec(&manifest)?)?;
        file.sync_all()?;
        drop(file);
        sync_directory(staging.path())?;
        fs::rename(staging.path(), path)
            .context("VECTOR_PUBLISH_FAILED: complete generation rename")?;
        sync_directory(parent)?;
        Ok(generation)
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
