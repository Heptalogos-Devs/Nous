//! Immutable native USearch candidate/Tag generations.
// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
// Adapted from Nous Wave 412b062082afb75ccfae4c3f69fc606d3e3cc862:
// crates/retrieval/src/vcp/index.rs and mechanisms/dense.rs. See ../UPSTREAM.md.
// The query authority supplies an already-authorized snapshot and owns source refs.

use anyhow::{Context, Result, ensure};
use nous_vcp::assets::TagSearch;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File},
    io::{Read, Write},
    path::Path,
};
use usearch::{Index, IndexOptions, MetricKind, ScalarKind};

const FORMAT_VERSION: u32 = 1;
const ENGINE: &str = "usearch-2.26.2/cos-f32/l2-f32";
const CANDIDATES_FILE: &str = "candidates.usearch";
const TAGS_FILE: &str = "tags.usearch";
const MANIFEST_FILE: &str = "manifest.json";

/// Opaque signatures are supplied by their authority owners; no business DB lives here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VectorGenerationIdentity {
    pub space: String,
    pub producer: String,
    pub generation: String,
    pub signature: String,
    pub watermark: i64,
    pub dimension: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VectorRecord {
    pub id: i64,
    /// Caller-owned stable mapping, interpreted only by the caller.
    pub source_key: String,
    pub vector: Vec<f32>,
}

/// Both lanes must be frozen under the same query view before construction.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VectorSnapshot {
    pub identity: VectorGenerationIdentity,
    pub candidates: Vec<VectorRecord>,
    pub tags: Vec<VectorRecord>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct VectorMatch {
    pub id: i64,
    /// Native cosine distance (smaller is closer).
    pub distance: f32,
    /// 1 - distance (larger is closer).
    pub similarity: f64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RecordIdentity {
    source_key: String,
    source_vector_sha256: String,
    index_vector_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FrozenMetadata {
    identity: VectorGenerationIdentity,
    candidates: BTreeMap<i64, RecordIdentity>,
    tags: BTreeMap<i64, RecordIdentity>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileStamp {
    byte_len: u64,
    sha256: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    format_version: u32,
    engine: String,
    frozen: FrozenMetadata,
    candidates_file: FileStamp,
    tags_file: FileStamp,
}

/// Owns two real HNSW indexes. All integrity walks occur at build/load, never query.
pub struct VectorGeneration {
    frozen: FrozenMetadata,
    candidates: Index,
    tags: Index,
}

impl VectorGeneration {
    /// Build an immutable in-memory generation from the authority-filtered snapshot.
    pub fn build(snapshot: VectorSnapshot) -> Result<Self> {
        let frozen = snapshot_metadata(&snapshot)?;
        let candidates = build_index(snapshot.identity.dimension, &snapshot.candidates)?;
        let tags = build_index(snapshot.identity.dimension, &snapshot.tags)?;
        Ok(Self {
            frozen,
            candidates,
            tags,
        })
    }

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
            format_version: FORMAT_VERSION,
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
            manifest.format_version == FORMAT_VERSION && manifest.engine == ENGINE,
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

    pub fn identity(&self) -> &VectorGenerationIdentity {
        &self.frozen.identity
    }
    pub fn candidate_count(&self) -> usize {
        self.frozen.candidates.len()
    }
    pub fn tag_count(&self) -> usize {
        self.frozen.tags.len()
    }
    pub fn candidate_source_key(&self, id: i64) -> Option<&str> {
        self.frozen
            .candidates
            .get(&id)
            .map(|record| record.source_key.as_str())
    }
    pub fn tag_source_key(&self, id: i64) -> Option<&str> {
        self.frozen
            .tags
            .get(&id)
            .map(|record| record.source_key.as_str())
    }

    /// A further candidate restriction is applied inside native ANN, before Top-K.
    /// `None` uses the complete frozen lane; `Some(empty)` returns no candidates.
    pub fn search_candidates(
        &self,
        vector: &[f32],
        limit: usize,
        allowed: Option<&BTreeSet<i64>>,
    ) -> Result<Vec<VectorMatch>> {
        self.search_lane(
            &self.candidates,
            &self.frozen.candidates,
            vector,
            limit,
            allowed,
        )
    }

    pub fn search_tags(&self, vector: &[f32], limit: usize) -> Result<Vec<VectorMatch>> {
        self.search_lane(&self.tags, &self.frozen.tags, vector, limit, None)
    }

    fn search_lane(
        &self,
        index: &Index,
        records: &BTreeMap<i64, RecordIdentity>,
        vector: &[f32],
        limit: usize,
        allowed: Option<&BTreeSet<i64>>,
    ) -> Result<Vec<VectorMatch>> {
        let query = unit_vector(vector, self.frozen.identity.dimension)?;
        if let Some(ids) = allowed {
            ensure!(
                ids.iter().all(|id| records.contains_key(id)),
                "VECTOR_SCOPE_ID_UNKNOWN: filter exceeds frozen lane"
            );
            if ids.is_empty() {
                return Ok(Vec::new());
            }
        }
        if limit == 0 || records.is_empty() {
            return Ok(Vec::new());
        }
        let count = limit.min(records.len());
        let matches = if let Some(ids) = allowed {
            index.filtered_search(&query, count, |key| {
                i64::try_from(key).is_ok_and(|id| ids.contains(&id))
            })
        } else {
            index.search(&query, count)
        }
        .context("USearch ANN search")?;
        ensure!(
            matches.keys.len() == matches.distances.len(),
            "VECTOR_RESULT_INVALID: native result lengths differ"
        );
        let mut seen = BTreeSet::new();
        matches.keys.into_iter().zip(matches.distances).map(|(key,distance)| {
            let id = i64::try_from(key).context("VECTOR_RESULT_INVALID: key exceeds signed ID range")?;
            ensure!(records.contains_key(&id) && seen.insert(id) && allowed.is_none_or(|ids|ids.contains(&id)) && distance.is_finite(),
                "VECTOR_RESULT_INVALID: native result exceeds frozen lane or has invalid distance");
            Ok(VectorMatch {id, distance, similarity: 1.0 - f64::from(distance)})
        }).collect()
    }
}

/// The VCP kernel sees only the frozen Tag lane and cosine similarities.
impl TagSearch for VectorGeneration {
    fn search(&self, vector: &[f32], limit: usize) -> std::result::Result<Vec<(i64, f64)>, String> {
        self.search_tags(vector, limit)
            .map(|hits| {
                hits.into_iter()
                    .map(|hit| (hit.id, hit.similarity))
                    .collect()
            })
            .map_err(|error| format!("{error:#}"))
    }
}

fn snapshot_metadata(snapshot: &VectorSnapshot) -> Result<FrozenMetadata> {
    let identity = &snapshot.identity;
    ensure!(
        identity.dimension > 0
            && identity.watermark >= 0
            && [
                &identity.space,
                &identity.producer,
                &identity.generation,
                &identity.signature
            ]
            .iter()
            .all(|value| !value.trim().is_empty()),
        "VECTOR_IDENTITY_INVALID: dimension, nonnegative watermark and all signatures required"
    );
    let candidates = record_metadata(&snapshot.candidates, identity.dimension)?;
    let tags = record_metadata(&snapshot.tags, identity.dimension)?;
    Ok(FrozenMetadata {
        identity: identity.clone(),
        candidates,
        tags,
    })
}

fn record_metadata(
    records: &[VectorRecord],
    dimension: usize,
) -> Result<BTreeMap<i64, RecordIdentity>> {
    let mut output = BTreeMap::new();
    let mut source_keys = BTreeSet::new();
    for record in records {
        ensure!(
            record.id > 0 && !record.source_key.trim().is_empty(),
            "VECTOR_RECORD_INVALID: positive ID and source key required"
        );
        ensure!(
            source_keys.insert(&record.source_key),
            "VECTOR_SOURCE_MAPPING_CONFLICT: duplicate source key within one lane"
        );
        let unit = unit_vector(&record.vector, dimension)?;
        let identity = RecordIdentity {
            source_key: record.source_key.clone(),
            source_vector_sha256: vector_hash(&record.vector),
            index_vector_sha256: vector_hash(&unit),
        };
        ensure!(
            output.insert(record.id, identity).is_none(),
            "VECTOR_ID_DUPLICATE: duplicate ID in one lane"
        );
    }
    Ok(output)
}

fn index_options(dimension: usize) -> IndexOptions {
    // Source DenseGeneration settings, with unique F32 cosine vectors.
    IndexOptions {
        dimensions: dimension,
        metric: MetricKind::Cos,
        quantization: ScalarKind::F32,
        connectivity: 16,
        expansion_add: 40,
        expansion_search: 32,
        multi: false,
    }
}

fn build_index(dimension: usize, records: &[VectorRecord]) -> Result<Index> {
    let index = Index::new(&index_options(dimension)).context("USearch initialization")?;
    index
        .reserve(records.len().max(1))
        .context("USearch reserve")?;
    // Stable insertion order for the same frozen source snapshot.
    let ordered: BTreeMap<_, _> = records.iter().map(|record| (record.id, record)).collect();
    for (id, record) in ordered {
        index
            .add(id as u64, &unit_vector(&record.vector, dimension)?)
            .context("USearch add")?;
    }
    Ok(index)
}

fn load_index(
    path: &Path,
    identity: &VectorGenerationIdentity,
    records: &BTreeMap<i64, RecordIdentity>,
) -> Result<Index> {
    let header = Index::metadata(utf8_path(path)?).context("USearch header")?;
    ensure!(
        header.dimensions == identity.dimension as u64
            && header.metric == MetricKind::Cos
            && header.quantization == ScalarKind::F32
            && !header.multi
            && header.count_present == records.len() as u64
            && header.count_deleted == 0,
        "VECTOR_INDEX_FORMAT_MISMATCH: dimension, cosine/F32/unique format or record count changed"
    );
    let index = Index::new(&index_options(identity.dimension)).context("USearch initialization")?;
    index.load(utf8_path(path)?).context("USearch load")?;
    ensure!(
        index.size() == records.len()
            && index.dimensions() == identity.dimension
            && index.metric_kind() == MetricKind::Cos
            && index.scalar_kind() == ScalarKind::F32
            && !index.multi(),
        "VECTOR_INDEX_FORMAT_MISMATCH: loaded index disagrees"
    );
    let mut vector = vec![0.0; identity.dimension];
    for (id, record) in records {
        let count = index
            .get(*id as u64, &mut vector)
            .context("USearch vector readback")?;
        ensure!(
            count == 1
                && vector.iter().all(|value: &f32| value.is_finite())
                && vector_hash(&vector) == record.index_vector_sha256,
            "VECTOR_INDEX_CONTENT_MISMATCH: missing ID or changed stored vector"
        );
    }
    Ok(index)
}

fn unit_vector(vector: &[f32], dimension: usize) -> Result<Vec<f32>> {
    ensure!(
        vector.len() == dimension && vector.iter().all(|value| value.is_finite()),
        "VECTOR_VALUES_INVALID: dimension or non-finite value"
    );
    let norm = vector
        .iter()
        .map(|value| f64::from(*value).powi(2))
        .sum::<f64>()
        .sqrt();
    ensure!(
        norm > 0.0 && norm.is_finite(),
        "VECTOR_ZERO: cosine vectors must be nonzero"
    );
    // Normalization in f64 keeps native F32 cosine away from overflow/underflow.
    Ok(vector
        .iter()
        .map(|value| (f64::from(*value) / norm) as f32)
        .collect())
}

fn vector_hash(vector: &[f32]) -> String {
    let mut hash = Sha256::new();
    for value in vector {
        hash.update(value.to_bits().to_le_bytes());
    }
    format!("{:x}", hash.finalize())
}
fn utf8_path(path: &Path) -> Result<&str> {
    path.to_str()
        .context("VECTOR_PATH_INVALID: USearch needs UTF-8 path")
}
fn stamp(path: &Path) -> Result<FileStamp> {
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
fn verify_stamp(path: &Path, expected: &FileStamp) -> Result<()> {
    let actual = stamp(path)?;
    ensure!(
        actual.byte_len == expected.byte_len && actual.sha256 == expected.sha256,
        "VECTOR_INDEX_CHECKSUM_MISMATCH: incomplete or changed index file"
    );
    Ok(())
}
fn sync_directory(path: &Path) -> Result<()> {
    #[cfg(not(windows))]
    File::open(path)?.sync_all()?;
    #[cfg(windows)]
    let _ = path;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> VectorSnapshot {
        let record = |id, source: &str, vector: Vec<f32>| VectorRecord {
            id,
            source_key: source.into(),
            vector,
        };
        VectorSnapshot {
            identity: VectorGenerationIdentity {
                space: "embedding-space-v1".into(),
                producer: "producer-v1".into(),
                generation: "generation-v1".into(),
                signature: "query-view-v1".into(),
                watermark: 42,
                dimension: 3,
            },
            candidates: vec![
                record(1001, "memory:alpha@1", vec![1.0, 0.0, 0.0]),
                record(1002, "memory:beta@2", vec![0.0, 1.0, 0.0]),
                record(1003, "memory:gamma@1", vec![-1.0, 0.0, 0.0]),
            ],
            tags: vec![
                record(1, "tag:alpha@1", vec![1.0, 0.0, 0.0]),
                record(2, "tag:beta@1", vec![0.0, 1.0, 0.0]),
                record(3, "tag:gamma@1", vec![-1.0, 0.0, 0.0]),
            ],
        }
    }
    #[test]
    fn native_ann_both_lanes_scope_and_persisted_reopen() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let path = temp.path().join("generation");
        let snapshot = fixture();
        let generation = VectorGeneration::create(&path, snapshot.clone())?;
        let hits = generation.search_candidates(&[1.0, 0.0, 0.0], 3, None)?;
        assert_eq!(
            hits.iter().map(|hit| hit.id).collect::<Vec<_>>(),
            vec![1001, 1002, 1003]
        );
        assert!((hits[0].similarity - 1.0).abs() < 1e-6);
        assert!((hits[2].similarity + 1.0).abs() < 1e-6);
        assert_eq!(
            generation.candidate_source_key(1001),
            Some("memory:alpha@1")
        );
        let scoped =
            generation.search_candidates(&[1.0, 0.0, 0.0], 1, Some(&BTreeSet::from([1002])))?;
        assert_eq!(scoped[0].id, 1002);
        assert!(
            generation
                .search_candidates(&[1.0, 0.0, 0.0], 3, Some(&BTreeSet::new()))?
                .is_empty()
        );
        assert!(
            generation
                .search_candidates(&[1.0, 0.0, 0.0], 1, Some(&BTreeSet::from([9999])))
                .is_err()
        );
        let tags =
            TagSearch::search(&generation, &[1.0, 0.0, 0.0], 3).map_err(anyhow::Error::msg)?;
        assert_eq!(
            tags.iter().map(|hit| hit.0).collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
        assert!((tags[2].1 + 1.0).abs() < 1e-6);
        drop(generation);
        let reopened = VectorGeneration::open(&path, &snapshot)?;
        assert_eq!(hits, reopened.search_candidates(&[1.0, 0.0, 0.0], 3, None)?);
        assert_eq!(
            tags,
            TagSearch::search(&reopened, &[1.0, 0.0, 0.0], 3).map_err(anyhow::Error::msg)?
        );
        assert_eq!(reopened.identity(), &snapshot.identity);
        assert_eq!((reopened.candidate_count(), reopened.tag_count()), (3, 3));
        assert!(VectorGeneration::create(&path, snapshot).is_err());
        Ok(())
    }
    #[test]
    fn restore_rejects_changed_identity_mapping_vectors_and_incomplete_pair() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let path = temp.path().join("generation");
        let snapshot = fixture();
        drop(VectorGeneration::create(&path, snapshot.clone())?);
        let mut changed = snapshot.clone();
        changed.identity.watermark += 1;
        assert!(
            VectorGeneration::open(&path, &changed)
                .err()
                .unwrap()
                .to_string()
                .contains("SNAPSHOT_MISMATCH")
        );
        changed = snapshot.clone();
        changed.identity.space = "different-space".into();
        assert!(VectorGeneration::open(&path, &changed).is_err());
        changed = snapshot.clone();
        changed.candidates[0].source_key = "memory:unrelated@9".into();
        assert!(VectorGeneration::open(&path, &changed).is_err());
        changed = snapshot.clone();
        changed.candidates[0].vector[1] = 0.5;
        assert!(VectorGeneration::open(&path, &changed).is_err());
        let tag_path = path.join(TAGS_FILE);
        let original = fs::read(&tag_path)?;
        fs::write(&tag_path, b"partial")?;
        assert!(
            VectorGeneration::open(&path, &snapshot)
                .err()
                .unwrap()
                .to_string()
                .contains("CHECKSUM_MISMATCH")
        );
        fs::remove_file(&tag_path)?;
        assert!(
            VectorGeneration::open(&path, &snapshot)
                .err()
                .unwrap()
                .to_string()
                .contains("INCOMPLETE")
        );
        fs::write(&tag_path, original)?;
        // Even a matching file checksum cannot substitute different IDs/vectors.
        let wrong = build_index(
            3,
            &[
                VectorRecord {
                    id: 999,
                    source_key: "wrong".into(),
                    vector: vec![1.0, 0.0, 0.0],
                },
                VectorRecord {
                    id: 1002,
                    source_key: "b".into(),
                    vector: vec![0.0, 1.0, 0.0],
                },
                VectorRecord {
                    id: 1003,
                    source_key: "c".into(),
                    vector: vec![-1.0, 0.0, 0.0],
                },
            ],
        )?;
        wrong.save(utf8_path(&path.join(CANDIDATES_FILE))?)?;
        let mut manifest: Manifest = serde_json::from_slice(&fs::read(path.join(MANIFEST_FILE))?)?;
        manifest.candidates_file = stamp(&path.join(CANDIDATES_FILE))?;
        fs::write(path.join(MANIFEST_FILE), serde_json::to_vec(&manifest)?)?;
        assert!(
            VectorGeneration::open(&path, &snapshot)
                .err()
                .unwrap()
                .to_string()
                .contains("CONTENT_MISMATCH")
        );
        Ok(())
    }
    #[test]
    fn invalid_snapshots_and_queries_fail_before_native_calls() -> Result<()> {
        let mut snapshot = fixture();
        snapshot.candidates.push(snapshot.candidates[0].clone());
        assert!(VectorGeneration::build(snapshot).is_err());
        let mut snapshot = fixture();
        snapshot.tags[0].id = snapshot.tags[1].id;
        assert!(VectorGeneration::build(snapshot).is_err());
        let mut snapshot = fixture();
        snapshot.tags[0].source_key = snapshot.tags[1].source_key.clone();
        assert!(VectorGeneration::build(snapshot).is_err());
        for vector in [vec![0.0, 0.0, 0.0], vec![f32::NAN, 0.0, 0.0], vec![1.0]] {
            let mut snapshot = fixture();
            snapshot.tags[0].vector = vector.clone();
            assert!(VectorGeneration::build(snapshot).is_err());
            assert!(
                VectorGeneration::build(fixture())?
                    .search_tags(&vector, 1)
                    .is_err()
            );
        }
        let generation = VectorGeneration::build(fixture())?;
        assert_eq!(generation.search_tags(&[f32::MAX, 0.0, 0.0], 1)?[0].id, 1);
        let empty = VectorGeneration::build(VectorSnapshot {
            identity: fixture().identity,
            candidates: vec![],
            tags: vec![],
        })?;
        assert!(
            empty
                .search_candidates(&[1.0, 0.0, 0.0], 1, None)?
                .is_empty()
        );
        Ok(())
    }

    #[test]
    fn overlapping_lane_local_ids_and_source_keys_keep_distinct_ann_and_reopen() -> Result<()> {
        let mut snapshot = fixture();
        snapshot.tags[0].id = 1001;
        snapshot.tags[0].vector = vec![0.0, 1.0, 0.0];
        snapshot.tags[1].source_key = snapshot.candidates[0].source_key.clone();
        snapshot.tags[1].vector = vec![1.0, 0.0, 0.0];
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("lane-local");
        let created = VectorGeneration::create(&path, snapshot.clone())?;
        assert_eq!(
            created.search_candidates(&[1.0, 0.0, 0.0], 1, None)?[0].id,
            1001
        );
        assert_eq!(created.search_tags(&[0.0, 1.0, 0.0], 1)?[0].id, 1001);
        assert_eq!(created.search_tags(&[1.0, 0.0, 0.0], 1)?[0].id, 2);
        assert_eq!(created.candidate_source_key(1001), Some("memory:alpha@1"));
        assert_eq!(created.tag_source_key(1001), Some("tag:alpha@1"));
        assert_eq!(created.tag_source_key(2), Some("memory:alpha@1"));
        drop(created);
        let reopened = VectorGeneration::open(&path, &snapshot)?;
        assert_eq!(
            reopened.search_candidates(&[1.0, 0.0, 0.0], 1, None)?[0].id,
            1001
        );
        assert_eq!(reopened.search_tags(&[0.0, 1.0, 0.0], 1)?[0].id, 1001);
        assert_eq!(reopened.search_tags(&[1.0, 0.0, 0.0], 1)?[0].id, 2);
        assert_eq!(reopened.candidate_source_key(1001), Some("memory:alpha@1"));
        assert_eq!(reopened.tag_source_key(1001), Some("tag:alpha@1"));
        assert_eq!(reopened.tag_source_key(2), Some("memory:alpha@1"));
        Ok(())
    }
}
