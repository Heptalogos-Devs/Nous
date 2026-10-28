use super::{Member, read_declared};
use crate::bundle_format::canonical_json;
use anyhow::{Context, ensure};
use nous_domain::{BundleContentLevel, SubjectPackage};
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::Path};

pub(super) fn project(
    root: &Path,
    package: &SubjectPackage,
    members: &mut BTreeMap<String, Member>,
) -> anyhow::Result<Value> {
    if package.content_level != BundleContentLevel::Full {
        return Ok(json!({"status":"excluded_from_knowledge"}));
    }
    let mut records: BTreeMap<(String, String), Value> = BTreeMap::new();
    for material in &package.materials {
        if material.details.source_type.as_deref() != Some("nous_portable_semantic_projection") {
            continue;
        }
        ensure!(
            material.details.metadata["member"].as_str() == Some("nous-semantic.json"),
            "unexpected semantic projection member"
        );
        let document: Value = serde_json::from_slice(&read_declared(root, material)?)?;
        for record in document["records"]
            .as_array()
            .context("semantic source lacks records")?
        {
            insert(&mut records, record)?;
        }
    }
    let mut current = BTreeMap::new();
    for object in &package.cognition {
        let previous: Option<&&nous_domain::StructuredObject> = current.get(&object.id);
        if previous.is_none_or(|old| old.revision < object.revision) {
            current.insert(object.id.clone(), object);
        }
    }
    for object in current.values() {
        if object.deleted {
            let kind = serde_json::to_value(object.body.kind())?;
            records.remove(&(
                kind.as_str().context("invalid cognitive kind")?.into(),
                object.id.to_string(),
            ));
            continue;
        }
        insert(
            &mut records,
            &json!({"id":object.id,"kind":object.body.kind(),"revision":object.revision,"body":object.body,"times":object.times}),
        )?;
    }
    if records.is_empty() {
        return Ok(json!({"status":"no_selected_semantic_records","record_count":0}));
    }
    let count = records.len();
    members.insert(
        "nous-semantic.json".into(),
        Member::Bytes(canonical_json(
            &json!({"records":records.into_values().collect::<Vec<_>>()}),
        )?),
    );
    Ok(
        json!({"status":"preserved_as_nous_semantic_sources","member":"nous-semantic.json","record_count":count,"adoption":"not_an_a_memorix_standard_table_or_automatically_adopted_cognition"}),
    )
}
fn insert(records: &mut BTreeMap<(String, String), Value>, source: &Value) -> anyhow::Result<()> {
    let id = source["id"]
        .as_str()
        .context("semantic source lacks stable id")?;
    let kind = source["kind"]
        .as_str()
        .context("semantic source lacks kind")?;
    let revision = source["revision"]
        .as_i64()
        .context("semantic source lacks revision")?;
    ensure!(revision > 0, "invalid semantic source revision");
    // Deserialize only the selected semantic payload, excluding arbitrary extension fields.
    let body: nous_domain::CognitiveContent = serde_json::from_value(source["body"].clone())?;
    let times: nous_domain::CognitiveTimes = serde_json::from_value(source["times"].clone())?;
    let key = (kind.into(), id.into());
    if records
        .get(&key)
        .is_none_or(|old| old["revision"].as_i64().is_none_or(|v| v < revision))
    {
        records.insert(
            key,
            json!({"id":id,"kind":kind,"revision":revision,"body":body,"times":times}),
        );
    }
    Ok(())
}
