use serde_json::Map;
pub type Tables = std::collections::BTreeMap<String, Vec<Map<String, Value>>>;
use serde_json::Value;
use std::collections::BTreeSet;

// The same semantic closure as A-Memorix _collect_related_rows, applied to admitted row materials.
pub fn retain_related(tables: &mut Tables, selected: &BTreeSet<String>) {
    tables
        .get_mut("paragraphs")
        .unwrap()
        .retain(|r| selected.contains(&text(r, "hash")));
    let paragraphs = ids(tables, "paragraphs", "hash");
    for table in [
        "paragraph_entities",
        "paragraph_relations",
        "external_memory_refs",
        "deleted_relations",
    ] {
        let key = if table == "deleted_relations" {
            "source_paragraph"
        } else {
            "paragraph_hash"
        };
        tables
            .get_mut(table)
            .unwrap()
            .retain(|r| paragraphs.contains(&text(r, key)));
    }
    let relation_ids = ids(tables, "paragraph_relations", "relation_hash");
    tables
        .get_mut("relations")
        .unwrap()
        .retain(|r| relation_ids.contains(&text(r, "hash")));
    let entity_ids = ids(tables, "paragraph_entities", "entity_hash");
    let endpoints: BTreeSet<_> = tables["relations"]
        .iter()
        .flat_map(|r| [text(r, "subject"), text(r, "object")])
        .map(|s| s.trim().to_lowercase())
        .collect();
    tables.get_mut("entities").unwrap().retain(|r| {
        entity_ids.contains(&text(r, "hash"))
            || endpoints.contains(&text(r, "name").trim().to_lowercase())
    });
    let episode_ids: BTreeSet<_> = ids(tables, "episode_paragraphs", "episode_id")
        .into_iter()
        .filter(|id| {
            let linked: BTreeSet<_> = tables["episode_paragraphs"]
                .iter()
                .filter(|r| text(r, "episode_id") == *id)
                .map(|r| text(r, "paragraph_hash"))
                .collect();
            !linked.is_empty() && linked.is_subset(&paragraphs)
        })
        .collect();
    tables
        .get_mut("episodes")
        .unwrap()
        .retain(|r| episode_ids.contains(&text(r, "episode_id")));
    tables
        .get_mut("episode_paragraphs")
        .unwrap()
        .retain(|r| episode_ids.contains(&text(r, "episode_id")));
    let mut semantic_ids = paragraphs.clone();
    semantic_ids.extend(ids(tables, "entities", "hash"));
    semantic_ids.extend(ids(tables, "relations", "hash"));
    semantic_ids.extend(episode_ids);
    let mut claims: BTreeSet<_> = tables["fact_evidence"]
        .iter()
        .filter(|r| semantic_ids.contains(&text(r, "evidence_id")))
        .map(|r| text(r, "claim_id"))
        .collect();
    let mut persons = BTreeSet::new();
    let mut chats = BTreeSet::new();
    for paragraph in &tables["paragraphs"] {
        let metadata = object_json(paragraph.get("metadata"));
        for key in ["person_id", "person_ids"] {
            tokens(&metadata[key], &mut persons);
        }
        for key in [
            "chat_id",
            "chat_ids",
            "session_id",
            "session_ids",
            "stream_id",
            "stream_ids",
        ] {
            tokens(&metadata[key], &mut chats);
        }
        if let Some(chat) = text(paragraph, "source").strip_prefix("chat_summary:") {
            tokens(&Value::String(chat.into()), &mut chats);
        }
    }
    for row in &tables["fact_claims"] {
        if match text(row, "scope_type").as_str() {
            "person" => persons.contains(&text(row, "scope_id")),
            "chat" => chats.contains(&text(row, "scope_id")),
            _ => false,
        } {
            claims.insert(text(row, "claim_id"));
        }
    }
    let all_claims = ids(tables, "fact_claims", "claim_id");
    tables
        .get_mut("person_profile_snapshots")
        .unwrap()
        .retain(|row| {
            let evidence = encoded_tokens(row.get("evidence_ids_json"));
            let facts = encoded_tokens(row.get("fact_claim_ids_json"));
            let person = text(row, "person_id");
            let keep = evidence.is_subset(&semantic_ids)
                && facts.is_subset(&all_claims)
                && (persons.contains(&person)
                    || !evidence.is_disjoint(&semantic_ids)
                    || !facts.is_disjoint(&claims));
            if keep {
                persons.insert(person);
                claims.extend(facts);
            }
            keep
        });
    for table in ["fact_claims", "fact_evidence"] {
        tables
            .get_mut(table)
            .unwrap()
            .retain(|r| claims.contains(&text(r, "claim_id")));
    }
    tables.get_mut("fact_transitions").unwrap().retain(|r| {
        claims.contains(&text(r, "old_claim_id")) || claims.contains(&text(r, "new_claim_id"))
    });
    for table in ["person_profile_overrides", "person_profile_alias_overrides"] {
        tables
            .get_mut(table)
            .unwrap()
            .retain(|r| persons.contains(&text(r, "person_id")));
    }
}
fn ids(tables: &Tables, table: &str, field: &str) -> BTreeSet<String> {
    tables[table]
        .iter()
        .map(|r| text(r, field))
        .filter(|s| !s.is_empty())
        .collect()
}
fn encoded_tokens(value: Option<&Value>) -> BTreeSet<String> {
    let mut result = BTreeSet::new();
    if let Some(value) = value {
        let value = value
            .as_str()
            .and_then(|s| serde_json::from_str::<Value>(s).ok())
            .unwrap_or_else(|| value.clone());
        tokens(&value, &mut result);
    }
    result
}
fn tokens(value: &Value, result: &mut BTreeSet<String>) {
    match value {
        Value::String(s) if !s.trim().is_empty() => {
            result.insert(s.trim().into());
        }
        Value::Array(v) => {
            for value in v {
                tokens(value, result);
            }
        }
        Value::Object(v) => {
            for value in v.values() {
                tokens(value, result);
            }
        }
        _ => {}
    }
}

fn text(row: &Map<String, Value>, field: &str) -> String {
    row.get(field)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}
fn object_json(value: Option<&Value>) -> Value {
    value
        .and_then(|v| {
            if let Some(text) = v.as_str() {
                serde_json::from_str(text).ok()
            } else {
                Some(v.clone())
            }
        })
        .filter(Value::is_object)
        .unwrap_or_else(|| serde_json::json!({}))
}
/// Relational fields are indexed alongside the CAS reference; the original row is not duplicated.
pub fn index(table: &str, row: &Map<String, Value>, ordinal: usize) -> Value {
    let (primary, fields): (&[&str], &[&str]) = match table {
        "paragraphs" => (&["hash"], &["hash", "source"]),
        "entities" => (&["hash"], &["hash", "name"]),
        "relations" => (&["hash"], &["hash", "subject", "object"]),
        "deleted_relations" => (&["hash"], &["hash", "source_paragraph"]),
        "paragraph_entities" => (
            &["paragraph_hash", "entity_hash"],
            &["paragraph_hash", "entity_hash"],
        ),
        "paragraph_relations" => (
            &["paragraph_hash", "relation_hash"],
            &["paragraph_hash", "relation_hash"],
        ),
        "external_memory_refs" => (&["external_id"], &["external_id", "paragraph_hash"]),
        "episodes" => (&["episode_id"], &["episode_id"]),
        "episode_paragraphs" => (
            &["episode_id", "paragraph_hash"],
            &["episode_id", "paragraph_hash"],
        ),
        "fact_claims" => (&["claim_id"], &["claim_id", "scope_type", "scope_id"]),
        "fact_evidence" => (
            &["claim_id", "evidence_type", "evidence_id", "stance"],
            &["claim_id", "evidence_type", "evidence_id", "stance"],
        ),
        "fact_transitions" => (
            &["transition_id"],
            &["transition_id", "old_claim_id", "new_claim_id"],
        ),
        "person_profile_snapshots" => (
            &["snapshot_id"],
            &[
                "snapshot_id",
                "person_id",
                "evidence_ids_json",
                "fact_claim_ids_json",
            ],
        ),
        "person_profile_overrides" | "person_profile_alias_overrides" => {
            (&["person_id"], &["person_id"])
        }
        _ => (&[], &[]),
    };
    let primary_key: Map<String, Value> = primary
        .iter()
        .filter_map(|key| row.get(*key).map(|v| ((*key).into(), v.clone())))
        .collect();
    let mut selected: Map<String, Value> = fields
        .iter()
        .filter_map(|key| row.get(*key).map(|v| ((*key).into(), v.clone())))
        .collect();
    if table == "paragraphs" {
        let metadata = object_json(row.get("metadata"));
        let metadata: Map<String, Value> = [
            "person_id",
            "person_ids",
            "chat_id",
            "chat_ids",
            "session_id",
            "session_ids",
            "stream_id",
            "stream_ids",
        ]
        .iter()
        .filter_map(|key| metadata.get(*key).map(|v| ((*key).into(), v.clone())))
        .collect();
        selected.insert("metadata".into(), Value::Object(metadata));
    }
    selected.insert("_row".into(), serde_json::json!(ordinal));
    serde_json::json!({"table":table,"primary_key":primary_key,"fields":selected})
}
