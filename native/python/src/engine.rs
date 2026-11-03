//! Immutable, reopenable Wave assets and the shared bounded native worker pool.
use crate::{bundles::*, *};
use anyhow::{Context, Result, ensure};
use nous_retrieval::{
    IndexDocument, RetrievalIndex,
    vector::{VectorGeneration, VectorGenerationIdentity, VectorRecord, VectorSnapshot},
};
use nous_vcp::{
    artifact::{ArtifactInput, FileTag, build_artifact},
    assets::{CandidateData, FileData, MemoAssetInput, MemoAssets, RootFact},
    geometry::{GeometryInput, GeometryTag, build_geometry},
    graph::NativeArtifact,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    path::{Path, PathBuf},
    sync::{
        Arc, RwLock,
        atomic::{AtomicBool, Ordering},
    },
};

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct Input {
    pub candidates: Candidates,
    pub concepts: Concepts,
    pub facts: SourceFacts,
    pub associations: Associations,
    pub identity: Value,
    pub options: Value,
}
#[derive(Clone, Deserialize, Serialize)]
pub(crate) struct CandidateMeta {
    pub keys: Vec<String>,
    pub documents: Vec<String>,
}
#[derive(Clone, Deserialize, Serialize)]
pub(crate) struct ConceptMeta {
    pub keys: Vec<String>,
    pub labels: Vec<String>,
    #[serde(default)]
    pub aliases: Vec<Vec<String>>,
}
#[derive(Clone, Deserialize, Serialize)]
pub(crate) struct FactMeta {
    pub root_keys: Vec<String>,
    pub root_checksums: Vec<String>,
    pub sequence_keys: Vec<String>,
}
#[derive(Clone, Deserialize, Serialize)]
pub(crate) struct AssociationMeta {
    pub keys: Vec<String>,
    pub relations: Vec<String>,
}
#[derive(Serialize, Deserialize)]
struct MathDisk {
    graph: NativeArtifact,
    epa_json: String,
    signature: String,
    geometry: Value,
}
pub(crate) struct Frozen {
    pub input: Input,
    pub candidates: CandidateMeta,
    pub concepts: ConceptMeta,
    pub facts: FactMeta,
    pub associations: AssociationMeta,
    pub lexical: RetrievalIndex,
    pub ann: Option<VectorGeneration>,
    pub assets: Option<MemoAssets>,
    pub stats: Value,
}
pub(crate) struct State {
    pub pool: rayon::ThreadPool,
    pub closed: AtomicBool,
    pub active: RwLock<()>,
}
#[pyclass(frozen, module = "nous_native._core")]
pub struct NativeEngine {
    pub(crate) state: Arc<State>,
}
#[pyclass(frozen, skip_from_py_object, module = "nous_native._core")]
#[derive(Clone)]
pub struct NativeSnapshot {
    pub(crate) inner: Arc<Frozen>,
    pub(crate) state: Arc<State>,
}

fn unique(values: &[String]) -> bool {
    values.iter().all(|s| !s.trim().is_empty())
        && values.iter().collect::<BTreeSet<_>>().len() == values.len()
}
impl Input {
    fn validate(&self) -> Result<(CandidateMeta, ConceptMeta, FactMeta, AssociationMeta)> {
        let native_options = self.options.get("native").unwrap_or(&self.options);
        let build_config: nous_vcp::config::BuildConfig =
            serde_json::from_value(native_options.clone())?;
        build_config
            .validate()
            .map_err(|e| failure("wave.invalid_input", e))?;
        let c: CandidateMeta = serde_json::from_str(&self.candidates.metadata_json)?;
        let t: ConceptMeta = serde_json::from_str(&self.concepts.metadata_json)?;
        let f: FactMeta = serde_json::from_str(&self.facts.metadata_json)?;
        let a: AssociationMeta = serde_json::from_str(&self.associations.metadata_json)?;
        for name in ["snapshot_id", "space_signature", "build_config_signature"] {
            ensure!(
                self.identity
                    .get(name)
                    .and_then(Value::as_str)
                    .is_some_and(|s| !s.is_empty()),
                "Missing snapshot identity {name}"
            );
        }
        let n = c.keys.len();
        let m = t.keys.len();
        let roots = f.root_keys.len();
        let seq = f.sequence_keys.len();
        let edges = a.keys.len();
        ensure!(
            unique(&c.keys)
                && unique(&t.keys)
                && unique(&f.root_keys)
                && unique(&f.sequence_keys)
                && unique(&a.keys),
            "Keys must be unique and nonempty"
        );
        ensure!(
            c.documents.len() == n
                && self.candidates.texts.len() == n
                && c.documents.iter().all(|d| !d.is_empty()),
            "Candidate metadata/text length mismatch"
        );
        ensure!(
            t.labels.len() == m
                && (t.aliases.is_empty() || t.aliases.len() == m)
                && f.root_checksums.len() == roots,
            "Concept or source metadata length mismatch"
        );
        csr(
            &self.candidates.membership_indptr,
            &self.candidates.membership_indices,
            n,
            m,
        )?;
        csr(
            &self.candidates.curve_indptr,
            &self.candidates.curve_indices,
            n,
            m,
        )?;
        csr(
            &self.candidates.root_indptr,
            &self.candidates.root_indices,
            n,
            roots,
        )?;
        ensure!(
            self.candidates.root_indptr.windows(2).all(|p| p[0] < p[1]),
            "Each candidate requires evidence roots"
        );
        ensure!(
            self.candidates.curve_positions.len() == self.candidates.curve_indices.len()
                && self.candidates.curve_positions.iter().all(|p| *p > 0),
            "Ordered candidate curve positions must be positive"
        );
        csr(&self.facts.indptr, &self.facts.indices, seq, m)?;
        ensure!(
            self.facts.positions.len() == self.facts.indices.len()
                && self.facts.ordered.len() == seq
                && self.facts.sequence_roots.len() == seq
                && self.facts.sequence_roots.iter().all(|r| *r < roots as u64),
            "Source sequence shape mismatch"
        );
        for row in 0..seq {
            let positions = &self.facts.positions
                [self.facts.indptr[row] as usize..self.facts.indptr[row + 1] as usize];
            if self.facts.ordered[row] {
                ensure!(
                    positions.iter().all(|p| *p > 0) && positions.windows(2).all(|p| p[0] <= p[1]),
                    "Ordered source positions must be positive and monotone"
                );
            }
        }
        if let Some(v) = &self.candidates.vectors {
            ensure!(v.rows == n, "Candidate matrix rows mismatch");
        }
        if let Some(v) = &self.concepts.vectors {
            ensure!(v.rows == m, "Concept matrix rows mismatch");
            if let Some(c) = &self.candidates.vectors {
                ensure!(c.cols == v.cols, "Vector space dimensions mismatch");
            }
        }
        let assoc = &self.associations;
        ensure!(
            a.relations.len() == edges
                && assoc.sources.len() == edges
                && assoc.targets.len() == edges
                && assoc.directed.len() == edges
                && assoc.strengths.len() == edges
                && assoc.confidences.len() == edges
                && assoc.polarities.len() == edges,
            "Association shape mismatch"
        );
        ensure!(
            assoc
                .sources
                .iter()
                .chain(assoc.targets.iter())
                .all(|i| *i < m as u64)
                && assoc
                    .strengths
                    .iter()
                    .chain(assoc.confidences.iter())
                    .all(|v| v.is_finite() && (0.0..=1.0).contains(v))
                && assoc.polarities.iter().all(|v| [-1, 1].contains(v)),
            "Association endpoint or quality invalid"
        );
        csr(&assoc.root_indptr, &assoc.root_indices, edges, roots)?;
        ensure!(
            assoc.root_indptr.windows(2).all(|p| p[0] < p[1]),
            "Associations require evidence roots"
        );
        Ok((c, t, f, a))
    }
}
pub(crate) fn unit(v: &[f32]) -> Result<Vec<f32>> {
    let norm = v.iter().map(|x| f64::from(*x).powi(2)).sum::<f64>().sqrt();
    ensure!(
        norm.is_finite() && norm > 1e-15 && v.iter().all(|v| v.is_finite()),
        "Nonzero finite vector required"
    );
    Ok(v.iter().map(|x| (f64::from(*x) / norm) as f32).collect())
}
pub(crate) fn slice<'a, T>(ptr: &[u64], values: &'a [T], row: usize) -> &'a [T] {
    &values[ptr[row] as usize..ptr[row + 1] as usize]
}
fn build_math(
    input: &Input,
    concepts: &ConceptMeta,
    cancel: &CancellationToken,
) -> Result<Option<MathDisk>> {
    let Some(vectors) = &input.concepts.vectors else {
        return Ok(None);
    };
    if input.candidates.vectors.is_none()
        || (input.facts.indices.is_empty() && input.associations.sources.is_empty())
    {
        return Ok(None);
    }
    let mut rows = Vec::new();
    for seq in 0..input.facts.sequence_roots.len() {
        for cursor in input.facts.indptr[seq] as usize..input.facts.indptr[seq + 1] as usize {
            rows.push(FileTag {
                file_id: seq as i64 + 1,
                tag_id: input.facts.indices[cursor] as i64 + 1,
                position: if input.facts.ordered[seq] {
                    input.facts.positions[cursor]
                } else {
                    0
                },
            });
        }
    }
    let pool = concepts
        .keys
        .iter()
        .enumerate()
        .map(|(row, _)| Ok((row as i64 + 1, Arc::<[f32]>::from(unit(vectors.row(row))?))))
        .collect::<Result<HashMap<_, _>>>()?;
    let assoc = &input.associations;
    let meta: AssociationMeta = serde_json::from_str(&assoc.metadata_json)?;
    let association_options = &input.options["associations"];
    let enabled = association_options
        .get("enabled")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let relation_weights = association_options
        .get("relation_gains")
        .and_then(Value::as_object);
    let mut masses = BTreeMap::<(i64, i64, String, i64), f64>::new();
    if enabled {
        for i in 0..assoc.sources.len() {
            if assoc.polarities[i] < 0 {
                continue;
            }
            let relation = &meta.relations[i];
            let gain = relation_weights
                .and_then(|w| w.get(relation))
                .and_then(Value::as_f64)
                .unwrap_or_else(|| {
                    if relation_weights.is_none()
                        && matches!(relation.as_str(), "related" | "supports" | "sequence")
                    {
                        1.0
                    } else {
                        0.0
                    }
                });
            ensure!(
                gain.is_finite() && gain >= 0.0,
                "Association projection gain invalid"
            );
            let roots = slice(&assoc.root_indptr, &assoc.root_indices, i)
                .iter()
                .copied()
                .collect::<BTreeSet<_>>();
            let weight =
                gain * assoc.strengths[i] * assoc.confidences[i] / roots.len().max(1) as f64;
            for (source, target) in [
                (assoc.sources[i] as i64 + 1, assoc.targets[i] as i64 + 1),
                (assoc.targets[i] as i64 + 1, assoc.sources[i] as i64 + 1),
            ]
            .into_iter()
            .take(if assoc.directed[i] { 1 } else { 2 })
            {
                for root in &roots {
                    let mass = masses
                        .entry((source, target, relation.clone(), *root as i64 + 1))
                        .or_default();
                    *mass = mass.max(weight);
                }
            }
        }
    }
    let additional_facts = masses
        .into_iter()
        .map(
            |((source, target, _, root), mass)| nous_vcp::artifact::FactEdge {
                source,
                target,
                root,
                mass,
            },
        )
        .collect::<Vec<_>>();
    let edges = additional_facts
        .iter()
        .map(|e| (e.source, e.target, e.mass))
        .collect::<Vec<_>>();
    let native_options = input.options.get("native").unwrap_or(&input.options);
    let build_config: nous_vcp::config::BuildConfig =
        serde_json::from_value(native_options.clone())
            .map_err(|e| failure("wave.invalid_input", e))?;
    build_config
        .validate()
        .map_err(|e| failure("wave.invalid_input", e))?;
    cancel.check()?;
    let geometry = build_geometry(
        GeometryInput {
            dimension: vectors.cols,
            model_sig: input.identity["space_signature"].as_str().unwrap().into(),
            config: serde_json::to_value(&build_config.geometry)?,
            tags: concepts
                .keys
                .iter()
                .enumerate()
                .map(|(row, _)| GeometryTag {
                    id: row as i64 + 1,
                    name: concepts.labels[row].clone(),
                    vector_version: 1,
                    vector: pool[&(row as i64 + 1)].to_vec(),
                })
                .collect(),
            memberships: rows.clone(),
            edges: if edges.is_empty() {
                None
            } else {
                Some(edges.clone())
            },
            recorded_at_ms: 0,
        },
        Some(&|| cancel.is_cancelled()),
    )
    .map_err(|e| {
        failure(
            if cancel.is_cancelled() {
                "wave.cancelled"
            } else {
                "wave.geometry_failed"
            },
            e,
        )
    })?;
    cancel.check()?;
    let config = serde_json::to_value(&build_config.artifact)?;
    let built = build_artifact(
        ArtifactInput {
            dimension: vectors.cols,
            model_sig: input.identity["space_signature"].as_str().unwrap().into(),
            generation: input.identity["snapshot_id"].as_str().unwrap().into(),
            geometry_sig: geometry.signature.clone(),
            rows,
            tag_vectors: pool,
            anchor_gain: geometry.anchor_gain.clone(),
            config: config.clone(),
            source_roots: input
                .facts
                .sequence_roots
                .iter()
                .enumerate()
                .map(|(seq, root)| (seq as i64 + 1, *root as i64 + 1))
                .collect(),
            additional_facts,
        },
        Some(&|| cancel.is_cancelled()),
    )
    .map_err(|e| {
        failure(
            if cancel.is_cancelled() {
                "wave.cancelled"
            } else {
                "wave.artifact_failed"
            },
            e,
        )
    })?;
    let graph = (*built.graph).clone();
    if graph.targets.is_empty() {
        return Ok(None);
    }
    let signature = format!(
        "{:x}",
        <sha2::Sha256 as sha2::Digest>::digest(rmp_serde::to_vec_named(&graph)?)
    );
    let epa_json = geometry.epa_cache_json.clone();
    Ok(Some(MathDisk {
        graph,
        epa_json,
        signature,
        geometry: json!({"algorithm_version":geometry.algorithm_version,"signature":geometry.signature,"tag_count":geometry.tag_count,"computed_count":geometry.computed_count}),
    }))
}
fn assemble(
    input: &Input,
    c: &CandidateMeta,
    t: &ConceptMeta,
    f: &FactMeta,
    math: &MathDisk,
) -> Result<MemoAssets> {
    let dimension = input.concepts.vectors.as_ref().unwrap().cols;
    let mut files = BTreeMap::new();
    let mut roots = BTreeMap::<i64, RootFact>::new();
    for (row, checksum) in f.root_checksums.iter().enumerate() {
        roots.insert(
            row as i64 + 1,
            RootFact {
                checksum: checksum.clone(),
                tags: BTreeSet::new(),
            },
        );
    }
    for seq in 0..f.sequence_keys.len() {
        let root = input.facts.sequence_roots[seq] as i64 + 1;
        for tag in slice(&input.facts.indptr, &input.facts.indices, seq) {
            roots.get_mut(&root).unwrap().tags.insert(*tag as i64 + 1);
        }
    }
    for (root, fact) in &roots {
        files.insert(
            *root,
            FileData {
                diary_name: "source".into(),
                tags: fact
                    .tags
                    .iter()
                    .map(|tag| FileTag {
                        file_id: *root,
                        tag_id: *tag,
                        position: 0,
                    })
                    .collect(),
            },
        );
    }
    let docs = c
        .documents
        .iter()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .enumerate()
        .map(|(i, key)| (key, i as i64 + 1))
        .collect::<BTreeMap<_, _>>();
    let mut candidates = BTreeMap::new();
    let candidate_vectors = input
        .candidates
        .vectors
        .as_ref()
        .context("VCP requires candidate vectors")?;
    for row in 0..c.keys.len() {
        let file_id = f.root_keys.len() as i64 + row as i64 + 1;
        let mut tags = Vec::new();
        let mut ordered = BTreeSet::new();
        for cursor in input.candidates.curve_indptr[row] as usize
            ..input.candidates.curve_indptr[row + 1] as usize
        {
            let tag = input.candidates.curve_indices[cursor] as i64 + 1;
            ordered.insert(tag);
            tags.push(FileTag {
                file_id,
                tag_id: tag,
                position: input.candidates.curve_positions[cursor],
            });
        }
        for tag in slice(
            &input.candidates.membership_indptr,
            &input.candidates.membership_indices,
            row,
        ) {
            let tag = *tag as i64 + 1;
            if !ordered.contains(&tag) {
                tags.push(FileTag {
                    file_id,
                    tag_id: tag,
                    position: 0,
                });
            }
        }
        files.insert(
            file_id,
            FileData {
                diary_name: c.documents[row].clone(),
                tags,
            },
        );
        candidates.insert(
            row as i64 + 1,
            CandidateData {
                id: row as i64 + 1,
                file_id,
                document_id: docs[&c.documents[row]],
                source_ids: slice(
                    &input.candidates.root_indptr,
                    &input.candidates.root_indices,
                    row,
                )
                .iter()
                .map(|r| *r as i64 + 1)
                .collect(),
                vector: Arc::from(unit(candidate_vectors.row(row))?),
            },
        );
    }
    MemoAssets::try_new(MemoAssetInput {
        tag_aliases: t
            .aliases
            .iter()
            .enumerate()
            .map(|(i, aliases)| (i as i64 + 1, aliases.clone()))
            .collect(),
        root_facts: roots,
        dimension,
        signature: math.signature.clone(),
        generation: input.identity["snapshot_id"].as_str().unwrap().into(),
        graph: Arc::new(math.graph.clone()),
        tag_vectors: t
            .keys
            .iter()
            .enumerate()
            .map(|(row, _)| {
                Ok((
                    row as i64 + 1,
                    Arc::from(unit(input.concepts.vectors.as_ref().unwrap().row(row))?),
                ))
            })
            .collect::<Result<_>>()?,
        tag_names: t
            .labels
            .iter()
            .enumerate()
            .map(|(i, name)| (i as i64 + 1, name.clone()))
            .collect(),
        epa: nous_vcp::pipeline::decode_epa_basis(math.epa_json.clone(), dimension).map(Arc::new),
        candidates,
        files,
    })
    .map_err(|e| failure("wave.assets_invalid", e))
}
fn load(path: &Path, expected: &Value) -> Result<Frozen> {
    ensure!(
        path.join("lexical/meta.json").is_file(),
        "Snapshot lexical index is missing"
    );
    let manifest: BTreeMap<String, String> =
        serde_json::from_slice(&std::fs::read(path.join("manifest.json"))?)?;
    for name in ["assets.msgpack", "math.msgpack", "timings.json"] {
        ensure!(
            manifest.get(name) == Some(&file_hash(&path.join(name))?),
            "Snapshot component checksum mismatch: {name}"
        );
    }
    let raw = std::fs::read(path.join("assets.msgpack"))?;
    let input: Input = rmp_serde::from_slice(&raw)?;
    if let Some(expected) = expected.as_object() {
        for (key, value) in expected {
            ensure!(
                input.identity.get(key) == Some(value),
                "Snapshot identity mismatch for {key}"
            );
        }
    }
    let (c, t, f, a) = input.validate()?;
    let math: Option<MathDisk> = rmp_serde::from_slice(&std::fs::read(path.join("math.msgpack"))?)?;
    let assets = if let Some(math) = &math {
        Some(assemble(&input, &c, &t, &f, math)?)
    } else {
        None
    };
    let ann = if input.candidates.vectors.is_some() {
        Some(VectorGeneration::open(
            &path.join("vectors"),
            &vector_snapshot(&input, &c, &t)?,
        )?)
    } else {
        None
    };
    let lexical = RetrievalIndex::open(path.join("lexical"))?;
    lexical.close()?;
    let timings: Value = serde_json::from_slice(&std::fs::read(path.join("timings.json"))?)?;
    let geometry=math.as_ref().map(|m|json!({"algorithm_version":m.geometry["algorithm_version"],"signature":m.geometry["signature"],"tag_count":m.geometry["tag_count"],"computed_count":m.geometry["computed_count"]}));
    let dimension = input.candidates.vectors.as_ref().map(|v| v.cols);
    let stats = json!({"dimension":dimension,"timings":timings,"candidates":c.keys.len(),"concepts":t.keys.len(),"evidence_roots":f.root_keys.len(),"source_sequences":f.sequence_keys.len(),"associations":a.keys.len(),"graph_nodes":math.as_ref().map_or(0,|m|m.graph.node_ids.len()),"graph_edges":math.as_ref().map_or(0,|m|m.graph.targets.len()),"geometry":geometry});
    Ok(Frozen {
        input,
        candidates: c,
        concepts: t,
        facts: f,
        associations: a,
        lexical,
        ann,
        assets,
        stats,
    })
}
#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Settings {
    threads: usize,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            threads: std::thread::available_parallelism().map_or(1, |count| count.get().min(4)),
        }
    }
}
#[pymethods]
impl NativeEngine {
    #[new]
    #[pyo3(signature=(settings_json="{}"))]
    fn new(settings_json: &str) -> PyResult<Self> {
        let settings: Settings = serde_json::from_str(settings_json)
            .map_err(|e| native_error(anyhow::Error::from(e)))?;
        if settings.threads == 0 {
            return Err(native_error(failure(
                "wave.invalid_input",
                "threads must be positive",
            )));
        }
        Ok(Self {
            state: Arc::new(State {
                pool: rayon::ThreadPoolBuilder::new()
                    .num_threads(settings.threads)
                    .build()
                    .map_err(|e| native_error(anyhow::Error::from(e)))?,
                closed: AtomicBool::new(false),
                active: RwLock::new(()),
            }),
        })
    }
    #[allow(clippy::too_many_arguments)]
    #[pyo3(signature=(candidates,concepts,facts,associations,identity_json,output_dir,*,options_json="{}",cancel=None))]
    fn build(
        &self,
        py: Python<'_>,
        candidates: PyRef<'_, Candidates>,
        concepts: PyRef<'_, Concepts>,
        facts: PyRef<'_, SourceFacts>,
        associations: PyRef<'_, Associations>,
        identity_json: &str,
        output_dir: String,
        options_json: &str,
        cancel: Option<PyRef<'_, CancellationToken>>,
    ) -> PyResult<NativeSnapshot> {
        let input = Input {
            candidates: candidates.clone(),
            concepts: concepts.clone(),
            facts: facts.clone(),
            associations: associations.clone(),
            identity: serde_json::from_str(identity_json)
                .map_err(|e| native_error(anyhow::Error::from(e)))?,
            options: serde_json::from_str(options_json)
                .map_err(|e| native_error(anyhow::Error::from(e)))?,
        };
        let token = cancel.map(|v| v.clone()).unwrap_or_default();
        let state = self.state.clone();
        let frozen = py
            .detach(|| {
                state.pool.install(|| -> Result<Frozen> {
                    let _active = state
                        .active
                        .read()
                        .map_err(|_| failure("wave.closed", "Engine lock poisoned"))?;
                    ensure!(
                        !state.closed.load(Ordering::Acquire),
                        "Native engine closed"
                    );
                    token.check()?;
                    let (c, t, _, _) = input
                        .validate()
                        .map_err(|e| failure("wave.invalid_input", e))?;
                    let path = PathBuf::from(output_dir);
                    std::fs::create_dir_all(&path)?;
                    ensure!(
                        !path.join("assets.msgpack").exists(),
                        "Snapshot directory already built"
                    );
                    let started=std::time::Instant::now();let stage=std::time::Instant::now();
                    let math = build_math(&input, &t, &token)?;
                    let math_ms=stage.elapsed().as_secs_f64()*1000.0;let stage=std::time::Instant::now();
                    let lexical = RetrievalIndex::open(path.join("lexical"))?;
                    lexical.rebuild(
                        &c.keys
                            .iter()
                            .enumerate()
                            .map(|(row, key)| IndexDocument {
                                id: key.clone(),
                                namespace_id: "snapshot".into(),
                                subject_id: "snapshot".into(),
                                commit_seq: 1,
                                text: input.candidates.texts[row].clone(),
                                deleted: false,
                            })
                            .collect::<Vec<_>>(),
                        1,
                    )?;
                    lexical.close()?;
                    let lexical_ms=stage.elapsed().as_secs_f64()*1000.0;let stage=std::time::Instant::now();
                    token.check()?;
                    if input.candidates.vectors.is_some() {
                        VectorGeneration::create(
                            &path.join("vectors"),
                            vector_snapshot(&input, &c, &t)?,
                            Some(&||token.is_cancelled()),
                        ).map_err(|e|if token.is_cancelled(){failure("wave.cancelled",e)}else{e})?;
                    }
                    token.check()?;
                    let ann_ms=stage.elapsed().as_secs_f64()*1000.0;
                    std::fs::write(path.join("math.msgpack"), rmp_serde::to_vec_named(&math)?)?;
                    std::fs::write(
                        path.join("assets.msgpack"),
                        rmp_serde::to_vec_named(&input)?,
                    )?;
                    std::fs::write(path.join("timings.json"),serde_json::to_vec(&json!({"total_ms":started.elapsed().as_secs_f64()*1000.0,"geometry_artifact_ms":math_ms,"lexical_ms":lexical_ms,"ann_ms":ann_ms}))?)?;
                    let manifest = ["assets.msgpack", "math.msgpack","timings.json"]
                        .into_iter()
                        .map(|name| Ok((name.to_string(), file_hash(&path.join(name))?)))
                        .collect::<Result<BTreeMap<_, _>>>()?;
                    std::fs::write(path.join("manifest.json"), serde_json::to_vec(&manifest)?)?;
                    load(&path, &input.identity)
                })
            })
            .map_err(native_error)?;
        Ok(NativeSnapshot {
            inner: Arc::new(frozen),
            state,
        })
    }
    #[pyo3(signature=(path,expected_identity_json="{}"))]
    fn open(
        &self,
        py: Python<'_>,
        path: String,
        expected_identity_json: &str,
    ) -> PyResult<NativeSnapshot> {
        let expected: Value = serde_json::from_str(expected_identity_json)
            .map_err(|e| native_error(anyhow::Error::from(e)))?;
        let state = self.state.clone();
        let frozen = py
            .detach(|| {
                state.pool.install(|| {
                    let _active = state
                        .active
                        .read()
                        .map_err(|_| failure("wave.closed", "Engine lock poisoned"))?;
                    ensure!(
                        !state.closed.load(Ordering::Acquire),
                        "Native engine closed"
                    );
                    load(Path::new(&path), &expected)
                })
            })
            .map_err(|e| native_error(failure("wave.snapshot_invalid", e)))?;
        Ok(NativeSnapshot {
            inner: Arc::new(frozen),
            state,
        })
    }
    fn close(&self, py: Python<'_>) -> PyResult<()> {
        self.state.closed.store(true, Ordering::Release);
        py.detach(|| {
            let _guard = self
                .state
                .active
                .write()
                .map_err(|_| failure("wave.closed", "Engine lock poisoned"))?;
            Ok::<_, anyhow::Error>(())
        })
        .map_err(native_error)
    }
}

fn file_hash(path: &Path) -> Result<String> {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 65536];
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn vector_snapshot(input: &Input, c: &CandidateMeta, t: &ConceptMeta) -> Result<VectorSnapshot> {
    let v = input
        .candidates
        .vectors
        .as_ref()
        .context("Candidate vectors unavailable")?;
    Ok(VectorSnapshot {
        identity: VectorGenerationIdentity {
            space: input.identity["space_signature"].as_str().unwrap().into(),
            generation: input.identity["snapshot_id"].as_str().unwrap().into(),
            signature: input.identity["build_config_signature"]
                .as_str()
                .unwrap()
                .into(),
            watermark: 0,
            dimension: v.cols,
        },
        candidates: c
            .keys
            .iter()
            .enumerate()
            .map(|(row, key)| VectorRecord {
                id: row as i64 + 1,
                source_key: key.clone(),
                vector: v.row(row).to_vec(),
            })
            .collect(),
        tags: input
            .concepts
            .vectors
            .as_ref()
            .map(|v| {
                t.keys
                    .iter()
                    .enumerate()
                    .map(|(row, key)| VectorRecord {
                        id: row as i64 + 1,
                        source_key: key.clone(),
                        vector: v.row(row).to_vec(),
                    })
                    .collect()
            })
            .unwrap_or_default(),
    })
}
