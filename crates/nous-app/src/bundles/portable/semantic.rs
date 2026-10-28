use super::{hash, read_declared};
use crate::bundle_format::{STATE_TABLES, canonical_json};
use anyhow::{Context, ensure};
use nous_domain::*;
use serde_json::{Map, Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

pub(super) type Tables = BTreeMap<String, Vec<Map<String, Value>>>;
pub(super) struct Semantic {
    pub knowledge: Value,
    pub state: Option<Value>,
    pub counts: BTreeMap<String, u64>,
    pub paragraphs: BTreeSet<String>,
    pub targets: BTreeMap<(String, String), String>,
    pub revisions: BTreeMap<(String, String), i64>,
    pub unmapped: BTreeMap<String, u64>,
}
pub(super) fn project(root: &Path, package: &SubjectPackage) -> anyhow::Result<Semantic> {
    let mut tables: Tables = STATE_TABLES
        .iter()
        .map(|(t, _)| ((*t).into(), vec![]))
        .collect();
    let mut sources = super::source_rows::Sources::default();
    // Foreign imports retain each source semantic row as a declared, independent material.
    for material in &package.materials {
        if material.details.source_type.as_deref() != Some("a_memorix_bundle") {
            continue;
        }
        let Some(table) = material
            .details
            .metadata
            .get("table")
            .and_then(Value::as_str)
        else {
            continue;
        };
        ensure!(
            tables.contains_key(table),
            "unsupported retained semantic table: {table}"
        );
        ensure!(
            material
                .details
                .metadata
                .get("row_index")
                .and_then(Value::as_u64)
                .is_some(),
            "semantic row lacks declared row index"
        );
        let row: Map<String, Value> = serde_json::from_slice(&read_declared(root, material)?)?;
        if sources.admit(table, &row, material)? {
            upsert(&mut tables, table, row)?;
        }
    }
    let mut memories = BTreeMap::new();
    for memory in &package.memories {
        let previous: Option<&&Memory> = memories.get(&memory.id);
        if previous.is_none_or(|old| old.revision < memory.revision) {
            memories.insert(memory.id.clone(), memory);
        }
    }
    let mut targets = BTreeMap::new();
    let mut revisions = BTreeMap::new();
    for memory in memories.values() {
        let original = package
            .materials
            .iter()
            .find(|m| m.material.id == memory.material_id);
        let external_id = original
            .and_then(|m| m.details.metadata.get("external_id"))
            .and_then(Value::as_str);
        let id = external_id.unwrap_or(memory.id.as_str()).to_owned();
        targets.insert(("paragraph".into(), memory.id.to_string()), id.clone());
        revisions.insert(("paragraph".into(), memory.id.to_string()), memory.revision);
        if memory.content.trim().is_empty() {
            continue;
        }
        let old = find(&tables, "paragraphs", "hash", &id).cloned();
        let mut row = old.unwrap_or_else(|| {
            Map::from_iter([
                ("hash".into(), json!(id)),
                ("knowledge_type".into(), json!("mixed")),
                ("source".into(), json!("nous-wave")),
                (
                    "metadata".into(),
                    json!({"epistemic_status":memory.epistemic_status})
                        .to_string()
                        .into(),
                ),
            ])
        });
        if let Some(meta) = original.and_then(|m| m.details.metadata.get("a_memorix")) {
            for field in ["source", "knowledge_type"] {
                if let Some(value) = meta.get(field) {
                    row.insert(field.into(), value.clone());
                }
            }
            if let Some(metadata) = meta.get("metadata") {
                row.insert("metadata".into(), json!(metadata.to_string()));
            }
            if let Some(time) = meta.get("time_meta").and_then(Value::as_object) {
                row.extend(time.clone());
            }
        }
        row.insert("content".into(), json!(memory.content));
        if memory.times.formation.domain == "unix_ms" {
            row.entry("created_at".to_owned())
                .or_insert(json!(memory.times.formation.coordinate as f64 / 1000.0));
        }
        if memory.times.recorded.domain == "unix_ms" {
            row.insert(
                "updated_at".into(),
                json!(memory.times.recorded.coordinate as f64 / 1000.0),
            );
        }
        row.insert("is_deleted".into(), json!(i32::from(memory.deleted)));
        row.insert(
            "is_permanent".into(),
            json!(i32::from(memory.lifecycle.pinned)),
        );
        if let Some(event) = &memory.times.event
            && event.domain == "unix_ms"
        {
            row.insert("event_time".into(), json!(event.coordinate as f64 / 1000.0));
        }
        upsert(&mut tables, "paragraphs", row)?;
    }
    let mut objects = BTreeMap::new();
    for object in &package.cognition {
        let previous: Option<&&StructuredObject> = objects.get(&object.id);
        if previous.is_none_or(|old| old.revision < object.revision) {
            objects.insert(object.id.clone(), object);
        }
    }
    let mut entities = BTreeMap::new();
    let mut unmapped = BTreeMap::new();
    for object in objects.values() {
        if let CognitiveContent::Entity(entity) = &object.body {
            let id = entity
                .identifiers
                .iter()
                .find(|i| i.system == "a_memorix.entity")
                .map(|i| i.key.clone())
                .unwrap_or_else(|| hash(entity.label.trim().to_lowercase().as_bytes()));
            entities.insert(object.id.to_string(), (id.clone(), entity.label.clone()));
            targets.insert(("entity".into(), object.id.to_string()), id.clone());
            revisions.insert(("entity".into(), object.id.to_string()), object.revision);
            let mut row = find(&tables, "entities", "hash", &id)
                .cloned()
                .unwrap_or_default();
            row.extend(
                json!({"hash":id,"name":entity.label,"is_deleted":i32::from(object.deleted)})
                    .as_object()
                    .unwrap()
                    .clone(),
            );
            row.entry("metadata".to_owned()).or_insert(json!(json!({"description":entity.description,"aliases":entity.aliases,"entity_type":entity.entity_type}).to_string()));
            upsert(&mut tables, "entities", row)?;
            let mut paragraphs = support_paragraphs(object, &targets, &revisions);
            for mention in &entity.mentions {
                let target = match &mention.target {
                    SemanticTarget::Object { identity }
                        if identity.kind == CognitiveKind::Memory =>
                    {
                        Some(identity.id.as_str())
                    }
                    SemanticTarget::Exact {
                        reference: RetrievalTarget::Cognition(reference),
                    } if reference.kind == CognitiveKind::Memory
                        && revisions.get(&("paragraph".into(), reference.id.clone()))
                            == Some(&reference.revision) =>
                    {
                        Some(reference.id.as_str())
                    }
                    _ => None,
                };
                if let Some(id) =
                    target.and_then(|id| targets.get(&("paragraph".into(), id.into())))
                {
                    paragraphs.insert(id.clone());
                }
            }
            for paragraph in paragraphs {
                upsert_value(
                    &mut tables,
                    "paragraph_entities",
                    json!({"paragraph_hash":paragraph,"entity_hash":id}),
                )?;
            }
        }
    }
    for object in objects.values() {
        match &object.body {
            CognitiveContent::Association(relation)
                if relation.polarity == AssociationPolarity::Positive =>
            {
                if [&relation.from, &relation.to].iter().any(|reference| {
                    revisions.get(&("entity".into(), reference.id.clone()))
                        != Some(&reference.revision)
                }) {
                    *unmapped
                        .entry("association_historical_endpoints".into())
                        .or_insert(0) += 1;
                    continue;
                }
                let (Some((_, subject)), Some((_, target))) = (
                    entities.get(&relation.from.id),
                    entities.get(&relation.to.id),
                ) else {
                    *unmapped
                        .entry("association_non_entity_endpoints".into())
                        .or_insert(0) += 1;
                    continue;
                };
                let triple = [
                    subject.as_str(),
                    relation.relation.as_str(),
                    target.as_str(),
                ];
                let generated_id = hash(
                    triple
                        .iter()
                        .map(|s| s.trim().to_lowercase())
                        .collect::<Vec<_>>()
                        .join("|")
                        .as_bytes(),
                );
                let id = tables["relations"]
                    .iter()
                    .find(|row| {
                        [
                            text(row, "subject"),
                            text(row, "predicate"),
                            text(row, "object"),
                        ]
                        .iter()
                        .zip(triple)
                        .all(|(a, b)| a.trim().to_lowercase() == b.trim().to_lowercase())
                    })
                    .map(|row| text(row, "hash"))
                    .unwrap_or(generated_id);
                targets.insert(("relation".into(), object.id.to_string()), id.clone());
                revisions.insert(("relation".into(), object.id.to_string()), object.revision);
                // Native relation provenance remains an attributed description, never a fabricated confidence.
                let mut row = find(&tables, "relations", "hash", &id)
                    .cloned()
                    .unwrap_or_default();
                row.extend(json!({"hash":id,"subject":subject,"predicate":relation.relation,"object":target,
                    "is_inactive":i32::from(object.deleted)}).as_object().unwrap().clone());
                row.entry("metadata".to_owned()).or_insert(json!(
                    json!({"description":relation.description,"basis_class":relation.basis_class})
                        .to_string()
                ));
                if !row.contains_key("retention_anchor_at") {
                    let point = if object.times.formation.domain == "unix_ms" {
                        &object.times.formation
                    } else {
                        &object.times.recorded
                    };
                    if point.domain != "unix_ms" {
                        *unmapped
                            .entry("association_without_unix_retention_anchor".into())
                            .or_insert(0) += 1;
                        continue;
                    }
                    row.insert(
                        "retention_anchor_at".into(),
                        json!(point.coordinate as f64 / 1000.0),
                    );
                }
                upsert(&mut tables, "relations", row)?;
                for paragraph in support_paragraphs(object, &targets, &revisions) {
                    upsert_value(
                        &mut tables,
                        "paragraph_relations",
                        json!({"paragraph_hash":paragraph,"relation_hash":id}),
                    )?;
                }
            }
            CognitiveContent::Entity(entity) => {
                if !entity.attributes.is_empty() {
                    *unmapped.entry("entity_attributes".into()).or_insert(0) +=
                        entity.attributes.len() as u64;
                }
            }
            CognitiveContent::MemorySemantics(_) => {
                *unmapped.entry("memory_semantics".into()).or_insert(0) += 1;
            }
            _ => {
                *unmapped
                    .entry(format!("{:?}", object.body.kind()).to_lowercase())
                    .or_insert(0) += 1;
            }
        }
    }
    let selected: BTreeSet<_> = memories
        .values()
        .filter(|m| !m.deleted && !m.content.trim().is_empty())
        .filter_map(|m| {
            targets
                .get(&("paragraph".into(), m.id.to_string()))
                .cloned()
        })
        .collect();
    nous_storage::bundle_semantics::retain_related(&mut tables, &selected);
    for object in objects.values().filter(|o| !o.deleted) {
        let (kind, table) = match object.body {
            CognitiveContent::Entity(_) => ("entity", "entities"),
            CognitiveContent::Association(_) => ("relation", "relations"),
            _ => continue,
        };
        if targets
            .get(&(kind.into(), object.id.to_string()))
            .is_some_and(|id| find(&tables, table, "hash", id).is_none())
        {
            *unmapped
                .entry(format!("{kind}_without_selected_paragraph_evidence"))
                .or_insert(0) += 1;
        }
    }
    targets.retain(|(kind, _), id| match kind.as_str() {
        "paragraph" => selected.contains(id),
        "entity" => find(&tables, "entities", "hash", id).is_some(),
        "relation" => find(&tables, "relations", "hash", id).is_some(),
        _ => false,
    });
    // This is the upstream knowledge projection: only retained paragraph links yield entities/triples.
    let mut docs = vec![];
    let mut paragraphs = BTreeSet::new();
    for row in &tables["paragraphs"] {
        if flag(row, "is_deleted") {
            continue;
        }
        let id = text(row, "hash");
        let passage = text(row, "content");
        if passage.trim().is_empty() {
            continue;
        }
        paragraphs.insert(id.clone());
        let mut entity_names = BTreeSet::new();
        for link in &tables["paragraph_entities"] {
            if text(link, "paragraph_hash") == id
                && let Some(entity) = find(&tables, "entities", "hash", &text(link, "entity_hash"))
                && !flag(entity, "is_deleted")
            {
                entity_names.insert(text(entity, "name"));
            }
        }
        let mut triples = vec![];
        for link in &tables["paragraph_relations"] {
            if text(link, "paragraph_hash") == id
                && let Some(relation) =
                    find(&tables, "relations", "hash", &text(link, "relation_hash"))
            {
                let triple = [
                    text(relation, "subject"),
                    text(relation, "predicate"),
                    text(relation, "object"),
                ];
                if !flag(relation, "is_inactive") && triple.iter().all(|s| !s.trim().is_empty()) {
                    triples.push(triple);
                }
            }
        }
        let time: Map<_, _> = [
            "event_time",
            "event_time_start",
            "event_time_end",
            "time_granularity",
            "time_confidence",
        ]
        .into_iter()
        .filter_map(|key| {
            row.get(key)
                .filter(|v| !v.is_null())
                .map(|v| (key.into(), v.clone()))
        })
        .collect();
        docs.push(json!({"id":id,"passage":passage,"extracted_entities":entity_names,"extracted_triples":triples,
            "a_memorix":{"knowledge_type":text(row,"knowledge_type"),"source":text(row,"source"),"time_meta":time,"metadata":object_json(row.get("metadata"))}}));
    }
    // Full paragraph rows and knowledge docs share the same active export set, as upstream does.
    tables
        .get_mut("paragraphs")
        .unwrap()
        .retain(|r| paragraphs.contains(&text(r, "hash")));
    for table in [
        "paragraph_entities",
        "paragraph_relations",
        "external_memory_refs",
        "episode_paragraphs",
    ] {
        tables
            .get_mut(table)
            .unwrap()
            .retain(|r| paragraphs.contains(&text(r, "paragraph_hash")));
    }
    let full = package.content_level == BundleContentLevel::Full;
    let counts = tables
        .iter()
        .filter(|(name, _)| {
            full || matches!(name.as_str(), "paragraphs" | "entities" | "relations")
        })
        .map(|(name, rows)| (name.clone(), rows.len() as u64))
        .collect();
    Ok(Semantic {
        knowledge:json!({"format":"lpmm_openie","format_version":1,"docs":docs}),
        state:full.then(||json!({"schema":"a-memorix-semantic-state","schema_version":1,"tables":tables,
            "excluded":["FTS、N-gram、关系图快照会在目标实例重建","后台队列、运行时任务、反馈和删除审计不属于可移植语义状态","聊天流开关、活跃人物集合等实例配置不会随包迁移"]})),
        counts, paragraphs, targets, revisions, unmapped,
    })
}
fn support_paragraphs(
    object: &StructuredObject,
    targets: &BTreeMap<(String, String), String>,
    revisions: &BTreeMap<(String, String), i64>,
) -> BTreeSet<String> {
    object
        .evidence
        .iter()
        .filter(|e| e.polarity == EvidencePolarity::Support)
        .filter_map(|e| e.source.cognitive_ref())
        .filter(|r| {
            r.kind == CognitiveKind::Memory
                && revisions.get(&("paragraph".into(), r.id.clone())) == Some(&r.revision)
        })
        .filter_map(|r| targets.get(&("paragraph".into(), r.id)).cloned())
        .collect()
}
fn find<'a>(
    tables: &'a Tables,
    table: &str,
    field: &str,
    value: &str,
) -> Option<&'a Map<String, Value>> {
    tables[table]
        .iter()
        .find(|r| r.get(field).and_then(Value::as_str) == Some(value))
}
fn upsert_value(tables: &mut Tables, table: &str, row: Value) -> anyhow::Result<()> {
    upsert(
        tables,
        table,
        row.as_object()
            .context("semantic row must be an object")?
            .clone(),
    )
}
fn upsert(tables: &mut Tables, table: &str, row: Map<String, Value>) -> anyhow::Result<()> {
    let identity = |row: &Map<String, Value>| -> anyhow::Result<Vec<u8>> {
        canonical_json(&super::source_rows::row_key(table, row)?)
    };
    let key = identity(&row)?;
    let rows = tables.get_mut(table).context("missing semantic table")?;
    if let Some(index) = rows
        .iter()
        .position(|r| identity(r).is_ok_and(|v| v == key))
    {
        rows[index] = row;
    } else {
        rows.push(row);
    }
    Ok(())
}
pub(super) fn text(row: &Map<String, Value>, field: &str) -> String {
    row.get(field)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .into()
}
fn flag(row: &Map<String, Value>, field: &str) -> bool {
    row.get(field)
        .is_some_and(|v| v.as_bool() == Some(true) || v.as_i64().is_some_and(|v| v != 0))
}
pub(super) fn object_json(value: Option<&Value>) -> Value {
    match value {
        Some(Value::Object(v)) => json!(v),
        Some(Value::String(v)) => serde_json::from_str::<Value>(v)
            .ok()
            .filter(Value::is_object)
            .unwrap_or(json!({})),
        _ => json!({}),
    }
}
