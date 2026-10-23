//! Frozen VCP serving assembled from authority-filtered source material.
// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
// Adapted from previous Nous vcp/{generation,graph,observation,lane,readout}.rs.
// Source commit 412b062082afb75ccfae4c3f69fc606d3e3cc862.
// Numerical computation uses the unchanged pinned nous-vcp kernels; see UPSTREAM.md.
use crate::vector::{VectorGeneration, VectorGenerationIdentity, VectorRecord, VectorSnapshot};
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
const FORMAT_VERSION: u32 = 2;
const FORMAT: &str = "nous-vcp-serving-ags-unit-f32-v2";
const VECTOR_POLICY: &str = "raw-f32/l2-f64-to-unit-f32-v1";
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
pub struct ServingMemory {
    pub id: String,
    pub revision: i64,
    pub commit_seq: i64,
    pub vector: Vec<f32>,
    pub source_ref: String,
    #[serde(default)]
    pub tags: Vec<ServingTag>,
}
/// The caller freezes scope, source, time and activation before constructing this input.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServingSnapshot {
    pub identity: VectorGenerationIdentity,
    pub memories: Vec<ServingMemory>,
    #[serde(default = "empty_object")]
    pub geometry_config: Value,
    #[serde(default = "empty_object")]
    pub artifact_config: Value,
}
impl ServingSnapshot {
    pub fn new(identity: VectorGenerationIdentity, memories: Vec<ServingMemory>) -> Self {
        Self {
            identity,
            memories,
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
    pub revision: i64,
    pub source_ref: String,
}
#[derive(Debug, Serialize)]
pub struct ServingHit {
    pub memory_id: String,
    pub revision: i64,
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
struct MemoryMapping {
    memory_id: String,
    revision: i64,
    commit_seq: i64,
    file_id: i64,
    source_ref: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct Mappings {
    memories: BTreeMap<i64, MemoryMapping>,
    tags: BTreeMap<i64, String>,
    roots: BTreeMap<i64, String>,
}

/// Immutable group. App owns any in-memory generation cache and publication gate.
pub struct ServingGeneration {
    identity: VectorGenerationIdentity,
    assets: MemoAssets,
    ann: VectorGeneration,
    mappings: Mappings,
    path: PathBuf,
    restored: bool,
    asset_diagnostics: Value,
}

impl ServingGeneration {
    pub fn create_or_open(parent: &Path, snapshot: &ServingSnapshot) -> Result<Self> {
        ensure!(
            parent.is_dir(),
            "SERVING_PATH_INVALID: managed parent must exist"
        );
        let prepared = prepare(snapshot)?;
        let path = parent.join(&prepared.identity.generation);
        if path.try_exists()? {
            return Self::open(&path, &prepared);
        }
        let staging = tempfile::Builder::new()
            .prefix(".nous-serving-")
            .tempdir_in(parent)?;
        let (assets, geometry, graph, artifact_meta) = build_assets(&prepared)?;
        let ann = VectorGeneration::create(&staging.path().join("ann"), prepared.vectors.clone())?;
        let geometry_hash = write_json(&staging.path().join("geometry.json"), &geometry)?;
        let graph_hash = write_json(&staging.path().join("graph.json"), &graph)?;
        let mappings_hash = write_json(&staging.path().join("mappings.json"), &prepared.mappings)?;
        let manifest = ServingManifest {
            format: FORMAT.into(),
            version: FORMAT_VERSION,
            digest: prepared.digest.clone(),
            identity: prepared.identity.clone(),
            geometry_hash,
            graph_hash,
            mappings_hash,
            artifact_meta,
        };
        write_json(&staging.path().join("manifest.json"), &manifest)?;
        sync_dir(staging.path())?;
        fs::rename(staging.path(), &path)
            .context("SERVING_PUBLISH_FAILED: complete generation rename")?;
        sync_dir(parent)?;
        let asset_diagnostics = asset_diagnostics(&assets, &prepared.mappings);
        Ok(Self {
            identity: prepared.identity,
            assets,
            ann,
            mappings: prepared.mappings,
            path,
            restored: false,
            asset_diagnostics,
        })
    }
    fn open(path: &Path, prepared: &Prepared) -> Result<Self> {
        let manifest: ServingManifest = read_json(&path.join("manifest.json"))?;
        ensure!(
            manifest.version == FORMAT_VERSION
                && manifest.format == FORMAT
                && manifest.digest == prepared.digest
                && manifest.identity == prepared.identity,
            "SERVING_MANIFEST_MISMATCH: frozen snapshot or format changed"
        );
        let geometry: Value = read_verified(&path.join("geometry.json"), &manifest.geometry_hash)?;
        let graph: GraphDisk = read_verified(&path.join("graph.json"), &manifest.graph_hash)?;
        let mappings: Mappings =
            read_verified(&path.join("mappings.json"), &manifest.mappings_hash)?;
        ensure!(
            mappings == prepared.mappings,
            "SERVING_MAPPING_MISMATCH: frozen source identities changed"
        );
        let graph = graph.restore()?;
        let assets = assemble_assets(prepared, graph, &geometry, &manifest.artifact_meta)?;
        let ann = VectorGeneration::open(&path.join("ann"), &prepared.vectors)?;
        let asset_diagnostics = asset_diagnostics(&assets, &mappings);
        Ok(Self {
            identity: prepared.identity.clone(),
            assets,
            ann,
            mappings,
            path: path.into(),
            restored: true,
            asset_diagnostics,
        })
    }
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
    pub fn search(
        &self,
        query: &[f32],
        limit: usize,
        profile: ServingProfile,
    ) -> Result<ServingResult> {
        ensure!(
            limit > 0,
            "SERVING_LIMIT_INVALID: search limit must be positive"
        );
        match profile {
            ServingProfile::Dense => {
                let hits = self.ann.search_candidates(query, limit, None)?;
                let source_mappings = self.sources_for(hits.iter().map(|hit| hit.id))?;
                let ranked = hits
                    .into_iter()
                    .map(|hit| self.materialize(hit.id, hit.similarity, None))
                    .collect::<Result<_>>()?;
                Ok(ServingResult {
                    ranked,
                    generation: self.identity.generation.clone(),
                    space: self.identity.space.clone(),
                    source_mappings,
                    diagnostics: json!({"profile":"dense","engine":"usearch","restored":self.restored,"assets":self.query_asset_diagnostics()}),
                })
            }
            ServingProfile::Vcp(config) => self.vcp_search(query, limit, *config),
        }
    }
    fn sources_for(
        &self,
        ids: impl Iterator<Item = i64>,
    ) -> Result<BTreeMap<String, ServingSourceMapping>> {
        ids.map(|id| {
            let memory = self
                .mappings
                .memories
                .get(&id)
                .context("SERVING_CANDIDATE_UNKNOWN: source mapping escaped snapshot")?;
            Ok((
                memory.memory_id.clone(),
                ServingSourceMapping {
                    revision: memory.revision,
                    source_ref: memory.source_ref.clone(),
                },
            ))
        })
        .collect()
    }
    fn query_asset_diagnostics(&self) -> Value {
        json!({"candidate_count":self.mappings.memories.len(),"tag_count":self.mappings.tags.len(),"source_root_count":self.mappings.roots.len(),"vector_policy":VECTOR_POLICY,
            "graph_nodes":self.asset_diagnostics["graph_nodes"],"graph_edges":self.asset_diagnostics["graph_edges"],"artifact_signature":self.assets.signature()})
    }
    fn materialize(&self, id: i64, score: f64, trace: Option<Value>) -> Result<ServingHit> {
        let memory = self
            .mappings
            .memories
            .get(&id)
            .context("SERVING_CANDIDATE_UNKNOWN: readout escaped frozen mapping")?;
        ensure!(
            score.is_finite(),
            "SERVING_SCORE_INVALID: non-finite readout score"
        );
        Ok(ServingHit {
            memory_id: memory.memory_id.clone(),
            revision: memory.revision,
            commit_seq: memory.commit_seq,
            score,
            trace,
        })
    }
    fn vcp_search(
        &self,
        query: &[f32],
        limit: usize,
        mut config: VcpConfig,
    ) -> Result<ServingResult> {
        // Derive from the original query once; never mutate adopted provider bytes.
        let normalized_query = unit_vector(query, self.identity.dimension)?;
        for ghost in &mut config.ghost_tags {
            ghost.vector = unit_vector(&ghost.vector, self.identity.dimension)?;
        }
        if config.thinking && config.pipeline.spike_routing.max_transition_records == 0 {
            config.pipeline.spike_routing.max_transition_records = 4096;
        }
        let observation = run_pipeline(
            &self.assets,
            &self.ann,
            PipelineInput {
                query_id: None,
                _query_text: String::new(),
                query_vector: normalized_query,
                core_tags: config.core_tags,
                ghost_tags: config.ghost_tags,
                config: config.pipeline,
            },
        )
        .map_err(anyhow::Error::msg)?;
        let candidate_limit = config
            .candidate_limit
            .max(limit)
            .min(self.mappings.memories.len());
        let mut offered = BTreeMap::<i64, CandidateInput>::new();
        let mut lanes = BTreeMap::new();
        // Copied multi-query offering contract: original/enhanced/local/transfer;
        // one observation, one candidate union, one readout.
        for (name, vector) in [
            // ANN normalizes the raw query itself using the identical policy;
            // feeding this lane the derived unit buffer would normalize it twice.
            ("original", query),
            ("enhanced", observation.enhanced_vector.as_slice()),
            ("local", observation.local_vector.as_slice()),
            ("transfer", observation.transfer_vector.as_slice()),
        ] {
            if vector.iter().all(|value| *value == 0.0) {
                lanes.insert(name, json!({"skipped":"zero field","hits":0}));
                continue;
            }
            let hits = self.ann.search_candidates(vector, candidate_limit, None)?;
            lanes.insert(name, json!({"hits":hits.len()}));
            for hit in hits {
                offered
                    .entry(hit.id)
                    .and_modify(|candidate| {
                        candidate.score = candidate.score.max(hit.similarity);
                        candidate.vector_score = candidate.vector_score.max(hit.similarity);
                    })
                    .or_insert(CandidateInput {
                        id: hit.id,
                        score: hit.similarity,
                        vector_score: hit.similarity,
                        hybrid_score: 0.0,
                        bm25_score: 0.0,
                        time_score: 0.0,
                        anchor_score: 0.0,
                    });
            }
        }
        let candidate_ids = offered.keys().copied().collect::<Vec<_>>();
        let native = readout(
            &self.assets,
            &observation,
            ReadoutRequest {
                top_k: limit,
                include_trace: config.include_trace,
                candidates: offered.into_values().collect(),
                allowed_file_ids: Some(self.mappings.roots.keys().copied().collect()),
                config: config.readout,
            },
        )
        .map_err(anyhow::Error::msg)?;
        let mut source_mappings = self.sources_for(native.results.iter().map(|hit| hit.id))?;
        let ranked = native
            .results
            .iter()
            .map(|hit| {
                self.materialize(
                    hit.id,
                    hit.score,
                    if config.include_trace {
                        Some(serde_json::to_value(hit)?)
                    } else {
                        None
                    },
                )
            })
            .collect::<Result<_>>()?;
        let thinking = if config.thinking {
            match plan(
                &self.assets,
                &observation,
                ThinkingInput {
                    stages: vec![Stage {
                        diary_name: "authorized-snapshot".into(),
                        k: limit.clamp(1, 32),
                        candidate_ids: candidate_ids.into_iter().take(256).collect(),
                    }],
                    min_closure: 0.2,
                },
            ) {
                Ok(value) => {
                    source_mappings.extend(self.sources_for(value.stages.iter().flat_map(
                        |stage| stage.results.iter().map(|selection| selection.chunk_id),
                    ))?);
                    json!({"status":"ready","plan":value})
                }
                Err(error) => json!({"status":"unavailable","reason":error}),
            }
        } else {
            json!({"status":"not_requested"})
        };
        let fields = if config.include_fields {
            json!({"local":observation.local_field,"transfer":observation.transfer_field,
            "local_vector":observation.local_vector,"transfer_vector":observation.transfer_vector,"observation":observation.observation})
        } else {
            Value::Null
        };
        Ok(ServingResult {
            ranked,
            generation: self.identity.generation.clone(),
            space: self.identity.space.clone(),
            source_mappings,
            diagnostics: json!({"profile":"vcp","engine":"usearch+ags+memo+river-v3","restored":self.restored,"assets":self.query_asset_diagnostics(),
                "candidate_lanes":lanes,"pipeline":observation.metadata,"river":native.diagnostics,"omega":native.omega,"thinking":thinking,"fields":fields}),
        })
    }
}

struct Prepared {
    identity: VectorGenerationIdentity,
    digest: String,
    snapshot: ServingSnapshot,
    mappings: Mappings,
    tag_definitions: BTreeMap<String, ServingTag>,
    root_tags: BTreeMap<String, Vec<String>>,
    vectors: VectorSnapshot,
    unit_candidates: BTreeMap<i64, Arc<[f32]>>,
    unit_tags: TagVectorPool,
}
fn prepare(snapshot: &ServingSnapshot) -> Result<Prepared> {
    let mut snapshot = snapshot.clone();
    snapshot.memories.sort_by(|a, b| a.id.cmp(&b.id));
    ensure!(
        snapshot.identity.dimension > 0
            && snapshot.identity.watermark >= 0
            && [
                &snapshot.identity.space,
                &snapshot.identity.producer,
                &snapshot.identity.generation,
                &snapshot.identity.signature
            ]
            .iter()
            .all(|s| !s.trim().is_empty()),
        "SERVING_IDENTITY_INVALID"
    );
    let mut tag_definitions = BTreeMap::<String, ServingTag>::new();
    let mut root_tags = BTreeMap::<String, Vec<String>>::new();
    let mut seen = BTreeSet::new();
    for memory in &mut snapshot.memories {
        ensure!(
            !memory.id.trim().is_empty()
                && seen.insert(memory.id.clone())
                && memory.revision > 0
                && memory.commit_seq >= 0
                && memory.commit_seq <= snapshot.identity.watermark
                && !memory.source_ref.trim().is_empty(),
            "SERVING_MEMORY_INVALID: identity, revision, source root or watermark"
        );
        validate_vector(&memory.vector, snapshot.identity.dimension)?;
        for tag in &memory.tags {
            ensure!(
                !tag.key.trim().is_empty()
                    && !tag.label.trim().is_empty()
                    && tag.source_ref == memory.source_ref,
                "SERVING_TAG_INVALID: Tag key/label or source root"
            );
            validate_vector(&tag.vector, snapshot.identity.dimension)?;
            if let Some(old) = tag_definitions.get(&tag.key) {
                ensure!(
                    old.label == tag.label && old.vector == tag.vector,
                    "SERVING_TAG_CONFLICT: one key has different label/vector"
                );
            } else {
                tag_definitions.insert(tag.key.clone(), tag.clone());
            }
        }
        let mut membership_seen = BTreeSet::new();
        memory
            .tags
            .retain(|tag| membership_seen.insert(tag.key.clone()));
        let ordered = memory
            .tags
            .iter()
            .map(|tag| tag.key.clone())
            .collect::<Vec<_>>();
        if let Some(old) = root_tags.get(&memory.source_ref) {
            ensure!(
                old == &ordered,
                "SERVING_ROOT_MEMBERSHIP_CONFLICT: one root has different ordered Tags"
            );
        } else {
            root_tags.insert(memory.source_ref.clone(), ordered);
        }
    }
    let digest = hash(&serde_json::to_vec(
        &json!({"format":FORMAT,"vector_policy":VECTOR_POLICY,"snapshot":snapshot}),
    )?);
    let mut identity = snapshot.identity.clone();
    identity.generation = format!("g-{}", &digest[..24]);
    identity.signature = digest.clone();
    let roots = root_tags
        .keys()
        .enumerate()
        .map(|(i, root)| Ok((i64::try_from(i + 1)?, root.clone())))
        .collect::<Result<BTreeMap<_, _>>>()?;
    let root_ids = roots
        .iter()
        .map(|(id, root)| (root.clone(), *id))
        .collect::<BTreeMap<_, _>>();
    let tags = tag_definitions
        .keys()
        .enumerate()
        .map(|(i, key)| Ok((i64::try_from(i + 1)?, key.clone())))
        .collect::<Result<BTreeMap<_, _>>>()?;
    let memories = snapshot
        .memories
        .iter()
        .enumerate()
        .map(|(i, memory)| {
            Ok((
                i64::try_from(i + 1)?,
                MemoryMapping {
                    memory_id: memory.id.clone(),
                    revision: memory.revision,
                    commit_seq: memory.commit_seq,
                    file_id: root_ids[&memory.source_ref],
                    source_ref: memory.source_ref.clone(),
                },
            ))
        })
        .collect::<Result<BTreeMap<_, _>>>()?;
    let vectors = VectorSnapshot {
        identity: identity.clone(),
        candidates: snapshot
            .memories
            .iter()
            .zip(memories.keys())
            .map(|(memory, id)| VectorRecord {
                id: *id,
                source_key: format!("memory:{}@{}", memory.id, memory.revision),
                vector: memory.vector.clone(),
            })
            .collect(),
        tags: tags
            .iter()
            .map(|(id, key)| VectorRecord {
                id: *id,
                source_key: key.clone(),
                vector: tag_definitions[key].vector.clone(),
            })
            .collect(),
    };
    // Keep raw ANN inputs for exact original-vector manifests. Both the adapter
    // and these immutable numerical assets derive unit F32 from those raw bytes,
    // using the same f64 accumulation/division. Never feed derived vectors back
    // into snapshot normalization or persistence identity.
    let unit_candidates = vectors
        .candidates
        .iter()
        .map(|record| {
            Ok((
                record.id,
                Arc::from(unit_vector(&record.vector, identity.dimension)?),
            ))
        })
        .collect::<Result<_>>()?;
    let unit_tags = vectors
        .tags
        .iter()
        .map(|record| {
            Ok((
                record.id,
                Arc::from(unit_vector(&record.vector, identity.dimension)?),
            ))
        })
        .collect::<Result<_>>()?;
    Ok(Prepared {
        identity,
        digest,
        snapshot,
        mappings: Mappings {
            memories,
            tags,
            roots,
        },
        tag_definitions,
        root_tags,
        vectors,
        unit_candidates,
        unit_tags,
    })
}
fn validate_vector(vector: &[f32], dimension: usize) -> Result<()> {
    ensure!(
        vector.len() == dimension
            && vector.iter().all(|v| v.is_finite())
            && vector.iter().any(|v| *v != 0.0),
        "SERVING_VECTOR_INVALID: dimension/finite/nonzero"
    );
    Ok(())
}
fn unit_vector(vector: &[f32], dimension: usize) -> Result<Vec<f32>> {
    validate_vector(vector, dimension)?;
    // Identical to vector.rs's native cosine boundary; source bytes remain raw.
    let norm = vector
        .iter()
        .map(|value| f64::from(*value).powi(2))
        .sum::<f64>()
        .sqrt();
    ensure!(
        norm.is_finite() && norm > 0.0,
        "SERVING_VECTOR_INVALID: finite nonzero norm required"
    );
    Ok(vector
        .iter()
        .map(|value| (f64::from(*value) / norm) as f32)
        .collect())
}
fn root_rows(prepared: &Prepared) -> Vec<FileTag> {
    let tags = prepared
        .mappings
        .tags
        .iter()
        .map(|(id, key)| (key, *id))
        .collect::<BTreeMap<_, _>>();
    prepared
        .mappings
        .roots
        .iter()
        .flat_map(|(id, root)| {
            prepared.root_tags[root]
                .iter()
                .enumerate()
                .map(|(position, key)| FileTag {
                    file_id: *id,
                    tag_id: tags[key],
                    position: position as i64 + 1,
                })
                .collect::<Vec<_>>()
        })
        .collect()
}
fn build_assets(prepared: &Prepared) -> Result<(MemoAssets, Value, GraphDisk, Value)> {
    let rows = root_rows(prepared);
    let tags = prepared
        .mappings
        .tags
        .iter()
        .map(|(id, key)| {
            let tag = &prepared.tag_definitions[key];
            GeometryTag {
                id: *id,
                name: tag.label.clone(),
                vector_version: 1,
                vector: prepared.unit_tags[id].to_vec(),
            }
        })
        .collect();
    let geometry = build_geometry(GeometryInput {
        dimension: prepared.identity.dimension,
        model_sig: prepared.identity.space.clone(),
        config: prepared.snapshot.geometry_config.clone(),
        tags,
        memberships: rows.clone(),
        recorded_at_ms: 0,
    })
    .map_err(anyhow::Error::msg)?;
    let tag_vectors = tag_vectors(prepared);
    let artifact = build_artifact(ArtifactInput {
        dimension: prepared.identity.dimension,
        model_sig: prepared.identity.space.clone(),
        generation: prepared.identity.generation.clone(),
        geometry_sig: geometry.signature.clone(),
        rows,
        tag_vectors,
        anchor_gain: geometry.anchor_gain.clone(),
        config: prepared.snapshot.artifact_config.clone(),
    })
    .map_err(anyhow::Error::msg)?;
    let graph = GraphDisk::capture(&artifact.graph);
    let geometry = serde_json::to_value(geometry)?;
    let assets = assemble_assets(prepared, artifact.graph, &geometry, &artifact.metadata)?;
    Ok((assets, geometry, graph, artifact.metadata))
}
fn tag_vectors(prepared: &Prepared) -> TagVectorPool {
    prepared.unit_tags.clone()
}
fn assemble_assets(
    prepared: &Prepared,
    graph: Arc<NativeArtifact>,
    geometry: &Value,
    artifact: &Value,
) -> Result<MemoAssets> {
    let rows = root_rows(prepared);
    let candidates = prepared
        .mappings
        .memories
        .iter()
        .map(|(id, mapping)| {
            (
                *id,
                CandidateData {
                    id: *id,
                    file_id: mapping.file_id,
                    vector: prepared.unit_candidates[id].clone(),
                },
            )
        })
        .collect();
    let files = prepared
        .mappings
        .roots
        .keys()
        .map(|id| {
            (
                *id,
                FileData {
                    diary_name: "authorized-snapshot".into(),
                    tags: rows
                        .iter()
                        .filter(|row| row.file_id == *id)
                        .cloned()
                        .collect(),
                },
            )
        })
        .collect();
    let epa = geometry
        .get("epa_cache_json")
        .and_then(Value::as_str)
        .and_then(|raw| decode_epa_basis(raw.to_owned(), prepared.identity.dimension))
        .map(Arc::new);
    MemoAssets::try_new(MemoAssetInput {
        dimension: prepared.identity.dimension,
        signature: artifact["artifactSig"]
            .as_str()
            .context("SERVING_ARTIFACT_INVALID: signature missing")?
            .into(),
        generation: prepared.identity.generation.clone(),
        graph,
        tag_vectors: tag_vectors(prepared),
        tag_names: prepared
            .mappings
            .tags
            .iter()
            .map(|(id, key)| (*id, prepared.tag_definitions[key].label.clone()))
            .collect(),
        epa,
        candidates,
        files,
    })
    .map_err(anyhow::Error::msg)
}
fn asset_diagnostics(assets: &MemoAssets, mappings: &Mappings) -> Value {
    let snapshot = assets.snapshot();
    json!({"candidate_count":mappings.memories.len(),"tag_count":mappings.tags.len(),"source_root_count":mappings.roots.len(),
        "graph_nodes":snapshot.graph.node_ids.len(),"graph_edges":snapshot.graph.targets.len(),"artifact_signature":assets.signature(),"vector_policy":VECTOR_POLICY,
        "numeric_tags":mappings.tags,"source_roots":mappings.roots,"memory_mapping":mappings.memories,"vector_coverage":assets.diagnostics()})
}
#[derive(Serialize, Deserialize)]
struct ServingManifest {
    format: String,
    version: u32,
    digest: String,
    identity: VectorGenerationIdentity,
    geometry_hash: String,
    graph_hash: String,
    mappings_hash: String,
    artifact_meta: Value,
}
#[derive(Serialize, Deserialize)]
struct GraphDisk {
    nodes: Vec<i64>,
    offsets: Vec<usize>,
    targets: Vec<usize>,
    weights: Vec<f64>,
    inbound: BTreeMap<i64, f64>,
    max_inbound: f64,
    anchor_gain: BTreeMap<i64, f64>,
    wormholes: BTreeSet<(i64, i64)>,
    provenance: Vec<EdgeProvenance>,
}
type EdgeProvenance = ((i64, i64), Vec<(i64, f64)>);
impl GraphDisk {
    fn capture(graph: &NativeArtifact) -> Self {
        let mut provenance = graph
            .provenance
            .iter()
            .map(|(edge, values)| (*edge, values.clone()))
            .collect::<Vec<_>>();
        provenance.sort_by_key(|item| item.0);
        Self {
            nodes: graph.node_ids.clone(),
            offsets: graph.row_offsets.clone(),
            targets: graph.targets.clone(),
            weights: graph.weights.clone(),
            inbound: graph.inbound.iter().map(|(a, b)| (*a, *b)).collect(),
            max_inbound: graph.max_inbound,
            anchor_gain: graph.anchor_gain.iter().map(|(a, b)| (*a, *b)).collect(),
            wormholes: graph.wormhole_edges.iter().copied().collect(),
            provenance,
        }
    }
    fn restore(self) -> Result<Arc<NativeArtifact>> {
        let graph = NativeArtifact {
            node_index: self
                .nodes
                .iter()
                .enumerate()
                .map(|(i, id)| (*id, i))
                .collect(),
            node_ids: self.nodes,
            row_offsets: self.offsets,
            targets: self.targets,
            weights: self.weights,
            inbound: self.inbound.into_iter().collect(),
            max_inbound: self.max_inbound,
            anchor_gain: self.anchor_gain.into_iter().collect(),
            wormhole_edges: self.wormholes.into_iter().collect(),
            provenance: self.provenance.into_iter().collect(),
            ..Default::default()
        }
        .try_prepared()
        .map_err(anyhow::Error::msg)?;
        Ok(Arc::new(graph))
    }
}
fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn write_json(path: &Path, value: &impl Serialize) -> Result<String> {
    let bytes = serde_json::to_vec(value)?;
    let mut file = File::create(path)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    Ok(hash(&bytes))
}
fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    serde_json::from_reader(
        File::open(path)
            .with_context(|| format!("SERVING_GENERATION_INCOMPLETE: {}", path.display()))?,
    )
    .context("SERVING_COMPONENT_INVALID")
}
fn read_verified<T: serde::de::DeserializeOwned>(path: &Path, expected: &str) -> Result<T> {
    let mut bytes = Vec::new();
    File::open(path)
        .with_context(|| format!("SERVING_GENERATION_INCOMPLETE: {}", path.display()))?
        .read_to_end(&mut bytes)?;
    ensure!(
        hash(&bytes) == expected,
        "SERVING_CHECKSUM_MISMATCH: changed component"
    );
    serde_json::from_slice(&bytes).context("SERVING_COMPONENT_INVALID")
}
fn sync_dir(path: &Path) -> Result<()> {
    #[cfg(not(windows))]
    File::open(path)?.sync_all()?;
    #[cfg(windows)]
    let _ = path;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn snapshot() -> ServingSnapshot {
        let identity = VectorGenerationIdentity {
            space: "three-dimensional-fixture".into(),
            producer: "adopted-vectors-v1".into(),
            generation: "caller-view-v1".into(),
            signature: "allowed-current".into(),
            watermark: 10,
            dimension: 3,
        };
        let memory = |id: &str, vector: Vec<f32>, members: Vec<(&str, Vec<f32>)>| {
            let root = format!("material:{id}");
            ServingMemory {
                id: id.into(),
                revision: 1,
                commit_seq: 5,
                vector,
                source_ref: root.clone(),
                tags: members
                    .into_iter()
                    .map(|(key, vector)| ServingTag {
                        key: key.into(),
                        label: key.into(),
                        vector,
                        source_ref: root.clone(),
                    })
                    .collect(),
            }
        };
        let alpha = vec![1.0, 0.0, 0.0];
        let bridge = vec![0.8, 0.6, 0.0];
        let omega = vec![0.0, 1.0, 0.0];
        let mut snapshot = ServingSnapshot::new(
            identity,
            vec![
                memory(
                    "a",
                    alpha.clone(),
                    vec![("alpha", alpha.clone()), ("bridge", bridge.clone())],
                ),
                memory(
                    "b",
                    omega.clone(),
                    vec![("bridge", bridge), ("omega", omega.clone())],
                ),
                memory(
                    "c",
                    vec![0.7, 0.7, 0.0],
                    vec![("alpha", alpha), ("omega", omega)],
                ),
            ],
        );
        snapshot.geometry_config = json!({"local":{"minNeighbors":1,"maxBasis":2,"minGain":0.0},"global":{"kMax":2,"kSem":2,"minGain":0.0,"minSupport":0.0}});
        snapshot
    }
    fn profile() -> ServingProfile {
        let config=VcpConfig{pipeline:serde_json::from_value(json!({"epaGateMode":"absolute","pyramidTopK":3,"spikeRouting":{"maxSafeHops":2,"firingThreshold":0.0,"minimumInjectedCurrent":0.0,"maxTransitionRecords":128,"dynamicWormholes":false}})).unwrap(),
            core_tags:vec!["alpha".into()],include_trace:true,include_fields:true,thinking:true,..Default::default()};
        ServingProfile::Vcp(Box::new(config))
    }
    #[test]
    fn provider_nonunit_vectors_use_one_derived_policy_without_rewriting_raw_identity() -> Result<()>
    {
        let parent = tempfile::tempdir()?;
        let baseline = snapshot();
        let mut provider_shape = baseline.clone();
        for memory in &mut provider_shape.memories {
            for value in &mut memory.vector {
                *value *= 1.002;
            }
            for tag in &mut memory.tags {
                for value in &mut tag.vector {
                    *value *= 1.002;
                }
            }
        }
        let original_raw = provider_shape.memories[0].vector.clone();
        let expected_raw = prepare(&provider_shape)?.vectors;
        assert_eq!(expected_raw.candidates[0].vector, original_raw);
        let baseline = ServingGeneration::create_or_open(parent.path(), &baseline)?;
        let derived = ServingGeneration::create_or_open(parent.path(), &provider_shape)?;
        for vector in derived.assets.tag_vectors().values().chain(
            derived
                .assets
                .candidates()
                .values()
                .map(|candidate| &candidate.vector),
        ) {
            let norm = vector
                .iter()
                .map(|value| f64::from(*value).powi(2))
                .sum::<f64>()
                .sqrt();
            assert!((norm - 1.0).abs() < 1e-7);
        }
        assert_eq!(provider_shape.memories[0].vector, original_raw);
        // The pair still accepts the original provider snapshot on native reopen:
        // its raw-vector hash was never replaced with a derived unit-vector hash.
        drop(VectorGeneration::open(
            &derived.path().join("ann"),
            &expected_raw,
        )?);
        let left = baseline.search(&[1.0, 0.0, 0.0], 3, profile())?;
        let right = derived.search(&[1.002, 0.0, 0.0], 3, profile())?;
        assert_eq!(
            left.ranked
                .iter()
                .map(|hit| &hit.memory_id)
                .collect::<Vec<_>>(),
            right
                .ranked
                .iter()
                .map(|hit| &hit.memory_id)
                .collect::<Vec<_>>()
        );
        for (a, b) in left.ranked.iter().zip(&right.ranked) {
            assert!(
                (a.score - b.score).abs() < 2e-6,
                "scaled score {} vs {}",
                a.score,
                b.score
            );
        }
        let identity = derived.identity().clone();
        drop(derived);
        let reopened = ServingGeneration::create_or_open(parent.path(), &provider_shape)?;
        assert!(reopened.restored());
        assert_eq!(reopened.identity(), &identity);
        assert_eq!(reopened.asset_diagnostics()["vector_policy"], VECTOR_POLICY);
        let replay = reopened.search(&[1.002, 0.0, 0.0], 3, profile())?;
        assert_eq!(
            right.ranked.iter().map(|hit| hit.score).collect::<Vec<_>>(),
            replay
                .ranked
                .iter()
                .map(|hit| hit.score)
                .collect::<Vec<_>>()
        );
        Ok(())
    }
    #[test]
    fn full_serving_group_restores_native_ann_graph_and_single_vcp_observation() -> Result<()> {
        let parent = tempfile::tempdir()?;
        let snapshot = snapshot();
        let generation = ServingGeneration::create_or_open(parent.path(), &snapshot)?;
        assert!(!generation.restored());
        let identity = generation.identity().clone();
        let dense = generation.search(&[1.0, 0.0, 0.0], 3, ServingProfile::Dense)?;
        assert_eq!(dense.ranked[0].memory_id, "a");
        let result = generation.search(&[1.0, 0.0, 0.0], 3, profile())?;
        assert_eq!(result.ranked.len(), 3);
        assert!(
            result
                .ranked
                .iter()
                .all(|hit| hit.trace.is_some() && hit.score.is_finite())
        );
        assert_eq!(result.source_mappings["a"].source_ref, "material:a");
        assert_eq!(
            result.diagnostics["candidate_lanes"]
                .as_object()
                .unwrap()
                .len(),
            4
        );
        assert_eq!(result.diagnostics["thinking"]["status"], "ready");
        assert!(
            result.diagnostics["fields"]["observation"]["transitions"]
                .as_array()
                .unwrap()
                .len()
                > 1
        );
        let order = result
            .ranked
            .iter()
            .map(|hit| hit.memory_id.clone())
            .collect::<Vec<_>>();
        let scores = result
            .ranked
            .iter()
            .map(|hit| hit.score)
            .collect::<Vec<_>>();
        let artifacts = generation.asset_diagnostics().clone();
        drop(generation);
        let reopened = ServingGeneration::create_or_open(parent.path(), &snapshot)?;
        assert!(reopened.restored());
        assert_eq!(reopened.identity(), &identity);
        assert_eq!(reopened.asset_diagnostics(), &artifacts);
        let replay = reopened.search(&[1.0, 0.0, 0.0], 3, profile())?;
        assert_eq!(
            replay
                .ranked
                .iter()
                .map(|hit| hit.memory_id.clone())
                .collect::<Vec<_>>(),
            order
        );
        assert_eq!(
            replay
                .ranked
                .iter()
                .map(|hit| hit.score)
                .collect::<Vec<_>>(),
            scores
        );
        Ok(())
    }
    #[test]
    fn authority_filtered_snapshot_alone_defines_membership_and_source_root_evidence() -> Result<()>
    {
        let mut all = snapshot();
        let mut hidden = all.memories[0].clone();
        hidden.id = "hidden".into();
        hidden.source_ref = "material:hidden".into();
        hidden.tags = vec![ServingTag {
            key: "secret".into(),
            label: "secret".into(),
            vector: vec![0.0, 0.0, 1.0],
            source_ref: hidden.source_ref.clone(),
        }];
        let mut future = hidden.clone();
        future.id = "future".into();
        future.commit_seq = 20;
        let mut deleted = hidden.clone();
        deleted.id = "deleted".into();
        let rows = [
            (all.memories[0].clone(), true, false),
            (hidden, false, false),
            (future, true, false),
            (deleted, true, true),
        ];
        // Query authority filters first; serving receives no permission DB or future rows.
        all.memories = rows
            .into_iter()
            .filter(|(m, allowed, deleted)| {
                *allowed && !deleted && m.commit_seq <= all.identity.watermark
            })
            .map(|row| row.0)
            .collect();
        let parent = tempfile::tempdir()?;
        let generation = ServingGeneration::create_or_open(parent.path(), &all)?;
        assert_eq!(generation.mappings.memories.len(), 1);
        assert_eq!(
            generation.mappings.roots.values().collect::<Vec<_>>(),
            vec!["material:a"]
        );
        assert_eq!(
            generation.mappings.tags.values().collect::<Vec<_>>(),
            vec!["alpha", "bridge"]
        );
        assert!(
            generation
                .search(&[0.0, 0.0, 1.0], 10, ServingProfile::Dense)?
                .ranked
                .iter()
                .all(|hit| hit.memory_id == "a")
        );
        let graph = generation.assets.snapshot().graph;
        assert!(
            graph
                .provenance
                .values()
                .flatten()
                .all(|(root, _)| generation.mappings.roots[root] == "material:a")
        );
        let mut derived = all.memories[0].clone();
        derived.id = "a-derived".into();
        all.memories.push(derived);
        let doubled = ServingGeneration::create_or_open(parent.path(), &all)?;
        assert_eq!(doubled.mappings.memories.len(), 2);
        assert_eq!(doubled.mappings.roots.len(), 1);
        assert_eq!(graph.weights, doubled.assets.snapshot().graph.weights);
        assert_eq!(graph.provenance, doubled.assets.snapshot().graph.provenance);
        Ok(())
    }
    #[test]
    fn no_tags_still_executes_vcp_and_corrupt_or_changed_generations_are_explicit() -> Result<()> {
        let parent = tempfile::tempdir()?;
        let mut snapshot = snapshot();
        snapshot.memories.truncate(1);
        snapshot.memories[0].tags.clear();
        let generation = ServingGeneration::create_or_open(parent.path(), &snapshot)?;
        let result = generation.search(&[1.0, 0.0, 0.0], 1, ServingProfile::Vcp(Box::default()))?;
        assert_eq!(result.ranked[0].memory_id, "a");
        assert_eq!(result.diagnostics["profile"], "vcp");
        assert_eq!(result.diagnostics["assets"]["tag_count"], 0);
        assert_eq!(result.diagnostics["candidate_lanes"]["original"]["hits"], 1);
        let path = generation.path().to_owned();
        drop(generation);
        let reopened = ServingGeneration::create_or_open(parent.path(), &snapshot)?;
        assert!(reopened.restored());
        drop(reopened);
        fs::write(path.join("graph.json"), b"partial")?;
        assert!(
            ServingGeneration::create_or_open(parent.path(), &snapshot)
                .err()
                .unwrap()
                .to_string()
                .contains("CHECKSUM_MISMATCH")
        );
        snapshot.memories[0].revision += 1;
        let changed = ServingGeneration::create_or_open(parent.path(), &snapshot)?;
        assert_ne!(changed.path(), path);
        assert!(!changed.restored());
        Ok(())
    }
}
