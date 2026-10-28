use super::{
    selection_graph::Selection,
    semantics::{Tables, retain_related},
};
use nous_domain::*;
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};

const TABLES: &[&str] = &[
    "paragraphs",
    "entities",
    "relations",
    "deleted_relations",
    "paragraph_entities",
    "paragraph_relations",
    "external_memory_refs",
    "episodes",
    "episode_paragraphs",
    "fact_claims",
    "fact_evidence",
    "fact_transitions",
    "person_profile_snapshots",
    "person_profile_overrides",
    "person_profile_alias_overrides",
];

struct Group {
    tables: Tables,
    materials: BTreeMap<(String, u64), String>,
    paragraphs: BTreeSet<String>,
}
impl Group {
    fn new() -> Self {
        Self {
            tables: TABLES.iter().map(|v| ((*v).into(), vec![])).collect(),
            materials: BTreeMap::new(),
            paragraphs: BTreeSet::new(),
        }
    }
}
impl Selection {
    /// Complete installation indexes provide candidate memberships, so partial
    /// episode links cannot be mistaken for a closed episode.
    pub(super) fn semantic_rows(&mut self, p: &SubjectPackage) -> Result<()> {
        let mut memberships = BTreeMap::new();
        for layer in &p.imports {
            if layer.installation.namespace_id != p.subject.namespace_id
                || layer.installation.subject_id != p.subject.id
            {
                continue;
            }
            for resource in &layer.resources {
                if resource.kind == "material" {
                    memberships.insert(resource.local_id.clone(), layer.installation.id.clone());
                }
            }
        }
        let mut groups = BTreeMap::<String, Group>::new();
        for material in &p.materials {
            if material.details.source_type.as_deref() != Some("a_memorix_bundle") {
                continue;
            }
            let metadata = &material.details.metadata;
            let Some(installation) = memberships.get(material.material.id.as_str()) else {
                continue;
            };
            let Some(group_id) = dataset(metadata, installation)? else {
                continue;
            };
            let group = groups.entry(group_id).or_insert_with(Group::new);
            if self.has("material", material.material.id.as_str())
                && let Some(id) = metadata.get("external_id").and_then(Value::as_str)
            {
                group.paragraphs.insert(id.into());
            }
            let Some(table) = metadata.get("table").and_then(Value::as_str) else {
                continue;
            };
            let index = metadata.get("semantic_row").ok_or_else(|| {
                Error::invalid("retained semantic row has no authoritative relation index")
            })?;
            if index.get("table").and_then(Value::as_str) != Some(table) {
                return Err(Error::invalid(
                    "semantic row index table differs from its source declaration",
                ));
            }
            let fields: Map<String, Value> = serde_json::from_value(
                index
                    .get("fields")
                    .cloned()
                    .ok_or_else(|| Error::invalid("semantic row index has no relation fields"))?,
            )?;
            let ordinal = fields
                .get("_row")
                .and_then(Value::as_u64)
                .ok_or_else(|| Error::invalid("semantic row index has no source ordinal"))?;
            let rows = group
                .tables
                .get_mut(table)
                .ok_or_else(|| Error::invalid("unknown semantic row index table"))?;
            if table == "paragraphs"
                && self.has("material", material.material.id.as_str())
                && let Some(id) = fields.get("hash").and_then(Value::as_str)
            {
                group.paragraphs.insert(id.into());
            }
            rows.push(fields);
            group
                .materials
                .insert((table.into(), ordinal), material.material.id.to_string());
        }
        for group in groups.values_mut() {
            if group.paragraphs.is_empty() {
                continue;
            }
            for rows in group.tables.values_mut() {
                rows.sort_by_key(|row| row.get("_row").and_then(Value::as_u64));
            }
            retain_related(&mut group.tables, &group.paragraphs);
            for (table, rows) in &group.tables {
                for row in rows {
                    let ordinal = row.get("_row").and_then(Value::as_u64).unwrap();
                    if let Some(material) = group.materials.get(&(table.clone(), ordinal)) {
                        self.add("material", material);
                        for event in &p.observations {
                            if event.material_id.as_str() == material {
                                self.add("observation", event.id.as_str());
                            }
                        }
                    }
                }
            }
        }
        for original in &p.materials {
            let Some(installation) = memberships.get(original.material.id.as_str()) else {
                continue;
            };
            let Some(group_id) = dataset(&original.details.metadata, installation)? else {
                continue;
            };
            let Some(group) = groups.get(&group_id) else {
                continue;
            };
            if original
                .details
                .metadata
                .get("external_id")
                .and_then(Value::as_str)
                .is_some_and(|id| group.paragraphs.contains(id))
            {
                self.add("material", original.material.id.as_str());
                for event in &p.observations {
                    if event.material_id == original.material.id {
                        self.add("observation", event.id.as_str());
                    }
                }
            }
        }
        Ok(())
    }
}

fn dataset(metadata: &Value, installation: &str) -> Result<Option<String>> {
    let Some(digest) = metadata
        .get("semantic_source_digest")
        .and_then(Value::as_str)
    else {
        if metadata.get("table").is_some() {
            return Err(Error::invalid("semantic row has no admitted source digest"));
        }
        return Ok(None);
    };
    let source = metadata
        .get("semantic_source_subject")
        .and_then(Value::as_str)
        .ok_or_else(|| Error::invalid("semantic row has no admitted source subject"))?;
    Ok(Some(format!("{installation}/{digest}/{source}")))
}
