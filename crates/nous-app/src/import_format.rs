//! Bounded text import decoding before any authority writes.
//!

use nous_domain::{
    Error, ErrorCode, ImportFormat, ImportRecord, ParsedImportRecord, Result, validate_name,
};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

pub const MAX_IMPORT_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_IMPORT_RECORDS: usize = 1000;
pub const MAX_RECORD_BYTES: usize = 1024 * 1024;

/// Decode the complete bounded input first. Record failures remain in their original order.
pub fn parse(
    content: &str,
    format: ImportFormat,
    default_source: Option<&str>,
) -> Result<Vec<ParsedImportRecord>> {
    if content.len() > MAX_IMPORT_BYTES {
        return Err(exhausted("import exceeds 4 MiB"));
    }
    if content.trim().is_empty() {
        return Err(Error::invalid("import content is empty"));
    }
    if content.starts_with("PK\u{3}\u{4}") {
        return Err(Error::new(
            ErrorCode::CapabilityUnavailable,
            ".amembundle archives require the memory-package importer",
        ));
    }
    match format {
        ImportFormat::Text => Ok(vec![item(
            0,
            None,
            (|| {
                let mut record = ImportRecord {
                    external_id: String::new(),
                    text: content.to_owned(),
                    source: default_source.map(str::to_owned),
                    source_type: Some("text".into()),
                    event_time: None,
                    observed_at: None,
                    origin: None,
                    tags: Vec::new(),
                    metadata: json!({}),
                    structured: None,
                };
                record.external_id =
                    stable_id(&json!({"format":"text","source":record.source,"text":record.text}))?;
                validate(record)
            })(),
        )]),
        ImportFormat::Jsonl => {
            let lines: Vec<_> = content
                .lines()
                .enumerate()
                .filter(|(_, line)| !line.trim().is_empty())
                .collect();
            check_count(lines.len())?;
            collect_items(
                lines
                    .into_iter()
                    .enumerate()
                    .map(|(index, (line_number, line))| {
                        if line.len() > MAX_RECORD_BYTES {
                            return item(index, None, Err(exhausted("JSONL record exceeds 1 MiB")));
                        }
                        match serde_json::from_str::<Value>(line) {
                            Ok(value) => {
                                let external_id = external_id(&value);
                                item(index, external_id, native(value, default_source))
                            }
                            Err(error) => item(
                                index,
                                None,
                                Err(Error::invalid(format!(
                                    "JSONL line {}: {error}",
                                    line_number + 1
                                ))),
                            ),
                        }
                    }),
            )
        }
        ImportFormat::Json => parse_json(content, default_source),
    }
}

fn parse_json(content: &str, default_source: Option<&str>) -> Result<Vec<ParsedImportRecord>> {
    let value: Value = serde_json::from_str(content)
        .map_err(|error| Error::invalid(format!("invalid import JSON: {error}")))?;
    let (units, kind, context) = match value {
        Value::Array(units) => (units, "native", Value::Null),
        Value::Object(mut envelope) => {
            if envelope.get("format").and_then(Value::as_str) == Some("a-memorix-memory-bundle") {
                return Err(Error::new(
                    ErrorCode::CapabilityUnavailable,
                    "memory packages require their manifest, checksums and installation contract",
                ));
            }
            let present: Vec<_> = ["records", "paragraphs", "docs"]
                .into_iter()
                .filter(|key| envelope.contains_key(*key))
                .collect();
            if present.len() != 1 {
                return Err(Error::invalid(
                    "import JSON requires exactly one records, paragraphs or docs array",
                ));
            }
            let key = present[0];
            let units = envelope
                .remove(key)
                .and_then(|v| v.as_array().cloned())
                .ok_or_else(|| Error::invalid("import records must be an array"))?;
            if key == "records" && !envelope.is_empty() {
                return Err(Error::invalid(
                    "native records envelope accepts only records",
                ));
            }
            (
                units,
                match key {
                    "records" => "native",
                    "paragraphs" => "a_memorix_paragraph",
                    _ => "lpmm_openie",
                },
                Value::Object(envelope),
            )
        }
        _ => {
            return Err(Error::invalid(
                "import JSON must be a record array or records/paragraphs/docs object",
            ));
        }
    };
    check_count(units.len())?;
    collect_items(units.into_iter().enumerate().map(|(index, value)| {
        let external_id = external_id(&value);
        let result = if kind == "native" {
            native(value, default_source)
        } else {
            mature(value, kind, &context, default_source)
        };
        item(index, external_id, result)
    }))
}

fn collect_items(
    items: impl Iterator<Item = ParsedImportRecord>,
) -> Result<Vec<ParsedImportRecord>> {
    let mut output = Vec::new();
    let mut normalized_bytes = 0usize;
    for item in items {
        normalized_bytes = normalized_bytes.saturating_add(serde_json::to_vec(&item)?.len());
        if normalized_bytes > MAX_IMPORT_BYTES {
            return Err(exhausted("normalized import records exceed 4 MiB"));
        }
        output.push(item);
    }
    Ok(output)
}

fn native(value: Value, default_source: Option<&str>) -> Result<ImportRecord> {
    let bytes = serde_json::to_vec(&value).map_err(Error::from)?;
    if bytes.len() > MAX_RECORD_BYTES {
        return Err(exhausted("import record exceeds 1 MiB"));
    }
    let object = value.as_object().ok_or_else(|| {
        Error::invalid("native import record must be an object with external_id and text")
    })?;
    let fields = [
        "external_id",
        "text",
        "source",
        "source_type",
        "event_time",
        "observed_at",
        "origin",
        "tags",
        "metadata",
        "structured",
    ];
    if let Some(key) = object.keys().find(|key| !fields.contains(&key.as_str())) {
        return Err(Error::invalid(format!(
            "unknown native import record field: {key}"
        )));
    }
    let mut record: ImportRecord = serde_json::from_value(value)
        .map_err(|error| Error::invalid(format!("invalid import record: {error}")))?;
    if record.source.is_none() {
        record.source = default_source.map(str::to_owned);
    }
    validate(record)
}

fn mature(
    unit: Value,
    format: &str,
    context: &Value,
    default_source: Option<&str>,
) -> Result<ImportRecord> {
    let text = if format == "a_memorix_paragraph" && unit.is_string() {
        unit.as_str()
    } else {
        unit.get(if format == "lpmm_openie" {
            "passage"
        } else {
            "content"
        })
        .and_then(Value::as_str)
    }
    .ok_or_else(|| Error::invalid("paragraph content or LPMM passage must be a string"))?
    .to_owned();
    let source = optional_string(&unit, "source")?.or_else(|| default_source.map(str::to_owned));
    let source_type = optional_string(&unit, "source_type")?.or_else(|| Some(format.to_owned()));
    let structured = json!({"format":format,"payload":unit,"document_context":context});
    if serde_json::to_vec(&structured)?.len() > MAX_RECORD_BYTES {
        return Err(exhausted("structured import record exceeds 1 MiB"));
    }
    let external_id = match unit.get("external_id") {
        Some(Value::String(id)) => id.clone(),
        Some(Value::Null) | None => stable_id(&json!({"source":source,"structured":structured}))?,
        _ => return Err(Error::invalid("external_id must be a string")),
    };
    let metadata = unit.get("metadata").cloned().unwrap_or_else(|| json!({}));
    let tags = unit
        .get("tags")
        .map(|tags| {
            serde_json::from_value(tags.clone())
                .map_err(|_| Error::invalid("tags must be an array of strings"))
        })
        .transpose()?
        .unwrap_or_default();
    validate(ImportRecord {
        external_id,
        text,
        source,
        source_type,
        event_time: None,
        observed_at: None,
        origin: None,
        tags,
        metadata,
        structured: Some(structured),
    })
}

fn optional_string(value: &Value, key: &str) -> Result<Option<String>> {
    match value.get(key) {
        Some(Value::String(value)) => Ok(Some(value.clone())),
        Some(Value::Null) | None => Ok(None),
        _ => Err(Error::invalid(format!("{key} must be a string"))),
    }
}
fn validate(mut record: ImportRecord) -> Result<ImportRecord> {
    if let Some(origin) = &record.origin {
        origin.validate()?;
    }
    validate_name(&record.external_id, "external_id")?;
    if record.text.trim().is_empty() {
        return Err(Error::invalid("import text is empty"));
    }
    if record.text.len() > MAX_RECORD_BYTES {
        return Err(exhausted("import text exceeds 1 MiB"));
    }
    if !record.metadata.is_object() {
        return Err(Error::invalid("metadata must be an object"));
    }
    if record
        .source
        .as_ref()
        .is_some_and(|source| source.len() > 4096)
    {
        return Err(Error::invalid("source exceeds 4096 bytes"));
    }
    if let Some(kind) = &record.source_type {
        validate_name(kind, "source_type")?;
    }
    if let Some(time) = &record.event_time {
        time.validate()?;
    }
    if let Some(time) = &record.observed_at {
        time.validate()?;
    }
    record.metadata = canonical(&record.metadata);
    record.structured = record.structured.as_ref().map(canonical);
    Ok(record)
}
fn item(
    index: usize,
    external_id: Option<String>,
    result: Result<ImportRecord>,
) -> ParsedImportRecord {
    match result {
        Ok(record) => ParsedImportRecord {
            index,
            external_id: Some(record.external_id.clone()),
            record: Some(record),
            error: None,
        },
        Err(error) => ParsedImportRecord {
            index,
            external_id,
            record: None,
            error: Some(error),
        },
    }
}
fn external_id(value: &Value) -> Option<String> {
    value
        .get("external_id")
        .and_then(Value::as_str)
        .map(str::to_owned)
}
fn check_count(count: usize) -> Result<()> {
    if count == 0 {
        return Err(Error::invalid("import contains no records"));
    }
    if count > MAX_IMPORT_RECORDS {
        return Err(exhausted("import exceeds 1000 records"));
    }
    Ok(())
}
fn exhausted(message: &str) -> Error {
    Error::new(ErrorCode::ResourceExhausted, message)
}
fn stable_id(value: &Value) -> Result<String> {
    Ok(format!(
        "sha256:{:x}",
        Sha256::digest(serde_json::to_vec(&canonical(value))?)
    ))
}
fn canonical(value: &Value) -> Value {
    match value {
        Value::Object(fields) => {
            let mut keys: Vec<_> = fields.keys().collect();
            keys.sort();
            Value::Object(
                keys.into_iter()
                    .map(|key| (key.clone(), canonical(&fields[key])))
                    .collect::<Map<_, _>>(),
            )
        }
        Value::Array(items) => Value::Array(items.iter().map(canonical).collect()),
        other => other.clone(),
    }
}
