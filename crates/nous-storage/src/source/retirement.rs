// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
// Adapted from Nous 412b062082afb75ccfae4c3f69fc606d3e3cc862; see NOTICE.
use crate::StructuredGroupState;
use crate::codec::*;
use crate::{WriteTransaction, storage_error, write_error};
use nous_domain::*;
use serde_json::Value;
use sqlx::Row;
use std::collections::HashSet;

impl WriteTransaction {
    /// Retire all captured direct/transitive Memory inputs before deleting the
    /// source revisions. Accepted typed objects remain independent authorities.
    pub(crate) async fn purge_memory_structured_formations(
        &mut self,
        namespace: &NamespaceId,
        subject: &SubjectId,
        memory: &MemoryId,
    ) -> Result<()> {
        let affected=sqlx::query("WITH RECURSIVE affected(kind,id,revision) AS (SELECT 'memory',memory_id,revision FROM memory_revisions WHERE namespace_id=? AND subject_id=? AND memory_id=? UNION SELECT r.kind,l.object_id,l.revision FROM structured_links l JOIN structured_revisions r ON r.object_id=l.object_id AND r.revision=l.revision JOIN affected a ON a.kind=l.source_kind AND a.id=l.source_id AND a.revision=l.source_revision WHERE l.namespace_id=? AND l.subject_id=?) SELECT kind,id,revision FROM affected")
            .bind(namespace.as_str()).bind(subject.as_str()).bind(memory.as_str()).bind(namespace.as_str()).bind(subject.as_str()).fetch_all(&mut *self.tx).await.map_err(storage_error)?
            .into_iter().map(|row|(row.get::<String,_>("kind"),row.get::<String,_>("id"),row.get::<i64,_>("revision"))).collect::<HashSet<_>>();
        let rows=sqlx::query("SELECT r.job_id,r.input FROM structured_formation_runs r JOIN task_records j ON j.job_id=r.job_id WHERE j.namespace_id=? AND j.subject_id=?")
            .bind(namespace.as_str()).bind(subject.as_str()).fetch_all(&mut *self.tx).await.map_err(storage_error)?;
        let error = Error::new(
            ErrorCode::NotFound,
            "structured formation source was permanently purged; captured payload retired",
        );
        for row in rows {
            let input: Value = serde_json::from_str(row.get("input"))?;
            if !references_affected(&input, memory, &affected) {
                continue;
            }
            let job: String = row.get("job_id");
            let redacted = serde_json::json!({"redacted":true,"reason":"memory_purged"});
            self.redact_job_calls(&job, "memory_purged").await?;
            let mut retired_input = redacted.clone();
            retired_input["namespace_id"] = serde_json::json!(namespace);
            retired_input["subject_id"] = serde_json::json!(subject);
            retired_input["purged_memory_id"] = serde_json::json!(memory);
            if let Some(cutoff) = input.get("knowledge_cutoff").and_then(Value::as_i64) {
                retired_input["knowledge_cutoff"] = serde_json::json!(cutoff);
            }
            retired_input["sources"] = Value::Array(
                input
                    .get("sources")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|entry| {
                        serde_json::from_value::<EvidenceSource>(
                            entry.get("source").unwrap_or(entry).clone(),
                        )
                        .ok()
                    })
                    .map(|source| serde_json::to_value(source).expect("typed source serialization"))
                    .collect(),
            );
            sqlx::query("UPDATE task_records SET state=CASE WHEN state IN('queued','waiting','running') THEN 'cancelled' ELSE state END,wait=NULL,error=?,updated_at=strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE job_id=?").bind(serde_json::to_string(&error)?).bind(&job).execute(&mut *self.tx).await.map_err(write_error)?;
            sqlx::query("UPDATE structured_formation_runs SET input=? WHERE job_id=?")
                .bind(serde_json::to_string(&retired_input)?)
                .bind(&job)
                .execute(&mut *self.tx)
                .await
                .map_err(write_error)?;
            let existing = self.structured_formation_groups(&job).await?;
            for group in existing {
                let result = group.result.map(|result| {
                    let mut metadata = redacted.clone();
                    if let Some(objects) = result.get("objects").and_then(Value::as_array) {
                        metadata["objects"] = Value::Array(
                            objects
                                .iter()
                                .map(|object| {
                                    let mut identity =
                                        retain_fields(object, &["id", "revision", "commit_seq"]);
                                    if let Some(kind) = object.pointer("/body/kind") {
                                        identity.insert("kind".into(), kind.clone());
                                    }
                                    Value::Object(identity)
                                })
                                .collect(),
                        );
                    }
                    metadata
                });
                let pending = group.state == StructuredGroupState::Pending;
                sqlx::query("UPDATE structured_formation_groups SET state=?,result=?,error=?,commit_seq=? WHERE job_id=? AND key=?")
                    .bind(enum_name(&if pending{StructuredGroupState::Skipped}else{group.state})?).bind(json_option(&result)?).bind(if pending || group.error.is_some(){Some(serde_json::to_string(&error)?)}else{None})
                    .bind(if pending{Some(self.commit_seq())}else{group.commit_seq}).bind(&job).bind(&group.key).execute(&mut *self.tx).await.map_err(write_error)?;
            }
            let receipt = serde_json::json!({"job_id":job,"retired":true,"redacted":true,"reason":"memory_purged"});
            sqlx::query("UPDATE operation_receipts SET receipt=json_set(receipt,'$.result',json(?)) WHERE scope=? OR json_extract(receipt,'$.result.job_id')=? OR EXISTS(SELECT 1 FROM json_each(receipt,'$.job_ids') WHERE value=?)")
                .bind(serde_json::to_string(&receipt)?).bind(&job).bind(&job).bind(&job).execute(&mut *self.tx).await.map_err(write_error)?;
            sqlx::query("UPDATE Jobs SET last_result=NULL WHERE id=?")
                .bind(&job)
                .execute(&mut *self.tx)
                .await
                .map_err(write_error)?;
        }
        Ok(())
    }
}

fn retain_fields(value: &Value, keys: &[&str]) -> serde_json::Map<String, Value> {
    keys.iter()
        .filter_map(|key| {
            value
                .get(*key)
                .map(|value| ((*key).to_owned(), value.clone()))
        })
        .collect()
}
fn references_affected(
    value: &Value,
    memory: &MemoryId,
    affected: &HashSet<(String, String, i64)>,
) -> bool {
    match value {
        Value::Object(fields) => {
            if fields.get("memory_id").and_then(Value::as_str) == Some(memory.as_str()) {
                return true;
            }
            if let (Some(kind), Some(id), Some(revision)) = (
                fields.get("kind").and_then(Value::as_str),
                fields.get("id").and_then(Value::as_str),
                fields
                    .get("revision")
                    .or_else(|| fields.get("expected_revision"))
                    .and_then(Value::as_i64),
            ) && affected.contains(&(kind.to_owned(), id.to_owned(), revision))
            {
                return true;
            }
            fields
                .values()
                .any(|value| references_affected(value, memory, affected))
        }
        Value::Array(values) => values
            .iter()
            .any(|value| references_affected(value, memory, affected)),
        _ => false,
    }
}
