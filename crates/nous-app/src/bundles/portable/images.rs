use super::{Member, PortableOptions, blob, semantic::Semantic};
use crate::bundle_format::{ImageAsset, ImageLink, ImageObservation, ImageOccurrence, Images};
use nous_domain::{media::*, *};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

pub(super) fn project(
    root: &Path,
    package: &SubjectPackage,
    options: &PortableOptions,
    semantic: &Semantic,
    members: &mut BTreeMap<String, Member>,
) -> anyhow::Result<Option<Images>> {
    if !options.include_images {
        return Ok(None);
    }
    let mut images = Images {
        assets: vec![],
        occurrences: vec![],
        observations: vec![],
        links: vec![],
        fingerprint: json!({}),
        extra: BTreeMap::new(),
    };
    let mut materials = BTreeMap::new();
    let mut hashes = BTreeSet::new();
    for original in &package.materials {
        let material = &original.material;
        let MediaMetadata::Image { width, height, .. } = &material.metadata else {
            continue;
        };
        let id = material.content_hash.clone();
        materials.insert(material.id.clone(), id.clone());
        if !hashes.insert(id.clone()) {
            continue;
        }
        let member = format!("images/assets/{id}.{}", suffix(&material.media_type));
        let size = u64::try_from(material.byte_len)?;
        members.insert(member.clone(), blob(root, &id, size)?);
        images.assets.push(ImageAsset {
            asset_id: id.clone(),
            content_hash: id,
            member,
            byte_size: size,
            mime_type: material.media_type.clone(),
            width: u64::from(*width),
            height: u64::from(*height),
            extra: BTreeMap::new(),
        });
    }
    if images.assets.is_empty() {
        return Ok(None);
    }
    let mut occurrences = BTreeMap::new();
    let mut retained_occurrences = BTreeSet::new();
    for observation in &package.observations {
        let Some(asset) = materials.get(&observation.material_id) else {
            continue;
        };
        let id = observation.id.to_string();
        occurrences.insert(observation.id.clone(), id.clone());
        let retained = package
            .materials
            .iter()
            .find(|m| m.material.id == observation.material_id)
            .and_then(|m| m.details.metadata["image_occurrences"].as_array())
            .and_then(|records| {
                records
                    .iter()
                    .find(|r| r["observation_id"].as_str() == Some(observation.id.as_str()))
            });
        if let Some(retained) = retained {
            let mut occurrence: ImageOccurrence =
                serde_json::from_value(retained["record"].clone())?;
            occurrence.extra.insert(
                "external_occurrence_id".into(),
                json!(occurrence.occurrence_id),
            );
            occurrence.occurrence_id = id.clone();
            occurrence.asset_id = asset.clone();
            occurrence.extra.insert(
                "receiving_event_kind".into(),
                serde_json::to_value(observation.kind)?,
            );
            images.occurrences.push(occurrence);
            retained_occurrences.insert(id.clone());
            for record in retained["observations"].as_array().into_iter().flatten() {
                let mut record: ImageObservation = serde_json::from_value(record.clone())?;
                record.occurrence_id = id.clone();
                images.observations.push(record);
            }
            for record in retained["links"].as_array().into_iter().flatten() {
                let mut record: ImageLink = serde_json::from_value(record.clone())?;
                record.occurrence_id = id.clone();
                if target_exists(semantic, &record.target_type, &record.target_id) {
                    images.links.push(record);
                }
            }
            continue;
        }
        images.occurrences.push(ImageOccurrence {
            occurrence_id: id,
            asset_id: asset.clone(),
            component_path: String::new(),
            extra: BTreeMap::from([
                (
                    "source_kind".into(),
                    serde_json::to_value(observation.kind)?,
                ),
                ("recorded_at".into(), json!(observation.recorded_at)),
                ("observed_at".into(), json!(observation.observed_at)),
                ("origin".into(), json!(observation.origin)),
                ("status".into(), json!("active")),
            ]),
        });
    }
    let mut representations = BTreeMap::new();
    for representation in &package.representations {
        let previous: Option<&&DerivedRepresentation> = representations.get(&representation.id);
        if previous.is_none_or(|v| v.revision < representation.revision) {
            representations.insert(representation.id.clone(), representation);
        }
    }
    for representation in representations.values() {
        if representation.kind != RepresentationKind::ImageDescription {
            continue;
        }
        let Some(occurrence) = representation
            .observation_id
            .as_ref()
            .and_then(|id| occurrences.get(id))
        else {
            continue;
        };
        let RepresentationPayload::Text { text } = &representation.payload else {
            continue;
        };
        if representation.producer.implementation == "a-memorix.imported-description"
            && retained_occurrences.contains(occurrence)
        {
            continue;
        }
        if text.trim().is_empty() {
            continue;
        }
        images.observations.push(ImageObservation {
            occurrence_id:occurrence.clone(), text:text.clone(),
            source_kind:if representation.producer.implementation == "a-memorix.imported-description" {"imported_description"} else {"model_description"}.into(),
            confirm_status:"unconfirmed".into(),
            evidence_json:json!({"representation_id":representation.id,"revision":representation.revision,
                "producer":representation.producer.implementation,"producer_signature":representation.producer.signature,
                "source_event_kind":representation.source_event_kind}),
            extra:BTreeMap::new(),
        });
    }
    let mut links = BTreeSet::new();
    for memory in &package.memories {
        if semantic
            .revisions
            .get(&("paragraph".into(), memory.id.to_string()))
            != Some(&memory.revision)
        {
            continue;
        }
        let Some(target) = semantic
            .targets
            .get(&("paragraph".into(), memory.id.to_string()))
        else {
            continue;
        };
        if memory.deleted || !semantic.paragraphs.contains(target) {
            continue;
        }
        if let Some(occurrence) = occurrences.get(&memory.observation_id) {
            links.insert((
                occurrence.clone(),
                "paragraph".to_owned(),
                target.clone(),
                "source".to_owned(),
            ));
        }
    }
    for object in &package.cognition {
        if object.deleted {
            continue;
        }
        if package
            .cognition
            .iter()
            .any(|v| v.id == object.id && v.revision > object.revision)
        {
            continue;
        }
        let target = match &object.body {
            CognitiveContent::MemorySemantics(content) => semantic
                .targets
                .get(&("paragraph".into(), content.memory_id.to_string()))
                .map(|id| ("paragraph", id)),
            CognitiveContent::Entity(_) => semantic
                .targets
                .get(&("entity".into(), object.id.to_string()))
                .map(|id| ("entity", id)),
            CognitiveContent::Association(_) => semantic
                .targets
                .get(&("relation".into(), object.id.to_string()))
                .map(|id| ("relation", id)),
            _ => None,
        };
        let Some((kind, id)) = target else {
            continue;
        };
        for evidence in &object.evidence {
            let occurrence = match &evidence.source {
                EvidenceSource::Observation { observation_id } => occurrences.get(observation_id),
                _ => None,
            };
            if let Some(occurrence) = occurrence {
                links.insert((
                    occurrence.clone(),
                    kind.into(),
                    id.clone(),
                    format!("{:?}", evidence.polarity).to_lowercase(),
                ));
            }
        }
    }
    // Knowledge image links must refer to a standard knowledge graph identity.
    for (occurrence_id, target_type, target_id, link_kind) in links {
        let valid = target_exists(semantic, &target_type, &target_id);
        if valid {
            images.links.push(ImageLink {
                occurrence_id,
                target_type,
                target_id,
                link_kind,
                evidence_json: json!({}),
                extra: BTreeMap::new(),
            });
        }
    }
    Ok(Some(images))
}
fn target_exists(semantic: &Semantic, kind: &str, id: &str) -> bool {
    if kind == "paragraph" {
        return semantic.paragraphs.contains(id);
    }
    if let Some(state) = &semantic.state {
        let (table, key) = match kind {
            "entity" => ("entities", "hash"),
            "relation" => ("relations", "hash"),
            "episode" => ("episodes", "episode_id"),
            _ => return false,
        };
        return state["tables"][table]
            .as_array()
            .into_iter()
            .flatten()
            .any(|r| r[key].as_str() == Some(id));
    }
    knowledge_graph_ids(&semantic.knowledge).contains(&(kind.into(), id.into()))
}
pub(super) fn knowledge_graph_ids(knowledge: &Value) -> BTreeSet<(String, String)> {
    let mut ids = BTreeSet::new();
    for doc in knowledge["docs"].as_array().into_iter().flatten() {
        for name in doc["extracted_entities"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
        {
            ids.insert((
                "entity".into(),
                super::hash(name.trim().to_lowercase().as_bytes()),
            ));
        }
        for triple in doc["extracted_triples"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_array)
        {
            for name in [triple.first(), triple.get(2)]
                .into_iter()
                .flatten()
                .filter_map(|v| v.as_str())
            {
                ids.insert((
                    "entity".into(),
                    super::hash(name.trim().to_lowercase().as_bytes()),
                ));
            }
            let key = triple
                .iter()
                .filter_map(Value::as_str)
                .map(|s| s.trim().to_lowercase())
                .collect::<Vec<_>>()
                .join("|");
            ids.insert(("relation".into(), super::hash(key.as_bytes())));
        }
    }
    ids
}
fn suffix(media_type: &str) -> &str {
    match media_type {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "image/avif" => "avif",
        _ => "bin",
    }
}
