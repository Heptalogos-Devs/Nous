//! Immutable, reopenable Wave assets and the shared bounded native worker pool.
mod delta;
mod identity;
mod math;
mod validation;

use crate::{CancellationToken, bundles::*, failure};
use crate::{
    RetrievalIndex,
    vector::{VectorGeneration, VectorGenerationIdentity, VectorRecord, VectorSnapshot},
};
use anyhow::{Context, Result, ensure};
use identity::StableIds;
use math::{assemble, build_math};
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
    pub titles: Vec<String>,
    pub group_keys: Vec<String>,
    pub group_rows: Vec<u64>,
    pub local: Vec<bool>,
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
#[derive(Clone, Serialize, Deserialize)]
struct MathDisk {
    graph: Arc<NativeArtifact>,
    epa_json: String,
    signature: String,
    geometry: Value,
}
pub struct Frozen {
    pub path: PathBuf,
    math: Option<MathDisk>,
    pub input: Input,
    pub candidates: CandidateMeta,
    ids: StableIds,
    pub candidate_ids: Vec<i64>,
    pub candidate_rows: HashMap<i64, u64>,
    pub group_ranks: Arc<BTreeMap<u64, u64>>,
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
pub fn load(path: &Path, expected: &Value, cancel: &CancellationToken) -> Result<Frozen> {
    cancel.check()?;
    ensure!(
        path.join("lexical/meta.json").is_file(),
        "Snapshot lexical index is missing"
    );
    let manifest: BTreeMap<String, String> =
        serde_json::from_slice(&std::fs::read(path.join("manifest.json"))?)?;
    for name in [
        "assets.msgpack",
        "math.msgpack",
        "timings.json",
        "lexical/meta.json",
        "identities.msgpack",
    ] {
        ensure!(
            manifest.contains_key(name),
            "Snapshot component missing from manifest: {name}"
        );
    }
    for (name, expected_hash) in &manifest {
        ensure!(
            Path::new(name)
                .components()
                .all(|component| matches!(component, std::path::Component::Normal(_))),
            "Invalid snapshot component path"
        );
        ensure!(
            expected_hash == &file_hash(&path.join(name), cancel)?,
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
    let ids: StableIds = rmp_serde::from_slice(&std::fs::read(path.join("identities.msgpack"))?)?;
    ids.validate(&c)?;
    let candidate_ids: Vec<i64> = c.keys.iter().map(|key| ids.candidates[key]).collect();
    let candidate_rows = candidate_ids
        .iter()
        .enumerate()
        .map(|(row, id)| (*id, row as u64))
        .collect();
    let group_ranks = Arc::new(
        c.group_keys
            .iter()
            .enumerate()
            .map(|(row, key)| (ids.groups[key], row as u64))
            .collect(),
    );
    let math: Option<MathDisk> = rmp_serde::from_slice(&std::fs::read(path.join("math.msgpack"))?)?;
    let assets = if let Some(math) = &math {
        Some(assemble(&input, &c, &t, &f, math)?)
    } else {
        None
    };
    let ann = if input.candidates.vectors.is_some() {
        Some(VectorGeneration::open(
            &path.join("vectors"),
            &vector_snapshot(&input, &c, &t, &ids)?,
        )?)
    } else {
        None
    };
    let lexical = RetrievalIndex::open(path.join("lexical"))?;
    lexical.close()?;
    let timings: Value = serde_json::from_slice(&std::fs::read(path.join("timings.json"))?)?;
    let geometry=math.as_ref().map(|m|json!({"algorithm_version":m.geometry["algorithm_version"],"signature":m.geometry["signature"],"tag_count":m.geometry["tag_count"],"computed_count":m.geometry["computed_count"]}));
    let dimension = input.candidates.vectors.as_ref().map(|v| v.cols);
    let stats = json!({"dimension":dimension,"timings":timings,"candidates":c.keys.len(),"groups":c.group_keys.len(),"concepts":t.keys.len(),"evidence_roots":f.root_keys.len(),"source_sequences":f.sequence_keys.len(),"associations":a.keys.len(),"graph_nodes":math.as_ref().map_or(0,|m|m.graph.node_ids.len()),"graph_edges":math.as_ref().map_or(0,|m|m.graph.targets.len()),"geometry":geometry});
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
        path: path.to_path_buf(),
        math,
        input,
        candidates: c,
        ids,
        candidate_ids,
        candidate_rows,
        group_ranks,
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
pub fn build(
    input: Input,
    output_dir: &Path,
    base: Option<&Frozen>,
    token: &CancellationToken,
) -> Result<BuiltSnapshot> {
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
    let ids = StableIds::build(
        &c,
        base.filter(|old| delta::compatible(&input, old))
            .map(|old| &old.ids),
    )?;
    let started = std::time::Instant::now();
    let stage = std::time::Instant::now();
    let reusable_math = base.filter(|old| {
        delta::compatible(&input, old)
            && input.concepts == old.input.concepts
            && input.facts == old.input.facts
            && input.associations == old.input.associations
            && input.candidates.vectors.is_some() == old.input.candidates.vectors.is_some()
    });
    let math = if let Some(old) = reusable_math {
        old.math.clone()
    } else {
        build_math(&input, &t, token)?
    };
    let math_ms = stage.elapsed().as_secs_f64() * 1000.0;
    let stage = std::time::Instant::now();
    let lexical_delta = delta::build_lexical(&input, &c, &ids, &path.join("lexical"), base, token)?;
    let lexical_ms = stage.elapsed().as_secs_f64() * 1000.0;
    let stage = std::time::Instant::now();
    token.check()?;
    let mut ann_delta = json!({"incremental": false});
    if input.candidates.vectors.is_some() {
        let next = vector_snapshot(&input, &c, &t, &ids)?;
        let result = if let Some(old) = base
            .filter(|old| delta::compatible(&input, old) && old.input.candidates.vectors.is_some())
        {
            let previous = vector_snapshot(&old.input, &old.candidates, &old.concepts, &old.ids)?;
            VectorGeneration::create_incremental(
                &path.join("vectors"),
                next,
                &old.path.join("vectors"),
                &previous,
                Some(&|| token.is_cancelled()),
            )
            .map(|(_, stats)| stats)
        } else {
            VectorGeneration::create(&path.join("vectors"), next, Some(&|| token.is_cancelled()))
                .map(|_| json!({"incremental": false}))
        };
        ann_delta = result.map_err(|e| {
            if token.is_cancelled() {
                failure("wave.cancelled", e)
            } else {
                e
            }
        })?;
    }
    token.check()?;
    let ann_ms = stage.elapsed().as_secs_f64() * 1000.0;
    write_synced(
        &path.join("identities.msgpack"),
        &rmp_serde::to_vec_named(&ids)?,
    )?;
    write_synced(&path.join("math.msgpack"), &rmp_serde::to_vec_named(&math)?)?;
    write_synced(
        &path.join("assets.msgpack"),
        &rmp_serde::to_vec_named(&input)?,
    )?;
    write_synced(
        &path.join("timings.json"),
        &serde_json::to_vec(
            &json!({"total_ms":started.elapsed().as_secs_f64()*1000.0,"geometry_artifact_ms":math_ms,"math_reused":reusable_math.is_some(),"lexical_ms":lexical_ms,"lexical_delta":lexical_delta,"ann_ms":ann_ms,"ann_delta":ann_delta}),
        )?,
    )?;
    sync_directory(&path.join("lexical"))?;
    let manifest = component_manifest(&path, token)?;
    let pending_manifest = path.join(format!(".manifest-{}.tmp", uuid::Uuid::new_v4()));
    {
        use std::io::Write;
        let mut file = std::fs::File::create(&pending_manifest)?;
        file.write_all(&serde_json::to_vec(&manifest)?)?;
        file.sync_all()?;
    }
    std::fs::rename(&pending_manifest, path.join("manifest.json"))?;
    sync_directory(&path)?;
    if let Some(parent) = path.parent() {
        sync_directory(parent)?;
    }
    Ok(BuiltSnapshot {
        identity: input.identity.clone(),
        manifest,
    })
}

fn write_synced(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    let mut file = std::fs::File::create(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

fn sync_directory(path: &Path) -> Result<()> {
    #[cfg(not(windows))]
    std::fs::File::open(path)?.sync_all()?;
    #[cfg(windows)]
    let _ = path;
    Ok(())
}

fn component_manifest(root: &Path, cancel: &CancellationToken) -> Result<BTreeMap<String, String>> {
    let mut manifest = BTreeMap::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        cancel.check()?;
        for entry in std::fs::read_dir(directory)? {
            let entry = entry?;
            let path = entry.path();
            let kind = entry.file_type()?;
            if kind.is_dir() {
                pending.push(path);
            } else if kind.is_file() {
                let relative = path.strip_prefix(root)?;
                let name = relative
                    .to_str()
                    .context("Snapshot component path is not UTF-8")?;
                let name = name.replace('\\', "/");
                let filename = entry.file_name();
                let filename = filename.to_string_lossy();
                if name == "manifest.json" || filename.ends_with(".lock") {
                    continue;
                }
                manifest.insert(name, file_hash(&path, cancel)?);
            } else {
                ensure!(false, "Unsupported snapshot component type");
            }
        }
    }
    Ok(manifest)
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

fn vector_snapshot(
    input: &Input,
    c: &CandidateMeta,
    t: &ConceptMeta,
    ids: &StableIds,
) -> Result<VectorSnapshot> {
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
                id: ids.candidates[key],
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
