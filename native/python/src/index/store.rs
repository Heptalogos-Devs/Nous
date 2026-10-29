//! Immutable numerical generations and their atomic publication lifecycle.
use super::*;
#[derive(Serialize, Deserialize)]
pub(super) struct BuildData {
    pub(super) references: Vec<String>,
    pub(super) texts: Vec<String>,
    pub(super) assets: Assets,
    pub(super) vectors: Option<Vec<Vec<f32>>>,
    pub(super) tag_vectors: Option<Vec<Vec<f32>>>,
    pub(super) tag_indptr: Vec<u64>,
    pub(super) tag_indices: Vec<u64>,
    pub(super) graph: Option<ServingGraph>,
    pub(super) source_references: Vec<String>,
    pub(super) candidate_source_indptr: Vec<u64>,
    pub(super) candidate_source_indices: Vec<u64>,
}
pub(super) struct Frozen {
    pub(super) generation: String,
    pub(super) data: BuildData,
    pub(super) lexical: RetrievalIndex,
    pub(super) dense: Option<VectorGeneration>,
    pub(super) vcp: Option<ServingGeneration>,
    pub(super) reference_rows: BTreeMap<String, u64>,
}
pub(super) struct StoreState {
    pub(super) path: PathBuf,
    pub(super) pool: rayon::ThreadPool,
    pub(super) closed: AtomicBool,
    pub(super) writer: Mutex<()>,
    pub(super) active: RwLock<()>,
}
pub(super) fn validate_generation(generation: &str) -> Result<()> {
    ensure!(
        generation.starts_with("g-")
            && generation.len() == 34
            && generation[2..].bytes().all(|b| b.is_ascii_hexdigit()),
        "invalid generation identity"
    );
    Ok(())
}
fn validate(data: &BuildData) -> Result<()> {
    let n = data.references.len();
    ensure!(
        data.texts.len() == n,
        "references and texts must have equal length"
    );
    ensure!(
        data.references.iter().all(|value| !value.trim().is_empty()),
        "references must be nonempty"
    );
    ensure!(
        data.references.iter().collect::<BTreeSet<_>>().len() == n,
        "duplicate reference"
    );
    if let Some(vectors) = &data.vectors {
        ensure!(vectors.len() == n, "vector rows must equal candidate count");
    }
    ensure!(
        data.assets
            .tags
            .iter()
            .all(|tag| !tag.key.trim().is_empty() && !tag.label.trim().is_empty()),
        "tag key and label required"
    );
    ensure!(
        data.assets
            .tags
            .iter()
            .map(|tag| &tag.key)
            .collect::<BTreeSet<_>>()
            .len()
            == data.assets.tags.len(),
        "duplicate tag key"
    );
    ensure!(
        data.source_references
            .iter()
            .all(|root| !root.trim().is_empty())
            && data.source_references.iter().collect::<BTreeSet<_>>().len()
                == data.source_references.len(),
        "source references must be unique and nonempty"
    );
    ensure!(
        data.candidate_source_indptr.len() == n + 1
            && data.candidate_source_indptr.first() == Some(&0)
            && data.candidate_source_indptr.last()
                == Some(&(data.candidate_source_indices.len() as u64))
            && data
                .candidate_source_indptr
                .windows(2)
                .all(|p| p[0] <= p[1])
            && data
                .candidate_source_indices
                .iter()
                .all(|id| (*id as usize) < data.source_references.len()),
        "invalid candidate source CSR"
    );
    if let Some(graph) = &data.graph {
        ensure!(
            graph.source_indptr.len() == graph.indices.len() + 1
                && graph.source_indptr.first() == Some(&0)
                && graph.source_indptr.last() == Some(&(graph.source_indices.len() as u64))
                && graph.source_indptr.windows(2).all(|p| p[0] <= p[1])
                && graph
                    .source_indices
                    .iter()
                    .all(|id| (*id as usize) < data.source_references.len()),
            "invalid graph source CSR"
        );
        let t = data.assets.tags.len();
        let m = graph.indices.len();
        ensure!(
            graph.indptr.len() == t + 1
                && graph.indptr.first() == Some(&0)
                && graph.indptr.last() == Some(&(m as u64))
                && graph.indptr.windows(2).all(|pair| pair[0] <= pair[1]),
            "invalid graph CSR offsets"
        );
        ensure!(
            graph.weights.len() == m
                && graph.polarities.len() == m
                && graph.indices.iter().all(|id| (*id as usize) < t)
                && graph.weights.iter().all(|v| v.is_finite() && *v >= 0.0)
                && graph.polarities.iter().all(|p| [-1, 1].contains(p)),
            "invalid graph arrays"
        );
    }
    if data.assets.tags.is_empty() {
        ensure!(
            data.tag_vectors.is_none() && data.tag_indptr.is_empty() && data.tag_indices.is_empty(),
            "tag arrays require tag metadata"
        );
        return Ok(());
    }
    let vectors = data
        .vectors
        .as_ref()
        .context("tag assets require candidate vectors")?;
    let dimension = vectors
        .first()
        .context("tag assets require at least one candidate")?
        .len();
    let tags = data
        .tag_vectors
        .as_ref()
        .context("tag metadata requires tag_vectors")?;
    ensure!(
        tags.len() == data.assets.tags.len() && tags.iter().all(|row| row.len() == dimension),
        "tag matrix shape must equal [tags, candidate dimension]"
    );
    ensure!(
        data.tag_indptr.len() == n + 1
            && data.tag_indptr.first() == Some(&0)
            && data.tag_indptr.last() == Some(&(data.tag_indices.len() as u64))
            && data.tag_indptr.windows(2).all(|pair| pair[0] <= pair[1]),
        "invalid tag CSR offsets"
    );
    ensure!(
        data.tag_indices
            .iter()
            .all(|id| (*id as usize) < tags.len()),
        "tag CSR index out of bounds"
    );
    Ok(())
}
pub(super) fn build(
    state: &StoreState,
    data: BuildData,
    token: &CancellationToken,
) -> Result<Frozen> {
    token.check()?;
    let _active = state
        .active
        .read()
        .map_err(|_| anyhow::anyhow!("native lifecycle poisoned"))?;
    validate(&data)?;
    let _writer = state
        .writer
        .lock()
        .map_err(|_| anyhow::anyhow!("native writer poisoned"))?;
    ensure!(!state.closed.load(Ordering::Acquire), "index store closed");
    let generation = format!("g-{}", uuid::Uuid::new_v4().simple());
    let staging = tempfile::Builder::new()
        .prefix(".building-")
        .tempdir_in(&state.path)
        .with_context(|| {
            format!(
                "create native generation staging in {}",
                state.path.display()
            )
        })?;
    let mut frozen = assemble(staging.path(), generation.clone(), data, true, Some(token))?;
    token.check()?;
    let bytes = rmp_serde::to_vec_named(&frozen.data)?;
    let mut file = std::fs::File::create(staging.path().join("snapshot.msgpack"))
        .context("create native generation snapshot")?;
    use std::io::Write;
    file.write_all(&bytes)
        .context("write native generation snapshot")?;
    file.sync_all().context("sync native generation snapshot")?;
    sync_directory(staging.path())?;
    std::fs::rename(staging.path(), state.path.join(&generation))
        .context("publish native generation directory")?;
    sync_directory(&state.path)?;
    // Readers and USearch arrays own their data; publication cannot invalidate them.
    frozen.generation = generation;
    Ok(frozen)
}
pub(super) fn load(path: &Path, generation: String) -> Result<Frozen> {
    let data = rmp_serde::from_read(std::fs::File::open(path.join("snapshot.msgpack"))?)?;
    validate(&data)?;
    assemble(path, generation, data, false, None)
}
fn assemble(
    path: &Path,
    generation: String,
    data: BuildData,
    create: bool,
    cancel: Option<&CancellationToken>,
) -> Result<Frozen> {
    let check = || cancel.map_or(Ok(()), CancellationToken::check);
    check()?;
    let lexical =
        RetrievalIndex::open(path.join("lexical")).context("open native lexical generation")?;
    if create {
        let documents = data
            .references
            .iter()
            .zip(&data.texts)
            .map(|(id, text)| IndexDocument {
                id: id.clone(),
                namespace_id: "snapshot".into(),
                subject_id: "snapshot".into(),
                commit_seq: 1,
                text: text.clone(),
                deleted: false,
            })
            .collect::<Vec<_>>();
        lexical
            .rebuild(&documents, 1)
            .context("build native lexical generation")?;
    }
    lexical.close().context("close native lexical writer")?;
    check()?;
    let mut dense = None;
    let mut vcp = None;
    if let Some(vectors) = &data.vectors {
        if !vectors.is_empty() {
            let identity = VectorGenerationIdentity {
                space: data.assets.space.clone(),
                generation: generation.clone(),
                signature: generation.clone(),
                watermark: 1,
                dimension: vectors[0].len(),
            };
            let records = data
                .references
                .iter()
                .zip(vectors)
                .enumerate()
                .map(|(i, (reference, vector))| VectorRecord {
                    id: i as i64 + 1,
                    source_key: reference.clone(),
                    vector: vector.clone(),
                })
                .collect();
            let snapshot = VectorSnapshot {
                identity: identity.clone(),
                candidates: records,
                tags: vec![],
            };
            dense = Some(if create {
                VectorGeneration::create(&path.join("dense"), snapshot)
                    .context("build native dense generation")?
            } else {
                VectorGeneration::open(&path.join("dense"), &snapshot)?
            });
            check()?;
            if let Some(tag_vectors) = &data.tag_vectors {
                let candidate_sources = (0..data.references.len())
                    .map(|row| {
                        data.candidate_source_indices[data.candidate_source_indptr[row] as usize
                            ..data.candidate_source_indptr[row + 1] as usize]
                            .iter()
                            .map(|index| data.source_references[*index as usize].clone())
                            .collect::<Vec<_>>()
                    })
                    .collect::<Vec<_>>();
                let candidates = data
                    .references
                    .iter()
                    .zip(vectors)
                    .enumerate()
                    .map(|(row, (reference, vector))| {
                        let tags = data.tag_indices
                            [data.tag_indptr[row] as usize..data.tag_indptr[row + 1] as usize]
                            .iter()
                            .flat_map(|index| {
                                let id = *index as usize;
                                let tag = &data.assets.tags[id];
                                candidate_sources[row]
                                    .iter()
                                    .map(move |source_ref| ServingTag {
                                        key: tag.key.clone(),
                                        label: tag.label.clone(),
                                        vector: tag_vectors[id].clone(),
                                        source_ref: source_ref.clone(),
                                    })
                            })
                            .collect();
                        ServingCandidate {
                            id: reference.clone(),
                            commit_seq: 1,
                            vector: vector.clone(),
                            source_refs: candidate_sources[row].clone(),
                            tags,
                        }
                    })
                    .collect();
                let snapshot = ServingSnapshot {
                    identity,
                    candidates,
                    tags: data
                        .assets
                        .tags
                        .iter()
                        .zip(tag_vectors)
                        .map(|(tag, vector)| ServingTag {
                            key: tag.key.clone(),
                            label: tag.label.clone(),
                            vector: vector.clone(),
                            source_ref: tag.key.clone(),
                        })
                        .collect(),
                    graph: data.graph.clone(),
                    geometry_config: data.assets.geometry_config.clone(),
                    artifact_config: data.assets.artifact_config.clone(),
                };
                std::fs::create_dir_all(path.join("vcp"))?;
                vcp = Some(ServingGeneration::create_or_open(
                    &path.join("vcp"),
                    &snapshot,
                )?);
                check()?;
            }
        }
    }
    let reference_rows = data
        .references
        .iter()
        .enumerate()
        .map(|(row, reference)| (reference.clone(), row as u64))
        .collect();
    Ok(Frozen {
        generation,
        data,
        lexical,
        dense,
        vcp,
        reference_rows,
    })
}

fn sync_directory(path: &Path) -> Result<()> {
    #[cfg(not(windows))]
    std::fs::File::open(path)?.sync_all()?;
    #[cfg(windows)]
    let _ = path;
    Ok(())
}
