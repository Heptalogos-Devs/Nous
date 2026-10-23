//! Authority-owned embedding materials and source validation.
// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
// Adapted from Nous retrieval/src/embedding.rs at
// 412b062082afb75ccfae4c3f69fc606d3e3cc862; see LICENSE.nous and NOTICE.
// Modified for SQLite, shared call reservations, scoped source identities and
// same-transaction publication of vectors and model job outcomes.
use nous_domain::*;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{Row, SqliteConnection, sqlite::SqliteRow};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use crate::{
    Database, WriteTransaction,
    cognition::{enum_name, json_option, parse_enum, parse_optional},
    not_found, storage_error, write_error,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetrievalEntry {
    pub memory: Memory,
    pub material: Material,
    pub details: MaterialDetails,
    pub embedding: Option<EmbeddingRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetrievalSnapshot {
    pub watermark: i64,
    pub entries: Vec<RetrievalEntry>,
    pub tag_embeddings: Vec<EmbeddingRecord>,
    pub complete: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingClaim {
    pub kind: EmbeddingKind,
    pub source_key: String,
    pub source_hash: String,
    pub job_id: String,
}

impl Database {
    pub async fn resume_waiting_embeddings(&self, include_budget: bool) -> Result<u64> {
        self.resume_waiting_jobs(crate::cognition::WaitingJobKind::Embedding, include_budget)
            .await
    }
    pub async fn embedding_space(&self, id: &str) -> Result<EmbeddingSpace> {
        space(&mut *self.pool.acquire().await.map_err(storage_error)?, id).await
    }

    pub async fn embedding_run(&self, job_id: &str) -> Result<EmbeddingRun> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let run = run(&mut tx, job_id).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(run)
    }

    pub async fn embedding_record(
        &self,
        namespace: &NamespaceId,
        subject: &SubjectId,
        space_id: &str,
        kind: EmbeddingKind,
        key: &str,
    ) -> Result<Option<EmbeddingRecord>> {
        record(
            &mut *self.pool.acquire().await.map_err(storage_error)?,
            namespace,
            subject,
            space_id,
            kind,
            key,
        )
        .await
    }

    pub async fn embeddings_for_memory(
        &self,
        namespace: &NamespaceId,
        id: &MemoryId,
    ) -> Result<Vec<EmbeddingRecord>> {
        sqlx::query("SELECT * FROM embedding_records WHERE namespace_id=? AND memory_id=? ORDER BY space_id,memory_revision,source_key")
            .bind(namespace.as_str()).bind(id.as_str()).fetch_all(&self.pool).await.map_err(storage_error)?
            .into_iter().map(record_row).collect()
    }

    /// One WAL snapshot supplies authority, material metadata and vectors.
    /// The caller applies current accessibility/lifecycle policy before graphing.
    pub async fn retrieval_snapshot(
        &self,
        namespace: &NamespaceId,
        subject: &SubjectId,
        space_id: &str,
        limit: usize,
    ) -> Result<RetrievalSnapshot> {
        let limit = i64::try_from(limit)
            .ok()
            .filter(|v| *v > 0)
            .and_then(|v| v.checked_add(1))
            .ok_or_else(|| Error::invalid("snapshot limit must be positive"))?;
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let watermark =
            sqlx::query_scalar("SELECT commit_seq FROM authority_state WHERE singleton=1")
                .fetch_one(&mut *tx)
                .await
                .map_err(storage_error)?;
        let mut rows=sqlx::query("SELECT r.*,s.content_hash AS source_hash,s.media_type AS source_media_type,s.byte_len AS source_byte_len,s.source AS source_name,s.event_time_domain AS source_event_domain,s.event_time_coordinate AS source_event_coordinate,s.recorded_at AS source_recorded_at,s.commit_seq AS source_commit_seq,d.source_type AS detail_source_type,d.tags AS detail_tags,d.metadata AS detail_metadata,d.structured AS detail_structured FROM memories m JOIN memory_revisions r ON r.memory_id=m.id AND r.revision=m.current_revision JOIN materials s ON s.id=r.material_id LEFT JOIN material_details d ON d.material_id=s.id WHERE m.namespace_id=? AND m.subject_id=? ORDER BY m.id LIMIT ?")
            .bind(namespace.as_str()).bind(subject.as_str()).bind(limit).fetch_all(&mut *tx).await.map_err(storage_error)?;
        let complete = rows.len() < limit as usize;
        if !complete {
            rows.pop();
        }
        let mut entries = Vec::with_capacity(rows.len());
        let mut tags = BTreeSet::new();
        for row in rows {
            let memory = crate::cognition::memory_row_ref(&row)?;
            let domain: Option<String> = row.get("source_event_domain");
            let material = Material {
                id: memory.material_id.clone(),
                namespace_id: namespace.clone(),
                content_hash: row.get("source_hash"),
                media_type: row.get("source_media_type"),
                byte_len: row.get("source_byte_len"),
                source: row.get("source_name"),
                event_time: domain.map(|domain| TimePoint {
                    domain,
                    coordinate: row.get("source_event_coordinate"),
                }),
                recorded_at: row.get("source_recorded_at"),
                commit_seq: row.get("source_commit_seq"),
            };
            let details = MaterialDetails {
                source_type: row.get("detail_source_type"),
                tags: serde_json::from_str(
                    row.get::<Option<&str>, _>("detail_tags").unwrap_or("[]"),
                )
                .map_err(storage_error)?,
                metadata: serde_json::from_str(
                    row.get::<Option<&str>, _>("detail_metadata")
                        .unwrap_or("{}"),
                )
                .map_err(storage_error)?,
                structured: parse_optional(row.get("detail_structured"))?,
            };
            for tag in &details.tags {
                let text = tag.trim();
                if !text.is_empty() {
                    tags.insert(format!("tag:{}", hash(text)));
                }
            }
            entries.push(RetrievalEntry {
                memory,
                material,
                details,
                embedding: None,
            });
        }
        let ids = entries
            .iter()
            .map(|e| e.memory.id.as_str())
            .collect::<Vec<_>>();
        let records=sqlx::query("SELECT e.* FROM embedding_records e JOIN memories m ON m.id=e.memory_id AND m.current_revision=e.memory_revision WHERE e.namespace_id=? AND e.subject_id=? AND e.space_id=? AND e.kind='memory' AND m.id IN (SELECT value FROM json_each(?))")
            .bind(namespace.as_str()).bind(subject.as_str()).bind(space_id).bind(serde_json::to_string(&ids)?).fetch_all(&mut *tx).await.map_err(storage_error)?;
        let mut by_key = records
            .into_iter()
            .map(record_row)
            .collect::<Result<Vec<_>>>()?
            .into_iter()
            .map(|e| (e.source_key.clone(), e))
            .collect::<BTreeMap<_, _>>();
        for entry in &mut entries {
            entry.embedding = by_key.remove(&format!(
                "memory:{}@{}",
                entry.memory.id, entry.memory.revision
            ));
        }
        let tag_embeddings=sqlx::query("SELECT * FROM embedding_records WHERE namespace_id=? AND subject_id=? AND space_id=? AND kind='tag' AND source_key IN (SELECT value FROM json_each(?)) ORDER BY source_key")
            .bind(namespace.as_str()).bind(subject.as_str()).bind(space_id).bind(serde_json::to_string(&tags)?).fetch_all(&mut *tx).await.map_err(storage_error)?
            .into_iter().map(record_row).collect::<Result<Vec<_>>>()?;
        tx.commit().await.map_err(storage_error)?;
        Ok(RetrievalSnapshot {
            watermark,
            entries,
            tag_embeddings,
            complete,
        })
    }
}

impl WriteTransaction {
    /// The write transaction serializes planning and input ownership. Terminal
    /// jobs release their inputs without a second claim state or cleanup queue.
    pub async fn embedding_claims(
        &mut self,
        namespace: &NamespaceId,
        subject: &SubjectId,
        space_id: &str,
    ) -> Result<Vec<EmbeddingClaim>> {
        sqlx::query("SELECT i.kind,i.source_key,i.source_hash,i.job_id FROM embedding_inputs i JOIN embedding_runs r ON r.job_id=i.job_id WHERE r.namespace_id=? AND r.subject_id=? AND r.space_id=? AND r.state IN ('queued','running','waiting_for_configuration','waiting_for_budget','uncertain') ORDER BY i.kind,i.source_key,i.job_id")
            .bind(namespace.as_str()).bind(subject.as_str()).bind(space_id).fetch_all(&mut *self.tx).await.map_err(storage_error)?
            .into_iter().map(|row|Ok(EmbeddingClaim {kind:parse_enum(row.get("kind"))?,source_key:row.get("source_key"),source_hash:row.get("source_hash"),job_id:row.get("job_id")})).collect()
    }
    pub async fn embedding_run(&mut self, job_id: &str) -> Result<EmbeddingRun> {
        run(&mut self.tx, job_id).await
    }
    pub async fn embedding_space(&mut self, id: &str) -> Result<EmbeddingSpace> {
        space(&mut self.tx, id).await
    }

    pub async fn save_embedding_space(&mut self, value: &EmbeddingSpace) -> Result<()> {
        for (field, name) in [
            (&value.id, "space ID"),
            (&value.resource_id, "embedding resource"),
            (&value.provider_model, "provider model"),
            (&value.producer_signature, "embedding producer"),
        ] {
            validate_name(field, name)?;
        }
        if value.modality != "text" || value.dimension == 0 || value.dimension > 65536 {
            return Err(Error::invalid("text embedding dimension must be 1..65536"));
        }
        sqlx::query("INSERT INTO embedding_spaces(id,resource_id,provider_model,dimension,producer_signature,modality) VALUES (?,?,?,?,?,?) ON CONFLICT(id) DO NOTHING")
            .bind(&value.id).bind(&value.resource_id).bind(&value.provider_model).bind(value.dimension as i64).bind(&value.producer_signature).bind(&value.modality)
            .execute(&mut *self.tx).await.map_err(write_error)?;
        if self.embedding_space(&value.id).await? != *value {
            return Err(Error::new(
                ErrorCode::Conflict,
                "embedding space identity is immutable",
            ));
        }
        Ok(())
    }

    pub async fn insert_embedding_run(&mut self, value: &EmbeddingRun) -> Result<()> {
        if value.inputs.is_empty() || value.inputs.len() > 256 {
            return Err(Error::invalid("embedding batch must contain 1..256 inputs"));
        }
        let space = self.embedding_space(&value.space_id).await?;
        if space.producer_signature != value.producer_signature {
            return Err(Error::new(
                ErrorCode::Conflict,
                "embedding producer disagrees with space",
            ));
        }
        let claims = self
            .embedding_claims(&value.namespace_id, &value.subject_id, &value.space_id)
            .await?;
        let claimed = claims
            .iter()
            .map(|claim| {
                (
                    (claim.kind, claim.source_key.as_str()),
                    claim.job_id.as_str(),
                )
            })
            .collect::<HashMap<_, _>>();
        let mut keys = HashSet::new();
        for input in &value.inputs {
            if let Some(job_id) = claimed.get(&(input.kind, input.source_key.as_str())) {
                return Err(Error::new(
                    ErrorCode::Conflict,
                    format!("embedding source is already owned by active job {job_id}"),
                ));
            }
            if !keys.insert((input.kind, &input.source_key)) {
                return Err(Error::invalid("embedding batch repeats a source identity"));
            }
            validate_input(&mut self.tx, &value.namespace_id, &value.subject_id, input).await?;
        }
        self.register_model_job(
            &value.job_id,
            &value.namespace_id,
            &value.subject_id,
            "embedding",
        )
        .await?;
        sqlx::query("INSERT INTO embedding_runs(job_id,namespace_id,subject_id,producer_signature,space_id,state,model_call_id,error,created_at,updated_at) VALUES (?,?,?,?,?,?,?,?,?,?)")
            .bind(&value.job_id).bind(value.namespace_id.as_str()).bind(value.subject_id.as_str()).bind(&value.producer_signature).bind(&value.space_id).bind(enum_name(&value.state)?)
            .bind(value.model_call_id.as_ref().map(ModelCallId::as_str)).bind(json_option(&value.error)?).bind(&value.created_at).bind(&value.updated_at)
            .execute(&mut *self.tx).await.map_err(write_error)?;
        for (ordinal, input) in value.inputs.iter().enumerate() {
            sqlx::query("INSERT INTO embedding_inputs(job_id,ordinal,kind,source_key,text,source_hash,memory_id,memory_revision) VALUES (?,?,?,?,?,?,?,?)")
                .bind(&value.job_id).bind(ordinal as i64).bind(enum_name(&input.kind)?).bind(&input.source_key).bind(&input.text).bind(&input.source_hash)
                .bind(input.memory_id.as_ref().map(MemoryId::as_str)).bind(input.memory_revision).execute(&mut *self.tx).await.map_err(write_error)?;
        }
        Ok(())
    }

    pub async fn update_embedding_run(&mut self, value: &EmbeddingRun) -> Result<()> {
        let previous = self.embedding_run(&value.job_id).await?;
        if previous.namespace_id != value.namespace_id
            || previous.subject_id != value.subject_id
            || previous.space_id != value.space_id
            || previous.producer_signature != value.producer_signature
            || previous.inputs != value.inputs
            || previous.created_at != value.created_at
        {
            return Err(Error::new(
                ErrorCode::Conflict,
                "embedding job identity and source inputs are immutable",
            ));
        }
        if matches!(
            previous.state,
            EmbeddingState::Succeeded | EmbeddingState::Cancelled
        ) && previous.state != value.state
        {
            return Err(Error::new(
                ErrorCode::Conflict,
                "embedding job already reached a terminal outcome",
            ));
        }
        sqlx::query(
            "UPDATE embedding_runs SET state=?,model_call_id=?,error=?,updated_at=? WHERE job_id=?",
        )
        .bind(enum_name(&value.state)?)
        .bind(value.model_call_id.as_ref().map(ModelCallId::as_str))
        .bind(json_option(&value.error)?)
        .bind(&value.updated_at)
        .bind(&value.job_id)
        .execute(&mut *self.tx)
        .await
        .map_err(write_error)?;
        Ok(())
    }

    /// Publish once for each scoped source identity. A previously accepted
    /// content-equivalent vector may be linked to a newer Memory revision.
    pub async fn save_embedding_record(&mut self, value: &EmbeddingRecord) -> Result<()> {
        self.check_sequence(value.commit_seq)?;
        let space = self.embedding_space(&value.space_id).await?;
        if value.vector.len() != space.dimension
            || value.vector.iter().any(|v| !v.is_finite())
            || !value.vector.iter().any(|v| *v != 0.0)
        {
            return Err(Error::invalid(
                "embedding vector must have the configured dimension and finite nonzero magnitude",
            ));
        }
        let call = self.model_call(&value.model_call_id).await?;
        if call.state != ModelCallState::Succeeded
            || call.namespace_id != value.namespace_id
            || call.subject_id != value.subject_id
            || call.resource_id != space.resource_id
        {
            return Err(Error::new(
                ErrorCode::Conflict,
                "embedding adoption requires a successful call in this scope and resource",
            ));
        }
        let run = self.embedding_run(&call.job_id).await?;
        if run.space_id != space.id || run.producer_signature != space.producer_signature {
            return Err(Error::new(
                ErrorCode::Conflict,
                "embedding call belongs to another space",
            ));
        }
        if let Some(existing) = record(
            &mut self.tx,
            &value.namespace_id,
            &value.subject_id,
            &value.space_id,
            value.kind,
            &value.source_key,
        )
        .await?
        {
            if existing.source_hash != value.source_hash
                || existing.memory_id != value.memory_id
                || existing.memory_revision != value.memory_revision
            {
                return Err(Error::new(
                    ErrorCode::Conflict,
                    "embedding source identity already belongs to different content",
                ));
            }
            return Ok(());
        }
        let equivalent=sqlx::query("SELECT * FROM embedding_records WHERE namespace_id=? AND subject_id=? AND space_id=? AND kind='memory' AND ?='memory' AND source_hash=? AND model_call_id=? AND memory_id IS ? LIMIT 1")
            .bind(value.namespace_id.as_str()).bind(value.subject_id.as_str()).bind(&value.space_id).bind(enum_name(&value.kind)?).bind(&value.source_hash).bind(value.model_call_id.as_str()).bind(value.memory_id.as_ref().map(MemoryId::as_str))
            .fetch_optional(&mut *self.tx).await.map_err(storage_error)?.map(record_row).transpose()?;
        let input = if let Some(previous) = equivalent {
            if previous.vector != value.vector {
                return Err(Error::new(
                    ErrorCode::Conflict,
                    "content reuse must preserve the previously accepted vector",
                ));
            }
            let text = source_text(&mut self.tx, value).await?;
            EmbeddingInputItem {
                kind: value.kind,
                source_key: value.source_key.clone(),
                text,
                memory_id: value.memory_id.clone(),
                memory_revision: value.memory_revision,
                source_hash: value.source_hash.clone(),
            }
        } else {
            if matches!(
                run.state,
                EmbeddingState::Cancelled | EmbeddingState::Failed
            ) {
                return Err(Error::new(
                    ErrorCode::Conflict,
                    "embedding job cannot adopt new output",
                ));
            }
            run.inputs
                .iter()
                .find(|input| {
                    input.kind == value.kind
                        && input.source_hash == value.source_hash
                        && input.memory_id == value.memory_id
                })
                .cloned()
                .ok_or_else(|| {
                    Error::new(
                        ErrorCode::Conflict,
                        "embedding source was not part of the actual model call",
                    )
                })?
        };
        // The submitted identity may be a newer revision with the same content.
        let text = if value.kind == EmbeddingKind::Memory {
            source_text(&mut self.tx, value).await?
        } else {
            input.text
        };
        let target = EmbeddingInputItem {
            kind: value.kind,
            source_key: value.source_key.clone(),
            text,
            memory_id: value.memory_id.clone(),
            memory_revision: value.memory_revision,
            source_hash: value.source_hash.clone(),
        };
        validate_input(
            &mut self.tx,
            &value.namespace_id,
            &value.subject_id,
            &target,
        )
        .await?;
        let vector = value
            .vector
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect::<Vec<_>>();
        sqlx::query("INSERT INTO embedding_records(namespace_id,subject_id,space_id,kind,source_key,source_hash,memory_id,memory_revision,dimension,vector,model_call_id,commit_seq) VALUES (?,?,?,?,?,?,?,?,?,?,?,?) ON CONFLICT(namespace_id,subject_id,space_id,kind,source_key) DO NOTHING")
            .bind(value.namespace_id.as_str()).bind(value.subject_id.as_str()).bind(&value.space_id).bind(enum_name(&value.kind)?).bind(&value.source_key).bind(&value.source_hash)
            .bind(value.memory_id.as_ref().map(MemoryId::as_str)).bind(value.memory_revision).bind(space.dimension as i64).bind(vector).bind(value.model_call_id.as_str()).bind(value.commit_seq)
            .execute(&mut *self.tx).await.map_err(write_error)?;
        Ok(())
    }

    pub(crate) async fn purge_memory_embeddings(&mut self, id: &MemoryId) -> Result<()> {
        let jobs: Vec<String> =
            sqlx::query_scalar("SELECT DISTINCT job_id FROM embedding_inputs WHERE memory_id=?")
                .bind(id.as_str())
                .fetch_all(&mut *self.tx)
                .await
                .map_err(storage_error)?;
        for job in jobs {
            let calls = sqlx::query("SELECT id,request,response FROM model_calls WHERE job_id=?")
                .bind(&job)
                .fetch_all(&mut *self.tx)
                .await
                .map_err(storage_error)?;
            for call in calls {
                let mut request: Value = serde_json::from_str(call.get("request"))?;
                redact_embedding_payload(&mut request);
                let mut response: Option<Value> = parse_optional(call.get("response"))?;
                if let Some(response) = &mut response {
                    redact_embedding_payload(response);
                }
                sqlx::query("UPDATE model_calls SET request=?,response=? WHERE id=?")
                    .bind(serde_json::to_string(&request)?)
                    .bind(json_option(&response)?)
                    .bind(call.get::<String, _>("id"))
                    .execute(&mut *self.tx)
                    .await
                    .map_err(write_error)?;
            }
            let error = Error::new(
                ErrorCode::NotFound,
                "embedding input was permanently purged; batch cancelled",
            );
            sqlx::query("UPDATE embedding_runs SET state='cancelled',error=?,updated_at=strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE job_id=?")
                .bind(serde_json::to_string(&error)?).bind(&job).execute(&mut *self.tx).await.map_err(write_error)?;
            sqlx::query("UPDATE embedding_inputs SET text='' WHERE job_id=? AND kind='memory'")
                .bind(&job)
                .execute(&mut *self.tx)
                .await
                .map_err(write_error)?;
        }
        sqlx::query("DELETE FROM embedding_records WHERE memory_id=?")
            .bind(id.as_str())
            .execute(&mut *self.tx)
            .await
            .map_err(write_error)?;
        Ok(())
    }
}

fn redact_embedding_payload(value: &mut Value) {
    fn remove_bodies(value: &mut Value) {
        match value {
            Value::Object(fields) => {
                for (name, value) in fields {
                    match name.as_str() {
                        "text" | "rendered_user" => *value = Value::String(String::new()),
                        "vectors" => *value = json!([]),
                        "raw_output" | "provider_response" | "raw_body" | "captured_raw_body" => {
                            *value = Value::Null
                        }
                        _ => remove_bodies(value),
                    }
                }
            }
            Value::Array(items) => {
                for item in items {
                    remove_bodies(item)
                }
            }
            _ => {}
        }
    }
    remove_bodies(value);
    if let Some(fields) = value.as_object_mut() {
        fields.insert("redacted".into(), Value::Bool(true));
        fields.insert(
            "redaction_reason".into(),
            Value::String("memory_purged".into()),
        );
    }
}

async fn space(conn: &mut SqliteConnection, id: &str) -> Result<EmbeddingSpace> {
    let row = sqlx::query("SELECT * FROM embedding_spaces WHERE id=?")
        .bind(id)
        .fetch_optional(conn)
        .await
        .map_err(storage_error)?
        .ok_or_else(not_found)?;
    Ok(EmbeddingSpace {
        id: row.get("id"),
        resource_id: row.get("resource_id"),
        provider_model: row.get("provider_model"),
        dimension: row.get::<i64, _>("dimension") as usize,
        producer_signature: row.get("producer_signature"),
        modality: row.get("modality"),
    })
}

async fn run(conn: &mut SqliteConnection, id: &str) -> Result<EmbeddingRun> {
    let row = sqlx::query("SELECT * FROM embedding_runs WHERE job_id=?")
        .bind(id)
        .fetch_optional(&mut *conn)
        .await
        .map_err(storage_error)?
        .ok_or_else(not_found)?;
    let inputs = sqlx::query("SELECT * FROM embedding_inputs WHERE job_id=? ORDER BY ordinal")
        .bind(id)
        .fetch_all(conn)
        .await
        .map_err(storage_error)?
        .into_iter()
        .map(|row| {
            Ok(EmbeddingInputItem {
                kind: parse_enum(row.get("kind"))?,
                source_key: row.get("source_key"),
                text: row.get("text"),
                memory_id: row.get::<Option<String>, _>("memory_id").map(MemoryId),
                memory_revision: row.get("memory_revision"),
                source_hash: row.get("source_hash"),
            })
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(EmbeddingRun {
        job_id: row.get("job_id"),
        namespace_id: NamespaceId(row.get("namespace_id")),
        subject_id: SubjectId(row.get("subject_id")),
        producer_signature: row.get("producer_signature"),
        space_id: row.get("space_id"),
        inputs,
        state: parse_enum(row.get("state"))?,
        model_call_id: row
            .get::<Option<String>, _>("model_call_id")
            .map(ModelCallId),
        error: parse_optional(row.get("error"))?,
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    })
}

async fn record(
    conn: &mut SqliteConnection,
    ns: &NamespaceId,
    subject: &SubjectId,
    space: &str,
    kind: EmbeddingKind,
    key: &str,
) -> Result<Option<EmbeddingRecord>> {
    sqlx::query("SELECT * FROM embedding_records WHERE namespace_id=? AND subject_id=? AND space_id=? AND kind=? AND source_key=?")
        .bind(ns.as_str()).bind(subject.as_str()).bind(space).bind(enum_name(&kind)?).bind(key).fetch_optional(conn).await.map_err(storage_error)?.map(record_row).transpose()
}

fn record_row(row: SqliteRow) -> Result<EmbeddingRecord> {
    let bytes: Vec<u8> = row.get("vector");
    if !bytes.len().is_multiple_of(4) {
        return Err(Error::internal("stored embedding byte length is invalid"));
    }
    let vector = bytes
        .chunks_exact(4)
        .map(|b| f32::from_le_bytes(b.try_into().expect("four-byte chunk")))
        .collect::<Vec<_>>();
    if vector.iter().any(|v| !v.is_finite()) || !vector.iter().any(|v| *v != 0.0) {
        return Err(Error::internal("stored embedding contains invalid values"));
    }
    Ok(EmbeddingRecord {
        namespace_id: NamespaceId(row.get("namespace_id")),
        subject_id: SubjectId(row.get("subject_id")),
        space_id: row.get("space_id"),
        kind: parse_enum(row.get("kind"))?,
        source_key: row.get("source_key"),
        source_hash: row.get("source_hash"),
        memory_id: row.get::<Option<String>, _>("memory_id").map(MemoryId),
        memory_revision: row.get("memory_revision"),
        vector,
        model_call_id: ModelCallId(row.get("model_call_id")),
        commit_seq: row.get("commit_seq"),
    })
}

async fn source_text(conn: &mut SqliteConnection, value: &EmbeddingRecord) -> Result<String> {
    if value.kind != EmbeddingKind::Memory {
        return Err(Error::invalid(
            "content reuse requires an actual source input",
        ));
    }
    let id = value
        .memory_id
        .as_ref()
        .ok_or_else(|| Error::invalid("memory embedding requires memory identity"))?;
    let row=sqlx::query("SELECT title,content FROM memory_revisions WHERE namespace_id=? AND subject_id=? AND memory_id=? AND revision=?")
        .bind(value.namespace_id.as_str()).bind(value.subject_id.as_str()).bind(id.as_str()).bind(value.memory_revision).fetch_optional(conn).await.map_err(storage_error)?.ok_or_else(not_found)?;
    Ok(format!(
        "{}\n{}",
        row.get::<String, _>("title"),
        row.get::<String, _>("content")
    ))
}

async fn validate_input(
    conn: &mut SqliteConnection,
    ns: &NamespaceId,
    subject: &SubjectId,
    input: &EmbeddingInputItem,
) -> Result<()> {
    if input.text.is_empty() || input.source_hash != hash(&input.text) {
        return Err(Error::invalid(
            "embedding input hash must identify its actual UTF-8 text",
        ));
    }
    match input.kind {
        EmbeddingKind::Memory => {
            let id = input
                .memory_id
                .as_ref()
                .ok_or_else(|| Error::invalid("memory embedding requires identity and revision"))?;
            let revision = input
                .memory_revision
                .filter(|v| *v > 0)
                .ok_or_else(|| Error::invalid("memory embedding requires a positive revision"))?;
            if input.source_key != format!("memory:{id}@{revision}") {
                return Err(Error::invalid(
                    "memory embedding source key disagrees with its revision",
                ));
            }
            let row=sqlx::query("SELECT title,content FROM memory_revisions WHERE namespace_id=? AND subject_id=? AND memory_id=? AND revision=?")
                .bind(ns.as_str()).bind(subject.as_str()).bind(id.as_str()).bind(revision).fetch_optional(conn).await.map_err(storage_error)?.ok_or_else(not_found)?;
            let text = format!(
                "{}\n{}",
                row.get::<String, _>("title"),
                row.get::<String, _>("content")
            );
            if input.text != text {
                return Err(Error::new(
                    ErrorCode::Conflict,
                    "memory embedding text disagrees with its authority revision",
                ));
            }
        }
        EmbeddingKind::Tag | EmbeddingKind::Query => {
            let prefix = if input.kind == EmbeddingKind::Tag {
                "tag"
            } else {
                "query"
            };
            if input.memory_id.is_some()
                || input.memory_revision.is_some()
                || input.text != input.text.trim()
                || input.source_key != format!("{prefix}:{}", input.source_hash)
            {
                return Err(Error::invalid(
                    "tag/query embedding identity must identify trimmed text without a Memory reference",
                ));
            }
        }
    }
    Ok(())
}

fn hash(text: &str) -> String {
    format!("{:x}", Sha256::digest(text.as_bytes()))
}
