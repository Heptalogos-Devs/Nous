use super::{MAX_GROUPS, RawStructuredProposal, StructuredFormationInput};
use anyhow::Result as ConfigResult;
use nous_domain as d;
use serde_json::{Value, json};
use std::collections::BTreeSet;

/// Build the actual provider contract from the typed DTOs, constraining every
/// reference namespace to this invocation and every change to its assigned kind.
pub fn structured_output_schema(input: &StructuredFormationInput) -> ConfigResult<Value> {
    input
        .validate()
        .map_err(|error| anyhow::anyhow!(error.to_string()))?;
    let mut schema = serde_json::to_value(schemars::schema_for!(RawStructuredProposal))?;
    let defs = schema
        .get_mut("$defs")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| anyhow::anyhow!("structured schema has no definitions"))?;
    let source_keys: Vec<_> = input
        .sources
        .iter()
        .map(|source| source.key.clone())
        .chain(input.slots.iter().map(|slot| slot.key.clone()))
        .collect();
    let reference_keys: Vec<_> = input
        .sources
        .iter()
        .filter(|source| source.source.cognitive_ref().is_some())
        .map(|source| source.key.clone())
        .chain(input.slots.iter().map(|slot| slot.key.clone()))
        .collect();
    let memory_keys: Vec<_> = input
        .sources
        .iter()
        .filter(|source| matches!(source.source, d::EvidenceSource::MemoryRevision { .. }))
        .map(|source| source.key.clone())
        .collect();
    let observation_keys: Vec<_> = input
        .sources
        .iter()
        .filter(|source| matches!(source.source, d::EvidenceSource::Observation { .. }))
        .map(|source| source.key.clone())
        .collect();
    for (name, values) in [
        ("SourceKey", source_keys),
        ("ReferenceKey", reference_keys),
        ("MemoryKey", memory_keys),
        ("ObservationKey", observation_keys.clone()),
    ] {
        if !values.is_empty() {
            defs.insert(name.into(), json!({"type":"string","enum":values}));
        }
    }
    if observation_keys.is_empty()
        && let Some(memory) = defs.get_mut("RawMemorySemantics")
    {
        memory["properties"]["grounding_observation"] = json!({"type":"null"});
    }
    let polarities: Vec<d::EvidencePolarity> =
        serde_json::from_value(defs["EvidencePolarity"]["enum"].clone())?;
    let relations: Vec<d::EpistemicRelation> =
        serde_json::from_value(defs["EpistemicRelation"]["enum"].clone())?;
    let evidence = defs
        .get("RawEvidenceLink")
        .ok_or_else(|| anyhow::anyhow!("evidence schema is missing"))?
        .clone();
    let evidence_choices = polarities
        .into_iter()
        .map(|polarity| {
            let mut choice = evidence.clone();
            choice["properties"]["polarity"] = json!({"type":"string","const":polarity});
            let allowed = relations
                .iter()
                .filter(|relation| polarity.accepts(Some(**relation)))
                .collect::<Vec<_>>();
            choice["properties"]["relation"] = if allowed.is_empty() {
                json!({"type":"null"})
            } else {
                json!({"anyOf":[{"type":"null"},{"type":"string","enum":allowed}]})
            };
            choice
        })
        .collect::<Vec<_>>();
    defs.insert("RawEvidenceLink".into(), json!({"anyOf":evidence_choices}));
    let variants = defs
        .get("RawCognitiveContent")
        .and_then(|body| body.get("oneOf").or_else(|| body.get("anyOf")))
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("structured body schema has no typed alternatives"))?
        .clone();
    let change = defs
        .get("RawProposalChange")
        .ok_or_else(|| anyhow::anyhow!("structured change schema is missing"))?
        .clone();
    let mut changes = serde_json::Map::new();
    for slot in &input.slots {
        let kind = serde_json::to_value(slot.kind)?;
        let body = variants
            .iter()
            .find(|variant| {
                variant["properties"]["kind"]["const"] == kind
                    || variant["properties"]["kind"]["enum"]
                        .as_array()
                        .is_some_and(|names| names.len() == 1 && names[0] == kind)
            })
            .ok_or_else(|| anyhow::anyhow!("target slot has no typed output schema"))?;
        let mut choice = change.clone();
        choice["properties"]["body"] = body.clone();
        choice["properties"]["evidence"]["minItems"] = json!(1);
        choice["properties"]["evidence"]["maxItems"] = json!(32);
        changes.insert(slot.key.clone(), json!({"anyOf":[choice,{"type":"null"}]}));
    }
    if let Some(group) = defs.get_mut("RawProposalGroup") {
        group["properties"]["depends_on"]["maxItems"] = json!(MAX_GROUPS - 1);
    }
    schema["properties"]["changes"] = json!({
        "type":"object", "additionalProperties":false,
        "required":input.slots.iter().map(|slot| &slot.key).collect::<Vec<_>>(),
        "properties":changes,
    });
    schema["properties"]["groups"]["maxItems"] = json!(MAX_GROUPS);
    prune_definitions(&mut schema)?;
    Ok(schema)
}

fn prune_definitions(schema: &mut Value) -> ConfigResult<()> {
    fn references(value: &Value, found: &mut BTreeSet<String>) {
        match value {
            Value::Object(fields) => {
                if let Some(name) = fields
                    .get("$ref")
                    .and_then(Value::as_str)
                    .and_then(|reference| reference.strip_prefix("#/$defs/"))
                {
                    found.insert(name.into());
                }
                for child in fields.values() {
                    references(child, found);
                }
            }
            Value::Array(values) => {
                for child in values {
                    references(child, found);
                }
            }
            _ => {}
        }
    }
    let defs = schema
        .as_object_mut()
        .and_then(|root| root.remove("$defs"))
        .and_then(|value| value.as_object().cloned())
        .ok_or_else(|| anyhow::anyhow!("structured definitions are missing"))?;
    let mut pending = BTreeSet::new();
    references(schema, &mut pending);
    let mut retained = serde_json::Map::new();
    while let Some(name) = pending.pop_first() {
        if retained.contains_key(&name) {
            continue;
        }
        let definition = defs
            .get(&name)
            .ok_or_else(|| anyhow::anyhow!("structured schema references an undefined type"))?
            .clone();
        references(&definition, &mut pending);
        retained.insert(name, definition);
    }
    schema["$defs"] = Value::Object(retained);
    Ok(())
}
