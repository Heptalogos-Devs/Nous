//! Immutable numerical inputs. Storage, identity and authorization remain with the caller.
use std::collections::BTreeMap;
use std::collections::HashMap;
use std::sync::Arc;

pub type TagVectorPool = HashMap<i64, Arc<[f32]>>;

#[derive(Clone)]
pub struct CandidateData {
    pub id: i64,
    pub file_id: i64,
    pub vector: Arc<[f32]>,
}

#[derive(Clone)]
pub struct FileData {
    pub diary_name: String,
    pub tags: Vec<crate::artifact::FileTag>,
}

#[derive(Clone)]
pub struct MemoAssetInput {
    pub dimension: usize,
    pub signature: String,
    pub generation: String,
    pub graph: Arc<crate::graph::NativeArtifact>,
    pub tag_vectors: TagVectorPool,
    pub tag_names: BTreeMap<i64, String>,
    pub epa: Option<Arc<crate::pipeline::EpaBasis>>,
    pub candidates: BTreeMap<i64, CandidateData>,
    pub files: BTreeMap<i64, FileData>,
}

/// A validated immutable generation. Validation runs once, before query kernels.
#[derive(Clone)]
pub struct MemoAssets {
    pub(crate) dimension: usize,
    pub(crate) signature: String,
    pub(crate) generation: String,
    pub(crate) graph: Arc<crate::graph::NativeArtifact>,
    pub(crate) tag_vectors: TagVectorPool,
    pub(crate) tag_names: BTreeMap<i64, String>,
    pub(crate) epa: Option<Arc<crate::pipeline::EpaBasis>>,
    pub(crate) candidates: BTreeMap<i64, CandidateData>,
    pub(crate) files: BTreeMap<i64, FileData>,
}

#[derive(Debug, serde::Serialize)]
pub struct AssetDiagnostics {
    pub missing_vector_tag_ids: Vec<i64>,
}

impl MemoAssets {
    pub fn try_new(input: MemoAssetInput) -> std::result::Result<Self, String> {
        if input.dimension == 0
            || input.signature.trim().is_empty()
            || input.generation.trim().is_empty()
        {
            return Err(
                "MEMO_ASSET_IDENTITY_INVALID: dimension, generation and signature are required"
                    .into(),
            );
        }
        input.graph.validate()?;
        let vector_valid = |vector: &[f32]| {
            vector.len() == input.dimension && vector.iter().all(|value| value.is_finite())
        };
        for (id, vector) in &input.tag_vectors {
            if *id <= 0 || !vector_valid(vector) {
                return Err(format!("MEMO_TAG_VECTOR_INVALID: {id}"));
            }
        }
        for (id, name) in &input.tag_names {
            if *id <= 0 || name.trim().is_empty() {
                return Err(format!("MEMO_TAG_NAME_INVALID: {id}"));
            }
        }
        if input
            .graph
            .node_ids
            .iter()
            .chain(input.tag_vectors.keys())
            .any(|id| !input.tag_names.contains_key(id))
        {
            return Err(
                "MEMO_TAG_ID_UNKNOWN: graph and vector IDs require a declared Tag name".into(),
            );
        }
        for (id, file) in &input.files {
            if *id <= 0
                || file
                    .tags
                    .iter()
                    .any(|row| row.file_id != *id || !input.tag_names.contains_key(&row.tag_id))
            {
                return Err(format!("MEMO_FILE_MEMBERSHIP_INVALID: {id}"));
            }
        }
        for (id, candidate) in &input.candidates {
            if *id <= 0
                || *id != candidate.id
                || !input.files.contains_key(&candidate.file_id)
                || !vector_valid(&candidate.vector)
            {
                return Err(format!("MEMO_CANDIDATE_VECTOR_INVALID: {id}"));
            }
        }
        if let Some(epa) = &input.epa
            && (epa.dimension != input.dimension
                || !vector_valid(&epa.mean)
                || epa.bases.iter().any(|basis| !vector_valid(basis))
                || !epa.baseline_coverage.is_finite()
                || !epa.background_ratio.is_finite()
                || epa.semantic_count > epa.bases.len())
        {
            return Err("MEMO_EPA_BASIS_INVALID".into());
        }
        Ok(Self {
            dimension: input.dimension,
            signature: input.signature,
            generation: input.generation,
            graph: input.graph,
            tag_vectors: input.tag_vectors,
            tag_names: input.tag_names,
            epa: input.epa,
            candidates: input.candidates,
            files: input.files,
        })
    }
    pub fn dimension(&self) -> usize {
        self.dimension
    }
    pub fn generation(&self) -> &str {
        &self.generation
    }
    pub fn signature(&self) -> &str {
        &self.signature
    }
    pub fn tag_vectors(&self) -> &TagVectorPool {
        &self.tag_vectors
    }
    pub fn files(&self) -> &BTreeMap<i64, FileData> {
        &self.files
    }
    pub fn candidates(&self) -> &BTreeMap<i64, CandidateData> {
        &self.candidates
    }
    pub fn diagnostics(&self) -> AssetDiagnostics {
        AssetDiagnostics {
            missing_vector_tag_ids: self
                .tag_names
                .keys()
                .filter(|id| !self.tag_vectors.contains_key(id))
                .copied()
                .collect(),
        }
    }
    pub fn snapshot(&self) -> MemoAssetInput {
        MemoAssetInput {
            dimension: self.dimension,
            signature: self.signature.clone(),
            generation: self.generation.clone(),
            graph: self.graph.clone(),
            tag_vectors: self.tag_vectors.clone(),
            tag_names: self.tag_names.clone(),
            epa: self.epa.clone(),
            candidates: self.candidates.clone(),
            files: self.files.clone(),
        }
    }
}

pub(crate) fn validate_finite_config(
    value: &impl serde::Serialize,
) -> std::result::Result<(), String> {
    fn has_null(value: &serde_json::Value) -> bool {
        match value {
            serde_json::Value::Null => true,
            serde_json::Value::Array(values) => values.iter().any(has_null),
            serde_json::Value::Object(values) => values.values().any(has_null),
            _ => false,
        }
    }
    let value =
        serde_json::to_value(value).map_err(|error| format!("MEMO_CONFIG_INVALID: {error}"))?;
    // Numerical kernel configs contain no optional/null fields. serde_json encodes
    // non-finite Rust floats as null; reject them before any numerical work.
    if has_null(&value) {
        return Err("MEMO_CONFIG_NONFINITE: kernel parameters must be finite".into());
    }
    Ok(())
}

/// A replaceable Tag ANN boundary. The query pipeline owns all subsequent math.
pub trait TagSearch: Sync {
    fn search(&self, vector: &[f32], limit: usize) -> std::result::Result<Vec<(i64, f64)>, String>;
}
impl<F> TagSearch for F
where
    F: Fn(&[f32], usize) -> std::result::Result<Vec<(i64, f64)>, String> + Sync,
{
    fn search(&self, vector: &[f32], limit: usize) -> std::result::Result<Vec<(i64, f64)>, String> {
        self(vector, limit)
    }
}

#[derive(Debug, Default)]
pub struct VectorLoadStats {
    pub requested_nodes: usize,
    pub fetched_nodes: usize,
    pub source_batches: usize,
}

pub fn ensure_tag_vectors_loaded(
    source: &TagVectorPool,
    target: &mut TagVectorPool,
    ids: &[i64],
    dimension: usize,
) -> std::result::Result<VectorLoadStats, String> {
    let mut stats = VectorLoadStats {
        requested_nodes: ids
            .iter()
            .filter(|id| !target.contains_key(id))
            .copied()
            .collect::<std::collections::HashSet<_>>()
            .len(),
        ..VectorLoadStats::default()
    };
    for id in ids {
        if let Some(vector) = target.get(id) {
            if vector.len() != dimension || vector.iter().any(|value| !value.is_finite()) {
                return Err(format!("MEMO_TAG_VECTOR_INVALID: cached vector {id}"));
            }
            continue;
        }
        if let Some(vector) = source.get(id) {
            if vector.len() != dimension || vector.iter().any(|value| !value.is_finite()) {
                return Err(format!("invalid Tag vector for {id}"));
            }
            target.insert(*id, vector.clone());
            stats.fetched_nodes += 1;
        }
    }
    stats.source_batches = usize::from(stats.fetched_nodes > 0);
    Ok(stats)
}
