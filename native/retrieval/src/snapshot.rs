//! Immutable, reopenable Wave assets and the shared bounded native worker pool.
use crate::{CancellationToken, bundles::*, failure};
use crate::{
    IndexDocument, RetrievalIndex,
    vector::{VectorGeneration, VectorGenerationIdentity, VectorRecord, VectorSnapshot},
};
use anyhow::{Context, Result, ensure};
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
    path::Path,
    sync::Arc,
};

#[derive(Clone, Serialize, Deserialize)]
pub struct Input {
    pub candidates: Candidates,
    pub concepts: Concepts,
    pub facts: SourceFacts,
    pub associations: Associations,
    pub identity: Value,
    pub options: Value,
}
#[derive(Clone, Deserialize, Serialize)]
pub struct CandidateMeta {
    pub keys: Vec<String>,
    pub documents: Vec<String>,
}
#[derive(Clone, Deserialize, Serialize)]
pub struct ConceptMeta {
    pub keys: Vec<String>,
    pub labels: Vec<String>,
    #[serde(default)]
    pub aliases: Vec<Vec<String>>,
}
#[derive(Clone, Deserialize, Serialize)]
pub struct FactMeta {
    pub root_keys: Vec<String>,
    pub root_checksums: Vec<String>,
    pub sequence_keys: Vec<String>,
}
#[derive(Clone, Deserialize, Serialize)]
pub struct AssociationMeta {
    pub support_groups: Vec<GroupMeta>,
    pub keys: Vec<String>,
    pub relations: Vec<String>,
}
#[derive(Clone, Deserialize, Serialize)]
pub struct GroupMeta {
    pub key: String,
    pub kind: String,
}
#[derive(Serialize, Deserialize)]
struct MathDisk {
    graph: NativeArtifact,
    epa_json: String,
    signature: String,
    geometry: Value,
}
pub struct Frozen {
    pub input: Input,
    pub candidates: CandidateMeta,
    pub concepts: ConceptMeta,
    pub facts: FactMeta,
    pub associations: AssociationMeta,
    pub lexical: RetrievalIndex,
    pub ann: Option<VectorGeneration>,
    pub assets: Option<MemoAssets>,
    pub stats: Value,
    pub concept_candidates: BTreeMap<u64, Vec<u64>>,
    pub negative_neighbors: BTreeMap<u64, BTreeSet<u64>>,
    pub negative_relations: BTreeMap<(u64, u64), Vec<usize>>,
    pub candidate_units: Option<Vec<Arc<[f32]>>>,
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
        let paths = self
            .candidates
            .path_step_indptr
            .len()
            .checked_sub(1)
            .context("Missing path offsets")?;
        csr(
            &self.candidates.path_indptr,
            &(0..paths as u64).collect::<Vec<_>>(),
            n,
            paths,
        )?;
        csr(
            &self.candidates.path_step_indptr,
            &self.candidates.path_indices,
            paths,
            m,
        )?;
        ensure!(
            self.candidates
                .path_indptr
                .windows(2)
                .all(|p| p[1] - p[0] <= 16),
            "At most 16 paths per candidate"
        );
        csr(
            &self.candidates.root_indptr,
            &self.candidates.root_indices,
            n,
            roots,
        )?;
        ensure!(
            self.candidates.path_positions.len() == self.candidates.path_indices.len()
                && self.candidates.path_positions.iter().all(|p| *p > 0),
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
        if let Some(limit) = build_config.pair_work_limit {
            let mut total = 0u128;
            for (i, key) in f.sequence_keys.iter().enumerate() {
                let n = (self.facts.indptr[i + 1] - self.facts.indptr[i]) as u128;
                let work = n.saturating_mul(n.saturating_sub(1)) / 2;
                total = total.saturating_add(work);
                if total > u128::from(limit) {
                    return Err(crate::Failure{code:"wave.pair_work_limit",message:format!("Occurrence pair work exceeds {limit} at source sequence {key}"),details:json!({"sequence":key,"sequence_pairs":work.to_string(),"total_pairs":total.to_string(),"limit":limit})}.into());
                }
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
        for matrix in [&self.candidates.vectors, &self.concepts.vectors]
            .into_iter()
            .flatten()
        {
            ensure!(
                matrix.cols > 0
                    && matrix.rows.checked_mul(matrix.cols) == Some(matrix.values.len())
                    && matrix.values.iter().all(|v| v.is_finite()),
                "Invalid finite matrix layout"
            );
        }
        for row in 0..n {
            let members = slice(
                &self.candidates.membership_indptr,
                &self.candidates.membership_indices,
                row,
            );
            for path in self.candidates.path_indptr[row] as usize
                ..self.candidates.path_indptr[row + 1] as usize
            {
                let indices = slice(
                    &self.candidates.path_step_indptr,
                    &self.candidates.path_indices,
                    path,
                );
                let positions = slice(
                    &self.candidates.path_step_indptr,
                    &self.candidates.path_positions,
                    path,
                );
                ensure!(
                    indices.iter().all(|i| members.contains(i))
                        && positions.windows(2).all(|p| p[0] <= p[1]),
                    "Path occurrences must have memberships and monotone positions"
                );
            }
        }
        let assoc = &self.associations;
        ensure!(
            a.relations.len() == edges
                && assoc.edge_groups.len() == edges
                && assoc
                    .edge_groups
                    .iter()
                    .all(|i| *i < a.support_groups.len() as u64)
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
        csr(
            &assoc.group_root_indptr,
            &assoc.group_root_indices,
            a.support_groups.len(),
            roots,
        )?;
        ensure!(
            unique(
                &a.support_groups
                    .iter()
                    .map(|g| g.key.clone())
                    .collect::<Vec<_>>()
            ) && a.support_groups.iter().enumerate().all(|(i, g)| matches!(
                g.kind.as_str(),
                "source" | "interpretation" | "hypothesis" | "imagination"
            ) && (!matches!(
                g.kind.as_str(),
                "source" | "interpretation"
            ) || assoc
                .group_root_indptr[i]
                < assoc.group_root_indptr[i + 1])),
            "Invalid support group premises, keys or kind"
        );
        Ok((c, t, f, a))
    }
}
pub fn unit(v: &[f32]) -> Result<Vec<f32>> {
    let norm = v.iter().map(|x| f64::from(*x).powi(2)).sum::<f64>().sqrt();
    if !norm.is_finite() || norm <= 1e-15 || v.iter().any(|x| !x.is_finite()) {
        return Err(failure(
            "wave.numeric_invalid",
            "A nonzero finite vector is required",
        ));
    }
    Ok(v.iter().map(|x| (f64::from(*x) / norm) as f32).collect())
}
pub fn slice<'a, T>(ptr: &[u64], values: &'a [T], row: usize) -> &'a [T] {
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
    if input.candidates.vectors.is_none() {
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
    let facts: FactMeta = serde_json::from_str(&input.facts.metadata_json)?;
    let mut canonical_groups = BTreeMap::<(String, Vec<u64>), usize>::new();
    let mut canonical_ids = Vec::new();
    for (i, g) in meta.support_groups.iter().enumerate() {
        let mut roots = slice(&assoc.group_root_indptr, &assoc.group_root_indices, i).to_vec();
        roots.sort_unstable();
        roots.dedup();
        canonical_ids.push(*canonical_groups.entry((g.kind.clone(), roots)).or_insert(i));
    }
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
            let group = &meta.support_groups[assoc.edge_groups[i] as usize];
            let interpretation_gain = association_options
                .get("interpretation_gain")
                .and_then(Value::as_f64)
                .unwrap_or(0.0);
            ensure!(
                interpretation_gain.is_finite() && interpretation_gain >= 0.0,
                "Invalid interpretation_gain"
            );
            let kind_gain = match group.kind.as_str() {
                "source" => 1.0,
                "interpretation" => interpretation_gain,
                _ => 0.0,
            };
            let weight = gain * assoc.strengths[i] * assoc.confidences[i] * kind_gain;
            let canonical = canonical_ids[assoc.edge_groups[i] as usize];
            let group_id = facts.root_keys.len() as i64 + 1 + canonical as i64;
            for (source, target) in [
                (assoc.sources[i] as i64 + 1, assoc.targets[i] as i64 + 1),
                (assoc.targets[i] as i64 + 1, assoc.sources[i] as i64 + 1),
            ]
            .into_iter()
            .take(if assoc.directed[i] { 1 } else { 2 })
            {
                let mass = masses
                    .entry((source, target, relation.clone(), group_id))
                    .or_default();
                *mass = mass.max(weight);
            }
        }
    }
    let additional_facts = masses
        .into_iter()
        .filter(|(_, mass)| *mass > 0.0)
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
    let mut graph = (*built.graph).clone();
    let facts: FactMeta = serde_json::from_str(&input.facts.metadata_json)?;
    for (i, g) in meta.support_groups.iter().enumerate() {
        if canonical_ids[i] != i {
            continue;
        }
        graph.support_groups.insert(
            facts.root_keys.len() as i64 + 1 + i as i64,
            nous_vcp::graph::SupportGroup {
                required_roots: slice(&assoc.group_root_indptr, &assoc.group_root_indices, i)
                    .iter()
                    .map(|r| *r as i64 + 1)
                    .collect(),
                kind: g.kind.clone(),
            },
        );
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
                required_roots: vec![row as i64 + 1],
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
    // Explicit source groups retain their complete premises for strong-route witnesses.
    // Interpretation groups never create an original-source witness.
    let mut root_groups = roots
        .iter()
        .map(|(id, fact)| (fact.required_roots.clone(), *id))
        .collect::<BTreeMap<_, _>>();
    let assoc = &input.associations;
    let metadata: AssociationMeta = serde_json::from_str(&assoc.metadata_json)?;
    for i in 0..assoc.sources.len() {
        let group_index = assoc.edge_groups[i] as usize;
        let group = &metadata.support_groups[group_index];
        if assoc.polarities[i] < 0 || group.kind != "source" {
            continue;
        }
        let mut required = slice(
            &assoc.group_root_indptr,
            &assoc.group_root_indices,
            group_index,
        )
        .iter()
        .map(|r| *r as i64 + 1)
        .collect::<Vec<_>>();
        required.sort_unstable();
        required.dedup();
        if required.is_empty() {
            continue;
        }
        let id = *root_groups
            .entry(required.clone())
            .or_insert(f.root_keys.len() as i64 + 1 + group_index as i64);
        let checksum = required
            .iter()
            .map(|r| f.root_checksums[(*r - 1) as usize].as_str())
            .collect::<Vec<_>>()
            .join("+");
        let fact = roots.entry(id).or_insert_with(|| RootFact {
            required_roots: required,
            checksum,
            tags: BTreeSet::new(),
        });
        fact.tags.insert(assoc.sources[i] as i64 + 1);
        fact.tags.insert(assoc.targets[i] as i64 + 1);
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
    let mut candidate_paths = BTreeMap::new();
    let candidate_vectors = input
        .candidates
        .vectors
        .as_ref()
        .context("VCP requires candidate vectors")?;
    for row in 0..c.keys.len() {
        let file_id = f.root_keys.len() as i64 + row as i64 + 1;
        let mut tags = Vec::new();
        let mut ordered = BTreeSet::new();
        let mut paths = Vec::new();
        for path in input.candidates.path_indptr[row] as usize
            ..input.candidates.path_indptr[row + 1] as usize
        {
            let mut steps = Vec::new();
            for cursor in input.candidates.path_step_indptr[path] as usize
                ..input.candidates.path_step_indptr[path + 1] as usize
            {
                steps.push(FileTag {
                    file_id,
                    tag_id: input.candidates.path_indices[cursor] as i64 + 1,
                    position: input.candidates.path_positions[cursor],
                });
            }
            paths.push(steps);
        }
        if let Some(first) = paths.first() {
            for tag in first {
                ordered.insert(tag.tag_id);
                tags.push(tag.clone());
            }
        }
        candidate_paths.insert(row as i64 + 1, paths);
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
        candidate_paths,
        files,
    })
    .map_err(|e| failure("wave.assets_invalid", e))
}
pub fn load(path: &Path, expected: &Value, cancel: &CancellationToken) -> Result<Frozen> {
    cancel.check()?;
    ensure!(
        path.join("lexical/meta.json").is_file(),
        "Snapshot lexical index is missing"
    );
    let manifest: BTreeMap<String, String> =
        serde_json::from_slice(&std::fs::read(path.join("manifest.json"))?)?;
    for name in ["assets.msgpack", "math.msgpack", "timings.json"] {
        ensure!(
            manifest.get(name) == Some(&file_hash(&path.join(name), cancel)?),
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
    let mut concept_candidates = BTreeMap::<u64, Vec<u64>>::new();
    for row in 0..c.keys.len() {
        for concept in slice(
            &input.candidates.membership_indptr,
            &input.candidates.membership_indices,
            row,
        ) {
            concept_candidates
                .entry(*concept)
                .or_default()
                .push(row as u64);
        }
    }
    let mut negative_neighbors = BTreeMap::<u64, BTreeSet<u64>>::new();
    let mut negative_relations = BTreeMap::<(u64, u64), Vec<usize>>::new();
    for i in 0..input.associations.sources.len() {
        let assoc = &input.associations;
        if assoc.polarities[i] < 0 {
            negative_relations
                .entry((assoc.sources[i], assoc.targets[i]))
                .or_default()
                .push(i);
            negative_neighbors
                .entry(assoc.sources[i])
                .or_default()
                .insert(assoc.targets[i]);
            if !assoc.directed[i] {
                negative_relations
                    .entry((assoc.targets[i], assoc.sources[i]))
                    .or_default()
                    .push(i);
                negative_neighbors
                    .entry(assoc.targets[i])
                    .or_default()
                    .insert(assoc.sources[i]);
            }
        }
    }
    let candidate_units = if let Some(assets) = &assets {
        Some(
            (1..=c.keys.len() as i64)
                .map(|id| assets.candidates()[&id].vector.clone())
                .collect(),
        )
    } else {
        input
            .candidates
            .vectors
            .as_ref()
            .map(|v| {
                (0..v.rows)
                    .map(|row| unit(v.row(row)).map(Arc::<[f32]>::from))
                    .collect::<Result<Vec<_>>>()
            })
            .transpose()?
    };
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
        concept_candidates,
        negative_neighbors,
        negative_relations,
        candidate_units,
    })
}

#[derive(Clone, Serialize)]
pub struct BuiltSnapshot {
    pub identity: Value,
    pub manifest: BTreeMap<String, String>,
}
pub fn build(input: Input, output_dir: &Path, token: &CancellationToken) -> Result<BuiltSnapshot> {
    let (c, t, _, _) = input.validate().map_err(|e| {
        if e.is::<crate::Failure>() {
            e
        } else {
            failure("wave.invalid_input", e)
        }
    })?;
    let path = output_dir.to_path_buf();
    std::fs::create_dir_all(&path)?;
    ensure!(
        !path.join("assets.msgpack").exists(),
        "Snapshot directory already built"
    );
    let started = std::time::Instant::now();
    let stage = std::time::Instant::now();
    let math = build_math(&input, &t, token)?;
    let math_ms = stage.elapsed().as_secs_f64() * 1000.0;
    let stage = std::time::Instant::now();
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
    let lexical_ms = stage.elapsed().as_secs_f64() * 1000.0;
    let stage = std::time::Instant::now();
    token.check()?;
    if input.candidates.vectors.is_some() {
        VectorGeneration::create(
            &path.join("vectors"),
            vector_snapshot(&input, &c, &t)?,
            Some(&|| token.is_cancelled()),
        )
        .map_err(|e| {
            if token.is_cancelled() {
                failure("wave.cancelled", e)
            } else {
                e
            }
        })?;
    }
    token.check()?;
    let ann_ms = stage.elapsed().as_secs_f64() * 1000.0;
    std::fs::write(path.join("math.msgpack"), rmp_serde::to_vec_named(&math)?)?;
    std::fs::write(
        path.join("assets.msgpack"),
        rmp_serde::to_vec_named(&input)?,
    )?;
    std::fs::write(
        path.join("timings.json"),
        serde_json::to_vec(
            &json!({"total_ms":started.elapsed().as_secs_f64()*1000.0,"geometry_artifact_ms":math_ms,"lexical_ms":lexical_ms,"ann_ms":ann_ms}),
        )?,
    )?;
    let manifest = ["assets.msgpack", "math.msgpack", "timings.json"]
        .into_iter()
        .map(|name| Ok((name.to_string(), file_hash(&path.join(name), token)?)))
        .collect::<Result<BTreeMap<_, _>>>()?;
    std::fs::write(path.join("manifest.json"), serde_json::to_vec(&manifest)?)?;
    Ok(BuiltSnapshot {
        identity: input.identity.clone(),
        manifest,
    })
}
fn file_hash(path: &Path, cancel: &CancellationToken) -> Result<String> {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 65536];
    loop {
        cancel.check()?;
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

#[derive(Clone)]
pub struct Snapshot {
    pub inner: Arc<Frozen>,
}
impl Snapshot {
    pub fn open(path: &Path, expected: &Value, cancel: &CancellationToken) -> Result<Self> {
        Ok(Self {
            inner: Arc::new(load(path, expected, cancel)?),
        })
    }
}
