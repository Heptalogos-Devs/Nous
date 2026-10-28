use crate::bundle_format::{STATE_TABLES, canonical_json};
use anyhow::Context;
use nous_domain::{Error, ErrorCode, media::MediaOriginal};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;

#[derive(Default)]
pub(super) struct Sources {
    rows: BTreeMap<(String, Vec<u8>), SourceRow>,
}
struct SourceRow {
    bytes: Vec<u8>,
    source: Value,
}
impl Sources {
    // Only raw source rows pass this gate. Generated current Native projections use
    // the projector's explicit upsert priority after all source rows are admitted.
    pub(super) fn admit(
        &mut self,
        table: &str,
        row: &Map<String, Value>,
        material: &MediaOriginal,
    ) -> anyhow::Result<bool> {
        let key = row_key(table, row)?;
        let identity = (table.to_owned(), canonical_json(&key)?);
        let actual = canonical_json(&Value::Object(row.clone()))?;
        let source = json!({
            "digest": material.details.metadata.get("semantic_source_digest"),
            "subject": material.details.metadata.get("semantic_source_subject"),
            "row_digest": material.material.content_hash,
        });
        if let Some(previous) = self.rows.get(&identity) {
            if previous.bytes == actual {
                return Ok(false);
            }
            return Err(Error::new(
                ErrorCode::Conflict,
                "portable semantic sources contain conflicting rows",
            )
            .with_context("table", table)
            .with_context("key", key)
            .with_context("sources", json!([previous.source, source]))
            .into());
        }
        self.rows.insert(
            identity,
            SourceRow {
                bytes: actual,
                source,
            },
        );
        Ok(true)
    }
}
pub(super) fn row_key(table: &str, row: &Map<String, Value>) -> anyhow::Result<Value> {
    let keys = STATE_TABLES
        .iter()
        .find(|(name, _)| *name == table)
        .context("unsupported semantic table")?
        .1;
    Ok(Value::Array(
        keys.iter()
            .map(|key| row.get(*key).cloned().context("semantic row lacks key"))
            .collect::<anyhow::Result<_>>()?,
    ))
}
