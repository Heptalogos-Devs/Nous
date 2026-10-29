//! Frozen VCP serving assembled from authority-filtered source material.
// Numerical computation uses pinned nous-vcp kernels with explicit source sets;
use crate::vector::{
    VectorGeneration, VectorGenerationIdentity, VectorRecord, VectorSnapshot, unit_vector,
};
use anyhow::{Context, Result, ensure};
use nous_vcp::{
    artifact::{ArtifactInput, FileTag, build_artifact},
    assets::{CandidateData, FileData, MemoAssetInput, MemoAssets, TagVectorPool},
    geometry::{GeometryInput, GeometryTag, build_geometry},
    graph::NativeArtifact,
    pipeline::{GhostTagInput, PipelineConfig, PipelineInput, decode_epa_basis, run_pipeline},
    river::{CandidateInput, NativeConfig, ReadoutRequest, readout},
    thinking::{Stage, ThinkingInput, plan},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::Arc,
};
const VECTOR_POLICY: &str = "raw-f32/l2-f64-to-unit-f32";
fn empty_object() -> Value {
    json!({})
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ServingTag {
    pub key: String,
    pub label: String,
    pub vector: Vec<f32>,
    pub source_ref: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServingCandidate {
    pub id: String,
    pub commit_seq: i64,
    pub vector: Vec<f32>,
    pub source_refs: Vec<String>,
    #[serde(default)]
    pub tags: Vec<ServingTag>,
}
/// Actual lexical hits from the caller's same frozen authorized query view.
pub struct ServingLexicalHit {
    pub reference: String,
    pub score: f64,
}
pub struct ServingQueryScope<'a> {
    pub references: &'a BTreeSet<String>,
    pub tag_keys: Option<&'a BTreeSet<String>>,
    pub lexical: &'a [ServingLexicalHit],
}
struct ScopedVcpCandidates<'a> {
    allowed: Option<&'a BTreeSet<i64>>,
    tags: Option<&'a BTreeSet<i64>>,
    lexical: &'a BTreeMap<i64, f64>,
}
/// Explicit signed directed edges in the declared Tag row order.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServingGraph {
    pub tag_keys: Vec<String>,
    pub indptr: Vec<u64>,
    pub indices: Vec<u64>,
    pub weights: Vec<f64>,
    pub polarities: Vec<i8>,
    pub source_references: Vec<String>,
    pub source_indptr: Vec<u64>,
    pub source_indices: Vec<u64>,
}
/// The caller freezes scope, source, time and activation before constructing this input.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServingSnapshot {
    pub identity: VectorGenerationIdentity,
    pub candidates: Vec<ServingCandidate>,
    #[serde(default)]
    pub tags: Vec<ServingTag>,
    #[serde(default)]
    pub graph: Option<ServingGraph>,
    #[serde(default = "empty_object")]
    pub geometry_config: Value,
    #[serde(default = "empty_object")]
    pub artifact_config: Value,
}
impl ServingSnapshot {
    pub fn new(identity: VectorGenerationIdentity, candidates: Vec<ServingCandidate>) -> Self {
        Self {
            identity,
            candidates,
            tags: vec![],
            graph: None,
            geometry_config: empty_object(),
            artifact_config: empty_object(),
        }
    }
}
#[derive(Default, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ServingProfile {
    #[default]
    Dense,
    Vcp(Box<VcpConfig>),
}
#[derive(Serialize, Deserialize)]
#[serde(default)]
pub struct VcpConfig {
    pub pipeline: PipelineConfig,
    pub readout: NativeConfig,
    pub candidate_limit: usize,
    pub core_tags: Vec<String>,
    pub ghost_tags: Vec<GhostTagInput>,
    pub include_trace: bool,
    pub include_fields: bool,
    pub thinking: bool,
}
impl Default for VcpConfig {
    fn default() -> Self {
        Self {
            pipeline: PipelineConfig::default(),
            readout: NativeConfig::default(),
            candidate_limit: 100,
            core_tags: vec![],
            ghost_tags: vec![],
            include_trace: false,
            include_fields: false,
            thinking: false,
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ServingSourceMapping {
    pub source_refs: Vec<String>,
}
#[derive(Debug, Serialize)]
pub struct ServingHit {
    pub id: String,
    pub commit_seq: i64,
    pub score: f64,
    pub trace: Option<Value>,
}
#[derive(Debug, Serialize)]
pub struct ServingResult {
    pub ranked: Vec<ServingHit>,
    pub generation: String,
    pub space: String,
    pub source_mappings: BTreeMap<String, ServingSourceMapping>,
    pub diagnostics: Value,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct CandidateMapping {
    id: String,
    commit_seq: i64,
    file_id: i64,
    source_refs: Vec<String>,
    tag_keys: Vec<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct Mappings {
    candidates: BTreeMap<i64, CandidateMapping>,
    tags: BTreeMap<i64, String>,
    roots: BTreeMap<i64, String>,
    /// Retrieval curve IDs map to true roots; only roots contribute graph mass.
    source_sets: BTreeMap<i64, Vec<i64>>,
}

/// Immutable group. App owns any in-candidate generation cache and publication gate.
pub struct ServingGeneration {
    identity: VectorGenerationIdentity,
    prepared: Prepared,
    assets: MemoAssets,
    ann: VectorGeneration,
    mappings: Mappings,
    path: PathBuf,
    restored: bool,
    asset_diagnostics: Value,
}

impl ServingGeneration {
    pub fn identity(&self) -> &VectorGenerationIdentity {
        &self.identity
    }
    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn restored(&self) -> bool {
        self.restored
    }
    pub fn asset_diagnostics(&self) -> &Value {
        &self.asset_diagnostics
    }
}

mod snapshot;
use snapshot::{Prepared, prepare};
mod assets;
use assets::{assemble_assets, asset_diagnostics, build_assets};
mod persistence;
use persistence::{GraphDisk, hash};
mod query;
mod scope;
