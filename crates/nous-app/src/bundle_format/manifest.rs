use super::{
    archive::ArchiveContents,
    json::{canonical_images, canonical_json},
    reference::build_index,
    types::*,
};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
pub(super) fn inspect_archive(
    archive: ArchiveContents,
    limits: &Limits,
) -> Result<BundleInspection> {
    let ArchiveContents {
        members,
        mut json_documents,
        archive_sha256,
        archive_bytes,
        expanded_bytes,
        mut remaining_nodes,
    } = archive;
    let manifest_raw = json_documents
        .remove("manifest.json")
        .context("missing manifest JSON")?;
    let manifest: Manifest =
        serde_json::from_value(manifest_raw).context("invalid bundle manifest schema")?;
    ensure!(
        manifest.format == FORMAT && matches!(manifest.format_version, 1 | 2),
        "unsupported bundle format/version"
    );
    for value in [
        &manifest.package.id,
        &manifest.package.version,
        &manifest.package.name,
    ] {
        ensure!(!value.trim().is_empty(), "invalid package identity");
    }
    ensure!(
        valid_hash(&manifest.content_digest),
        "invalid semantic content digest"
    );
    let declared_components: BTreeSet<_> = manifest.components.iter().cloned().collect();
    let actual_components: BTreeSet<_> = members
        .keys()
        .filter(|name| !matches!(name.as_str(), "manifest.json" | "checksums.json"))
        .cloned()
        .collect();
    ensure!(
        declared_components.len() == manifest.components.len()
            && declared_components == actual_components,
        "manifest components do not cover the archive"
    );
    let checksums: Checksums = serde_json::from_value(
        json_documents
            .remove("checksums.json")
            .context("missing checksums JSON")?,
    )
    .context("invalid checksums schema")?;
    ensure!(
        checksums.algorithm == "sha256",
        "unsupported bundle checksum algorithm"
    );
    let files = &checksums.files;
    ensure!(
        files.len() + 1 == members.len()
            && files
                .keys()
                .all(|name| name != "checksums.json" && members.contains_key(name)),
        "checksum coverage is incomplete"
    );
    for (name, expected) in files {
        ensure!(
            expected == &members[name].sha256,
            "bundle checksum mismatch: {name}"
        );
    }
    let knowledge_raw = json_documents
        .remove("knowledge.json")
        .context("missing knowledge JSON")?;
    let state_raw = json_documents.remove("state.json");
    let images_raw = json_documents.remove("images.json");
    ensure!(
        manifest.content_level != ContentLevel::Full || state_raw.is_some(),
        "full bundle requires state.json"
    );
    ensure!(
        manifest.format_version != 1 || images_raw.is_none(),
        "v1 bundle cannot contain image records"
    );
    ensure!(
        manifest.format_version != 2 || images_raw.is_some(),
        "v2 bundle requires images.json"
    );
    let knowledge: Knowledge =
        serde_json::from_value(knowledge_raw.clone()).context("invalid knowledge schema")?;
    ensure!(
        knowledge.format == "lpmm_openie" && knowledge.format_version == 1,
        "unsupported knowledge schema"
    );
    let state = state_raw
        .as_ref()
        .map(|state| serde_json::from_value::<State>(state.clone()))
        .transpose()
        .context("invalid full-state schema")?;
    if let Some(state) = &state {
        ensure!(
            state.schema == "a-memorix-semantic-state" && state.schema_version == 1,
            "unsupported semantic state schema"
        );
    }
    let images = images_raw
        .as_ref()
        .map(|images| serde_json::from_value::<Images>(images.clone()))
        .transpose()
        .context("invalid image record schema")?;
    let mut semantics = json!({"knowledge":knowledge_raw,"state":if manifest.content_level==ContentLevel::Full {state_raw.as_ref().unwrap_or(&Value::Null)}else{&Value::Null}});
    if let Some(images) = &images {
        semantics["images"] = canonical_images(images, &mut remaining_nodes, limits.json_depth)?;
    }
    ensure!(
        format!("{:x}", Sha256::digest(canonical_json(&semantics)?)) == manifest.content_digest,
        "bundle semantic content digest mismatch"
    );
    let index = build_index(&knowledge, state.as_ref(), images.as_ref(), &members)?;
    validate_counts(&manifest, &knowledge, state.as_ref(), images.as_ref())?;
    let required_capabilities = requirements(&knowledge, state.as_ref(), images.as_ref(), &index);
    Ok(BundleInspection {
        manifest,
        checksums,
        knowledge,
        state,
        images,
        members,
        index,
        required_capabilities,
        archive_sha256,
        archive_bytes,
        expanded_bytes,
    })
}

fn validate_counts(
    manifest: &Manifest,
    knowledge: &Knowledge,
    state: Option<&State>,
    images: Option<&Images>,
) -> Result<()> {
    if let Some(count) = manifest.counts.get("paragraphs") {
        ensure!(
            *count == knowledge.docs.len() as u64,
            "paragraph count mismatch"
        );
    }
    if manifest.content_level == ContentLevel::Full
        && let Some(state) = state
    {
        for (name, rows) in &state.tables {
            if let Some(count) = manifest.counts.get(name) {
                ensure!(
                    *count == rows.len() as u64,
                    "semantic state count mismatch: {name}"
                );
            }
        }
    }
    if let Some(images) = images {
        for (name, count) in [
            ("image_assets", images.assets.len()),
            ("image_occurrences", images.occurrences.len()),
            ("image_observations", images.observations.len()),
            ("image_links", images.links.len()),
        ] {
            if let Some(expected) = manifest.counts.get(name) {
                ensure!(*expected == count as u64, "image count mismatch");
            }
        }
    }
    Ok(())
}
fn requirements(
    knowledge: &Knowledge,
    state: Option<&State>,
    images: Option<&Images>,
    index: &ReferenceIndex,
) -> Vec<CapabilityRequirement> {
    let mut counts: BTreeMap<String, u64> = BTreeMap::new();
    counts.insert("package_installation".into(), 1);
    counts.insert("material_memory".into(), knowledge.docs.len() as u64);
    let graph = knowledge
        .docs
        .iter()
        .map(|doc| doc.extracted_entities.len() + doc.extracted_triples.len())
        .sum::<usize>();
    if graph > 0 {
        counts.insert("graph".into(), graph as u64);
    }
    if let Some(state) = state {
        for (name, rows) in &state.tables {
            if rows.is_empty() {
                continue;
            }
            let capability = match name.as_str() {
                "paragraphs" | "external_memory_refs" => "material_memory",
                "entities"
                | "relations"
                | "deleted_relations"
                | "paragraph_entities"
                | "paragraph_relations" => "graph",
                "episodes" | "episode_paragraphs" => "episodes",
                "fact_claims" | "fact_evidence" | "fact_transitions" => "facts",
                "person_profile_snapshots"
                | "person_profile_overrides"
                | "person_profile_alias_overrides" => "profiles",
                _ => "state_extensions",
            };
            *counts.entry(capability.into()).or_default() += rows.len() as u64;
        }
    }
    if let Some(images) = images {
        counts.insert("images".into(), images.assets.len() as u64);
    }
    let mut result: Vec<_> = counts
        .into_iter()
        .map(|(capability, records)| CapabilityRequirement {
            capability,
            records,
            members: Vec::new(),
        })
        .collect();
    for (kind, member) in &index.vector_members {
        result.push(CapabilityRequirement {
            capability: if kind == "image" {
                "image_embeddings"
            } else {
                "dense_vectors"
            }
            .into(),
            records: 1,
            members: vec![member.clone()],
        });
    }
    if !index.extension_members.is_empty() {
        result.push(CapabilityRequirement {
            capability: "archive_extensions".into(),
            records: index.extension_members.len() as u64,
            members: index.extension_members.clone(),
        });
    }
    result
}
