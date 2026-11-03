//! Immutable native USearch candidate/Tag generations.
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

const ENGINE: &str = "usearch-2.26.2/cos-f32/l2-f32";
const CANDIDATES_FILE: &str = "candidates.usearch";
const TAGS_FILE: &str = "tags.usearch";
const MANIFEST_FILE: &str = "manifest.json";

/// Opaque signatures are supplied by their authority owners; no business DB lives here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VectorGenerationIdentity {
    pub space: String,
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
}

impl VectorGeneration {
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

    pub fn search_tags_allowed(
        &self,
        vector: &[f32],
        limit: usize,
        allowed: &BTreeSet<i64>,
    ) -> Result<Vec<VectorMatch>> {
        self.search_lane(&self.tags, &self.frozen.tags, vector, limit, Some(allowed))
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

mod snapshot;
use snapshot::snapshot_metadata;
mod engine;
use engine::{build_index, load_index, utf8_path, vector_hash};
mod persistence;

pub(crate) use engine::unit_vector;
