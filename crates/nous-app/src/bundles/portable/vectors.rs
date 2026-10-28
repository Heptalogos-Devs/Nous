use super::{Member, PortableOptions, hash, read_declared, semantic::Semantic};
use crate::bundle_format::{Images, canonical_json};
use anyhow::{Context, ensure};
use nous_domain::{media::*, *};
use npyz::WriterBuilder;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::Cursor,
    path::Path,
};
use zip::{ZipArchive, ZipWriter, write::SimpleFileOptions};

pub(super) struct VectorResult {
    pub text_fingerprint: Value,
    pub image_fingerprint: Value,
    pub retention: Value,
}
struct Array {
    ids: Vec<String>,
    dimension: usize,
    values: Vec<f32>,
}
pub(super) fn project(
    root: &Path,
    package: &SubjectPackage,
    options: &PortableOptions,
    semantic: &Semantic,
    images: Option<&Images>,
    members: &mut BTreeMap<String, Member>,
) -> anyhow::Result<VectorResult> {
    if !options.include_vectors {
        return Ok(VectorResult {
            text_fingerprint: Value::Null,
            image_fingerprint: Value::Null,
            retention: json!({"status":"excluded_by_request"}),
        });
    }
    let mut text_fingerprint = Value::Null;
    let mut image_fingerprint = Value::Null;
    let mut external: BTreeMap<String, Vec<(Value, Array)>> = BTreeMap::new();
    let mut retained = vec![];
    let image_ids = image_id_mapping(package, images);
    let graph_ids = graph_ids(semantic);
    // A retained foreign NPZ describes its imported source revision. Later local edits
    // have no accepted replacement vector provenance in that archive.
    let changed: BTreeSet<_> = semantic
        .targets
        .iter()
        .filter(|(key, _)| {
            semantic
                .revisions
                .get(*key)
                .is_some_and(|revision| *revision > 1)
        })
        .map(|((kind, _), id)| format!("{kind}:{id}"))
        .collect();
    retained.extend(reexport_extensions(
        root, package, semantic, &image_ids, &graph_ids, members,
    )?);
    for material in &package.materials {
        if material.details.source_type.as_deref() != Some("external_embedding_archive") {
            continue;
        }
        let metadata = &material.details.metadata;
        let Some(member) = metadata["member"].as_str() else {
            continue;
        };
        let fingerprint = match member {
            "vectors/paragraphs.npz" | "vectors/graph.npz" => {
                metadata["embedding_fingerprint"].clone()
            }
            "vectors/images.npz" if images.is_some() => {
                metadata["image_embedding_fingerprint"].clone()
            }
            _ => continue,
        };
        if fingerprint.is_null() {
            continue;
        }
        let array = read_npz(&read_declared(root, material)?)?;
        let array = select(array, |id| match member {
            "vectors/paragraphs.npz" => (semantic.paragraphs.contains(id)
                && !changed.contains(&format!("paragraph:{id}")))
            .then(|| id.into()),
            "vectors/graph.npz" => {
                (graph_ids.contains(id) && !changed.contains(id)).then(|| id.into())
            }
            "vectors/images.npz" => image_ids.get(id).cloned(),
            _ => None,
        });
        if !array.ids.is_empty() {
            external
                .entry(member.into())
                .or_default()
                .push((fingerprint, array));
        }
    }
    // One standard text fingerprint governs both paragraph and graph pools.
    let text_fingerprints = external
        .iter()
        .filter(|(m, _)| m.as_str() != "vectors/images.npz")
        .flat_map(|(_, arrays)| arrays.iter())
        .map(|(fp, _)| canonical_json(fp))
        .collect::<anyhow::Result<BTreeSet<_>>>()?;
    for (member, arrays) in external {
        let fingerprints = arrays
            .iter()
            .map(|(fp, _)| canonical_json(fp))
            .collect::<anyhow::Result<BTreeSet<_>>>()?;
        let compatible = fingerprints.len() == 1
            && (member == "vectors/images.npz" || text_fingerprints.len() == 1);
        if compatible {
            let fingerprint = arrays[0].0.clone();
            let merged = merge(arrays.into_iter().map(|(_, a)| a))?;
            if member == "vectors/images.npz" {
                image_fingerprint = fingerprint;
            } else {
                text_fingerprint = fingerprint;
            }
            members.insert(member, Member::Bytes(write_npz(&merged)?));
        } else {
            for (fingerprint, array) in arrays {
                let bytes = write_npz(&array)?;
                let extension = format!("vectors/external/{}.npz", hash(&bytes));
                retained.push(json!({"member":extension,"standard_pool":member,"fingerprint":fingerprint,"status":"multiple_incompatible_fingerprints"}));
                members.insert(extension, Member::Bytes(bytes));
            }
        }
    }
    let mut by_space: BTreeMap<String, BTreeMap<String, &EmbeddingRecord>> = BTreeMap::new();
    for association in &package.vectors.associations {
        if association.retired {
            continue;
        }
        let record = &association.record;
        let Some(id) = native_target(&record.target, semantic, &image_ids) else {
            continue;
        };
        let id = format!("{id}#{}", record.source_hash);
        let entries = by_space.entry(record.space_id.clone()).or_default();
        if entries
            .get(&id)
            .is_none_or(|old| old.commit_seq < record.commit_seq)
        {
            entries.insert(id, record);
        }
    }
    let mut native_count = 0usize;
    for (space_id, records) in by_space {
        let space = package
            .vectors
            .spaces
            .iter()
            .find(|s| s.id == space_id)
            .context("portable vector space is missing")?;
        space.validate()?;
        let mut array = Array {
            ids: vec![],
            dimension: space.dimension,
            values: vec![],
        };
        let mut sources = vec![];
        for (id, record) in records {
            ensure!(
                record.vector.len() == space.dimension
                    && record.vector.iter().all(|v| v.is_finite()),
                "invalid native portable vector"
            );
            array.ids.push(id.clone());
            array.values.extend_from_slice(&record.vector);
            sources.push(json!({"id":id,"source_hash":record.source_hash,"model_call_id":record.model_call_id}));
        }
        native_count += array.ids.len();
        let member = format!("vectors/nous/{}.npz", hash(space_id.as_bytes()));
        members.insert(member.clone(), Member::Bytes(write_npz(&array)?));
        retained.push(json!({"member":member,"space":space,"sources":sources,"status":"native_space_without_a_memorix_fingerprint"}));
    }
    if !retained.is_empty() {
        members.insert(
            "nous-vector-spaces.json".into(),
            Member::Bytes(canonical_json(&json!({"spaces":retained}))?),
        );
    }
    Ok(VectorResult {
        text_fingerprint,
        image_fingerprint,
        retention: json!({"status":if native_count == 0 {"no_native_vectors_selected"} else {"preserved_in_space_qualified_npz_extensions"},
        "native_vector_count":native_count,"standard_adoption":"requires an actual compatible A-Memorix fingerprint"}),
    })
}
fn native_target(
    target: &EmbeddingTarget,
    semantic: &Semantic,
    images: &BTreeMap<String, String>,
) -> Option<String> {
    match target.item()? {
        RetrievalTarget::Cognition(reference) => {
            let kind = match reference.kind {
                CognitiveKind::Memory => "paragraph",
                CognitiveKind::Entity => "entity",
                CognitiveKind::Association => "relation",
                _ => return None,
            };
            let id = semantic.targets.get(&(kind.into(), reference.id.clone()))?;
            if semantic.revisions.get(&(kind.into(), reference.id.clone()))
                != Some(&reference.revision)
            {
                return None;
            }
            if kind == "paragraph" && !semantic.paragraphs.contains(id) {
                return None;
            }
            Some(format!("{kind}:{id}"))
        }
        RetrievalTarget::Media(MediaReference::Original { material_id }) => images
            .get(material_id.as_str())
            .map(|id| format!("image:{id}")),
        _ => None,
    }
}
fn reexport_extensions(
    root: &Path,
    package: &SubjectPackage,
    semantic: &Semantic,
    images: &BTreeMap<String, String>,
    graph: &BTreeSet<String>,
    members: &mut BTreeMap<String, Member>,
) -> anyhow::Result<Vec<Value>> {
    let mut entries = vec![];
    for material in &package.materials {
        if material.details.source_type.as_deref() != Some("nous_portable_vector_metadata") {
            continue;
        }
        let metadata: Value = serde_json::from_slice(&read_declared(root, material)?)?;
        for entry in metadata["spaces"]
            .as_array()
            .context("portable vector metadata lacks spaces")?
        {
            let member = entry["member"]
                .as_str()
                .context("portable vector metadata lacks member")?;
            ensure!(
                (member.starts_with("vectors/nous/") || member.starts_with("vectors/external/"))
                    && member.ends_with(".npz"),
                "invalid vector extension member"
            );
            let Some(source) = package.materials.iter().find(|m| {
                m.details.source_type.as_deref() == Some("nous_portable_vector_archive")
                    && m.details.metadata["member"].as_str() == Some(member)
            }) else {
                continue;
            };
            let array = read_npz(&read_declared(root, source)?)?;
            let native = member.starts_with("vectors/nous/");
            let array = select(array, |id| {
                if native {
                    let (target, source_hash) = id.split_once('#')?;
                    let mapped = if let Some(id) = target.strip_prefix("paragraph:") {
                        return semantic
                            .paragraphs
                            .contains(id)
                            .then(|| format!("paragraph:{id}#{source_hash}"));
                    } else if let Some(id) = target.strip_prefix("image:") {
                        images.get(id).map(|id| format!("image:{id}"))
                    } else {
                        graph.contains(target).then(|| target.into())
                    };
                    return mapped.map(|target| format!("{target}#{source_hash}"));
                }
                match entry["standard_pool"].as_str() {
                    Some("vectors/paragraphs.npz") => {
                        semantic.paragraphs.contains(id).then(|| id.into())
                    }
                    Some("vectors/graph.npz") => graph.contains(id).then(|| id.into()),
                    Some("vectors/images.npz") => images.get(id).cloned(),
                    _ => None,
                }
            });
            if array.ids.is_empty() {
                continue;
            }
            let bytes = write_npz(&array)?;
            let new_member = format!(
                "vectors/{}/{}.npz",
                if native { "nous" } else { "external" },
                hash(&bytes)
            );
            members.insert(new_member.clone(), Member::Bytes(bytes));
            // Explicit fields prevent a retained vector metadata record from becoming a raw-input carrier.
            let mut output =
                json!({"member":new_member,"status":"retained_unadopted_vector_source"});
            if native {
                let space: EmbeddingSpace = serde_json::from_value(entry["space"].clone())?;
                space.validate()?;
                output["space"] = serde_json::to_value(space)?;
                output["sources"] = json!(entry["sources"].as_array().into_iter().flatten()
                    .filter(|s|s["id"].as_str().is_some_and(|id|array.ids.iter().any(|candidate|candidate == id)))
                    .map(|s|json!({"id":s["id"],"source_hash":s["source_hash"],"model_call_id":s["model_call_id"]})).collect::<Vec<_>>());
            } else {
                output["standard_pool"] = entry["standard_pool"].clone();
                output["fingerprint"] = entry["fingerprint"].clone();
            }
            entries.push(output);
        }
    }
    Ok(entries)
}
fn image_id_mapping(package: &SubjectPackage, images: Option<&Images>) -> BTreeMap<String, String> {
    let mut ids = BTreeMap::new();
    let Some(images) = images else {
        return ids;
    };
    for original in &package.materials {
        let Some(asset) = images
            .assets
            .iter()
            .find(|a| a.content_hash == original.material.content_hash)
        else {
            continue;
        };
        ids.insert(original.material.id.to_string(), asset.asset_id.clone());
        ids.insert(asset.asset_id.clone(), asset.asset_id.clone());
        if let Some(id) = original
            .details
            .metadata
            .pointer("/asset/asset_id")
            .and_then(Value::as_str)
        {
            ids.insert(id.into(), asset.asset_id.clone());
        }
    }
    ids
}
fn graph_ids(semantic: &Semantic) -> BTreeSet<String> {
    let mut ids: BTreeSet<_> = super::images::knowledge_graph_ids(&semantic.knowledge)
        .into_iter()
        .map(|(kind, id)| format!("{kind}:{id}"))
        .collect();
    if let Some(state) = &semantic.state {
        for (table, kind) in [("entities", "entity"), ("relations", "relation")] {
            for row in state["tables"][table].as_array().into_iter().flatten() {
                if let Some(id) = row["hash"].as_str() {
                    ids.insert(format!("{kind}:{id}"));
                }
            }
        }
    }
    ids
}
fn read_npz(bytes: &[u8]) -> anyhow::Result<Array> {
    let mut archive = ZipArchive::new(Cursor::new(bytes))?;
    ensure!(
        archive.len() == 2,
        "vector archive must contain ids and vectors"
    );
    let ids_member = archive.by_name("ids.npy")?;
    ensure!(
        ids_member.size() <= 256 * 1024 * 1024,
        "vector IDs exceed member limit"
    );
    let ids = npyz::NpyFile::new(ids_member)?;
    ensure!(
        ids.shape().len() == 1,
        "vector ids must be one-dimensional Unicode strings"
    );
    ensure!(
        ids.shape()[0] <= 1_000_000 && ids.dtype().num_bytes().is_some_and(|n| n <= 8192),
        "vector IDs exceed portable bounds"
    );
    let ids = ids.into_vec::<String>()?;
    let vectors_member = archive.by_name("vectors.npy")?;
    ensure!(
        vectors_member.size() <= 256 * 1024 * 1024,
        "vectors exceed member limit"
    );
    let vectors = npyz::NpyFile::new(vectors_member)?;
    ensure!(
        vectors.shape().len() == 2 && vectors.shape()[0] == ids.len() as u64,
        "vector array shape does not match ids"
    );
    let dimension = usize::try_from(vectors.shape()[1])?;
    ensure!(
        dimension > 0
            && dimension <= 65536
            && ids
                .len()
                .checked_mul(dimension)
                .is_some_and(|n| n <= 64 * 1024 * 1024),
        "vector array exceeds portable bounds"
    );
    ensure!(
        vectors.order() == npyz::Order::C,
        "vector array requires row-major order"
    );
    let values = vectors.into_vec::<f32>()?;
    ensure!(
        values.iter().all(|v| v.is_finite()),
        "non-finite portable vectors"
    );
    ensure!(
        ids.iter().collect::<BTreeSet<_>>().len() == ids.len(),
        "duplicate vector identifiers"
    );
    Ok(Array {
        ids,
        dimension,
        values,
    })
}
fn select(array: Array, mut mapping: impl FnMut(&str) -> Option<String>) -> Array {
    let mut result = Array {
        ids: vec![],
        dimension: array.dimension,
        values: vec![],
    };
    for (index, id) in array.ids.iter().enumerate() {
        if let Some(id) = mapping(id) {
            result.ids.push(id);
            result.values.extend_from_slice(
                &array.values[index * array.dimension..(index + 1) * array.dimension],
            );
        }
    }
    result
}
fn merge(arrays: impl Iterator<Item = Array>) -> anyhow::Result<Array> {
    let mut dimension = 0;
    let mut rows = BTreeMap::new();
    for array in arrays {
        ensure!(
            dimension == 0 || dimension == array.dimension,
            "same fingerprint has inconsistent vector dimensions"
        );
        dimension = array.dimension;
        for (index, id) in array.ids.into_iter().enumerate() {
            let vector = array.values[index * dimension..(index + 1) * dimension].to_vec();
            if let Some(previous) = rows.insert(id, vector.clone()) {
                ensure!(
                    previous == vector,
                    "same fingerprint and id have conflicting vectors"
                );
            }
        }
    }
    Ok(Array {
        ids: rows.keys().cloned().collect(),
        dimension,
        values: rows.into_values().flatten().collect(),
    })
}
fn write_npz(array: &Array) -> anyhow::Result<Vec<u8>> {
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    zip.start_file("ids.npy", options)?;
    let width = array
        .ids
        .iter()
        .map(|s| s.chars().count())
        .max()
        .unwrap_or(1)
        .max(1);
    let dtype = npyz::DType::Plain(format!("<U{width}").parse()?);
    let mut writer = npyz::WriteOptions::<str>::new()
        .dtype(dtype)
        .shape(&[array.ids.len() as u64])
        .writer(&mut zip)
        .begin_nd()?;
    for id in &array.ids {
        writer.push(id.as_str())?;
    }
    writer.finish()?;
    zip.start_file("vectors.npy", options)?;
    let mut writer = npyz::WriteOptions::new()
        .default_dtype()
        .shape(&[array.ids.len() as u64, array.dimension as u64])
        .writer(&mut zip)
        .begin_nd()?;
    writer.extend(array.values.iter().copied())?;
    writer.finish()?;
    Ok(zip.finish()?.into_inner())
}
