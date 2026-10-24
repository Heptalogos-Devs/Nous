use super::{
    Images, Knowledge, Member, MemberKind, ReferenceIndex, STATE_TABLES, State,
    json::canonical_json, valid_hash,
};
use anyhow::{Context, Result, ensure};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
pub(super) fn build_index(
    knowledge: &Knowledge,
    state: Option<&State>,
    images: Option<&Images>,
    members: &BTreeMap<String, Member>,
) -> Result<ReferenceIndex> {
    let mut index = ReferenceIndex::default();
    for (position, doc) in knowledge.docs.iter().enumerate() {
        ensure!(
            !doc.id.trim().is_empty() && !doc.passage.trim().is_empty(),
            "knowledge document identity and passage are required"
        );
        ensure!(
            index.documents.insert(doc.id.clone(), position).is_none(),
            "duplicate knowledge document ID"
        );
        for entity in doc.extracted_entities.iter().chain(
            doc.extracted_triples
                .iter()
                .flat_map(|triple| [&triple[0], &triple[2]]),
        ) {
            ensure!(!entity.trim().is_empty(), "empty extracted entity");
            index.knowledge_entities.insert(format!(
                "{:x}",
                Sha256::digest(entity.trim().to_lowercase().as_bytes())
            ));
        }
        for triple in &doc.extracted_triples {
            ensure!(
                triple.iter().all(|part| !part.trim().is_empty()),
                "empty extracted triple component"
            );
            let key = triple
                .iter()
                .map(|part| part.trim().to_lowercase())
                .collect::<Vec<_>>()
                .join("|");
            index
                .knowledge_relations
                .insert(format!("{:x}", Sha256::digest(key.as_bytes())));
        }
    }
    if let Some(state) = state {
        for (table, rows) in &state.tables {
            let keys = STATE_TABLES
                .iter()
                .find(|(name, _)| name == table)
                .map(|(_, keys)| *keys);
            let mut row_index = BTreeMap::new();
            for (position, row) in rows.iter().enumerate() {
                let identity = if let Some(keys) = keys {
                    Value::Array(
                        keys.iter()
                            .map(|key| {
                                row.get(*key)
                                    .cloned()
                                    .context("state row lacks primary key")
                            })
                            .collect::<Result<_>>()?,
                    )
                } else {
                    Value::Object(row.clone())
                };
                let key = String::from_utf8(canonical_json(&identity)?)?;
                ensure!(
                    row_index.insert(key, position).is_none(),
                    "duplicate semantic state row identity"
                );
            }
            index.state_rows.insert(table.clone(), row_index);
        }
    }
    validate_state_refs(state, &index)?;
    if let Some(images) = images {
        let mut declared = BTreeSet::new();
        for (position, asset) in images.assets.iter().enumerate() {
            ensure!(
                valid_hash(&asset.content_hash) && asset.member.starts_with("images/assets/"),
                "invalid image asset reference"
            );
            let member = members
                .get(&asset.member)
                .context("image asset member is missing")?;
            ensure!(
                member.sha256 == asset.content_hash && member.byte_len == asset.byte_size,
                "image asset hash/size mismatch"
            );
            ensure!(
                declared.insert(asset.member.clone())
                    && index
                        .image_assets
                        .insert(asset.asset_id.clone(), position)
                        .is_none(),
                "duplicate image asset identity/member"
            );
        }
        let actual: BTreeSet<_> = members
            .keys()
            .filter(|name| name.starts_with("images/assets/"))
            .cloned()
            .collect();
        ensure!(actual == declared, "image asset membership is not closed");
        for (position, occurrence) in images.occurrences.iter().enumerate() {
            ensure!(
                index.image_assets.contains_key(&occurrence.asset_id),
                "image occurrence references unknown asset"
            );
            ensure!(
                index
                    .image_occurrences
                    .insert(occurrence.occurrence_id.clone(), position)
                    .is_none(),
                "duplicate image occurrence ID"
            );
        }
        for observation in &images.observations {
            ensure!(
                index
                    .image_occurrences
                    .contains_key(&observation.occurrence_id),
                "image observation references unknown occurrence"
            );
        }
        for link in &images.links {
            ensure!(
                index.image_occurrences.contains_key(&link.occurrence_id),
                "image link references unknown occurrence"
            );
            ensure!(
                target_exists(&link.target_type, &link.target_id, &index),
                "image link references a target outside the bundle"
            );
        }
    } else {
        ensure!(
            !members
                .keys()
                .any(|name| name.starts_with("images/assets/")),
            "image assets lack image records"
        );
    }
    for (name, member) in members {
        match member.kind {
            MemberKind::ParagraphVectors => {
                index
                    .vector_members
                    .insert("paragraph".into(), name.clone());
            }
            MemberKind::GraphVectors => {
                index.vector_members.insert("graph".into(), name.clone());
            }
            MemberKind::ImageVectors => {
                ensure!(images.is_some(), "image vectors lack image records");
                index.vector_members.insert("image".into(), name.clone());
            }
            MemberKind::Extension => index.extension_members.push(name.clone()),
            _ => {}
        }
    }
    Ok(index)
}
fn target_exists(kind: &str, id: &str, index: &ReferenceIndex) -> bool {
    if kind == "paragraph" && index.documents.contains_key(id) {
        return true;
    }
    if kind == "entity" && index.knowledge_entities.contains(id)
        || kind == "relation" && index.knowledge_relations.contains(id)
    {
        return true;
    }
    let table = match kind {
        "paragraph" => "paragraphs",
        "entity" => "entities",
        "relation" => "relations",
        "episode" => "episodes",
        _ => return false,
    };
    let key = format!(
        "[{}]",
        serde_json::to_string(id).expect("string is JSON serializable")
    );
    index
        .state_rows
        .get(table)
        .is_some_and(|rows| rows.contains_key(&key))
}
fn validate_state_refs(state: Option<&State>, index: &ReferenceIndex) -> Result<()> {
    let Some(state) = state else {
        return Ok(());
    };
    for (table, rows) in &state.tables {
        for row in rows {
            for (field, kind) in match table.as_str() {
                "paragraph_entities" => {
                    &[("paragraph_hash", "paragraph"), ("entity_hash", "entity")][..]
                }
                "paragraph_relations" => &[
                    ("paragraph_hash", "paragraph"),
                    ("relation_hash", "relation"),
                ][..],
                "episode_paragraphs" => {
                    &[("episode_id", "episode"), ("paragraph_hash", "paragraph")][..]
                }
                "external_memory_refs" => &[("paragraph_hash", "paragraph")][..],
                _ => &[][..],
            } {
                let id = row
                    .get(*field)
                    .and_then(Value::as_str)
                    .context("invalid semantic reference")?;
                ensure!(
                    target_exists(kind, id, index),
                    "semantic state references a missing {kind}: {id}"
                );
            }
        }
    }
    Ok(())
}
