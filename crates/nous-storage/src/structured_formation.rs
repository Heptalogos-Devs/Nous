//! Frozen model inputs and independent, idempotent proposal-group adoption.
// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
// Reuses the formation/embedding model_jobs and transaction implementation.
use std::collections::{HashMap, HashSet};

use nous_domain::{
    CognitiveId, CognitiveKind, CognitiveRef, Error, ErrorCode, EvidenceSource, FormationState,
    MemoryId, ModelCallId, ModelCallState, NamespaceId, Result, SubjectId,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{Row, SqliteConnection, sqlite::SqliteRow};

use crate::{
    Database, WriteTransaction, cognition::WaitingJobKind, not_found, storage_error, write_error,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StructuredFormationRun {
    pub job_id: String,
    pub namespace_id: NamespaceId,
    pub subject_id: SubjectId,
    pub state: FormationState,
    pub input: Value,
    pub output: Option<Value>,
    pub model_call_id: Option<ModelCallId>,
    pub error: Option<Error>,
    pub created_at: String,
    pub updated_at: String,
}

impl StructuredFormationRun {
    /// Retired inputs are metadata-only and must never be prepared or adopted.
    pub fn is_retired(&self) -> bool {
        self.state == FormationState::Cancelled
            || self.input.get("redacted") == Some(&Value::Bool(true))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StructuredGroupState {
    Pending,
    Applied,
    Rejected,
    Conflict,
    Skipped,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StructuredFormationGroup {
    pub key: String,
    pub ordinal: i64,
    pub depends_on: Vec<String>,
    pub state: StructuredGroupState,
    pub result: Option<Value>,
    pub error: Option<Error>,
    pub commit_seq: Option<i64>,
}

impl PartialEq for StructuredFormationGroup {
    fn eq(&self, other: &Self) -> bool {
        self.key == other.key
            && self.ordinal == other.ordinal
            && self.depends_on == other.depends_on
            && self.state == other.state
            && self.result == other.result
            && self.commit_seq == other.commit_seq
            && self
                .error
                .as_ref()
                .map(|error| (error.code, error.message.as_str()))
                == other
                    .error
                    .as_ref()
                    .map(|error| (error.code, error.message.as_str()))
    }
}

impl Database {
    pub async fn structured_formation(&self, job_id: &str) -> Result<StructuredFormationRun> {
        run(
            &mut *self.pool.acquire().await.map_err(storage_error)?,
            job_id,
        )
        .await
    }
    pub async fn structured_formation_groups(
        &self,
        job_id: &str,
    ) -> Result<Vec<StructuredFormationGroup>> {
        groups(
            &mut *self.pool.acquire().await.map_err(storage_error)?,
            job_id,
        )
        .await
    }
    pub async fn resume_waiting_structured_formations(&self, include_budget: bool) -> Result<u64> {
        self.resume_waiting_jobs(WaitingJobKind::StructuredFormation, include_budget)
            .await
    }
}

impl WriteTransaction {
    pub async fn structured_formation(&mut self, job_id: &str) -> Result<StructuredFormationRun> {
        run(&mut self.tx, job_id).await
    }
    pub async fn structured_formation_groups(
        &mut self,
        job_id: &str,
    ) -> Result<Vec<StructuredFormationGroup>> {
        groups(&mut self.tx, job_id).await
    }

    pub async fn insert_structured_formation(
        &mut self,
        value: &StructuredFormationRun,
    ) -> Result<()> {
        if !value.input.is_object()
            || value.output.is_some()
            || value.model_call_id.is_some()
            || value.state != FormationState::Queued
        {
            return Err(Error::invalid(
                "structured formation starts queued with a frozen input object and no output",
            ));
        }
        self.validate_structured_formation_sources(value).await?;
        self.register_model_job(
            &value.job_id,
            &value.namespace_id,
            &value.subject_id,
            "structured_formation",
        )
        .await?;
        sqlx::query("INSERT INTO structured_formation_runs(job_id,namespace_id,subject_id,state,input,output,model_call_id,error,created_at,updated_at) VALUES (?,?,?,?,?,?,?,?,?,?)")
            .bind(&value.job_id).bind(value.namespace_id.as_str()).bind(value.subject_id.as_str()).bind(enum_name(&value.state)?).bind(serde_json::to_string(&value.input)?)
            .bind(json_option(&value.output)?).bind(value.model_call_id.as_ref().map(ModelCallId::as_str)).bind(json_option(&value.error)?).bind(&value.created_at).bind(&value.updated_at)
            .execute(&mut *self.tx).await.map_err(write_error)?;
        Ok(())
    }

    pub async fn update_structured_formation(
        &mut self,
        value: &StructuredFormationRun,
    ) -> Result<()> {
        let previous = self.structured_formation(&value.job_id).await?;
        if previous.is_retired() {
            return Err(Error::new(
                ErrorCode::Conflict,
                "structured formation input was retired",
            ));
        }
        if value.namespace_id != previous.namespace_id
            || value.subject_id != previous.subject_id
            || value.input != previous.input
            || value.created_at != previous.created_at
        {
            return Err(Error::new(
                ErrorCode::Conflict,
                "structured formation scope and frozen input are immutable",
            ));
        }
        if matches!(
            previous.state,
            FormationState::Succeeded | FormationState::Cancelled
        ) && (value.state != previous.state
            || value.output != previous.output
            || value.model_call_id != previous.model_call_id)
        {
            return Err(Error::new(
                ErrorCode::Conflict,
                "structured formation is already terminal",
            ));
        }
        if previous.output.is_some()
            && (value.output != previous.output || value.model_call_id != previous.model_call_id)
        {
            return Err(Error::new(
                ErrorCode::Conflict,
                "accepted structured proposal and its call identity are immutable",
            ));
        }
        if value.output.is_some() {
            let call_id = value.model_call_id.as_ref().ok_or_else(|| {
                Error::invalid("structured proposal requires its successful model call")
            })?;
            let call = self.model_call(call_id).await?;
            if call.job_id != value.job_id
                || call.namespace_id != value.namespace_id
                || call.subject_id != value.subject_id
                || call.state != ModelCallState::Succeeded
            {
                return Err(Error::invalid(
                    "structured proposal requires a successful call for this scoped job",
                ));
            }
        }
        if value.state == FormationState::Succeeded {
            let groups = self.structured_formation_groups(&value.job_id).await?;
            if value.output.is_none()
                || groups
                    .iter()
                    .any(|group| group.state == StructuredGroupState::Pending)
            {
                return Err(Error::new(
                    ErrorCode::Conflict,
                    "structured formation still has unadopted groups or no proposal",
                ));
            }
        }
        sqlx::query("UPDATE structured_formation_runs SET state=?,output=?,model_call_id=?,error=?,updated_at=? WHERE job_id=?")
            .bind(enum_name(&value.state)?).bind(json_option(&value.output)?).bind(value.model_call_id.as_ref().map(ModelCallId::as_str)).bind(json_option(&value.error)?).bind(&value.updated_at).bind(&value.job_id)
            .execute(&mut *self.tx).await.map_err(write_error)?;
        Ok(())
    }

    /// Register the proposal's complete group graph once, in the transaction
    /// that saves the validated output. Dependencies keep stable group keys.
    pub async fn insert_structured_groups(
        &mut self,
        job_id: &str,
        values: &[StructuredFormationGroup],
    ) -> Result<()> {
        let run = self.structured_formation(job_id).await?;
        if run.is_retired() {
            return Err(Error::new(
                ErrorCode::Conflict,
                "structured formation input was retired",
            ));
        }
        if run.output.is_none()
            || matches!(
                run.state,
                FormationState::Succeeded | FormationState::Cancelled
            )
        {
            return Err(Error::new(
                ErrorCode::Conflict,
                "groups require an accepted, unfinished structured proposal",
            ));
        }
        let existing = self.structured_formation_groups(job_id).await?;
        if !existing.is_empty() {
            if existing == values {
                return Ok(());
            }
            return Err(Error::new(
                ErrorCode::Conflict,
                "structured group graph is already registered",
            ));
        }
        validate_groups(values)?;
        for value in values {
            sqlx::query("INSERT INTO structured_formation_groups(job_id,key,ordinal,depends_on,state,result,error,commit_seq) VALUES (?,?,?,?,?,?,?,?)")
                .bind(job_id).bind(&value.key).bind(value.ordinal).bind(serde_json::to_string(&value.depends_on)?).bind(enum_name(&value.state)?).bind(json_option(&value.result)?).bind(json_option(&value.error)?).bind(value.commit_seq)
                .execute(&mut *self.tx).await.map_err(write_error)?;
        }
        Ok(())
    }

    /// Call in the same operation transaction as this group's domain revisions.
    /// Terminal outcomes are immutable, so a crash before queue ACK cannot
    /// adopt the group twice or overwrite its conflict/skip result.
    pub async fn update_structured_group(
        &mut self,
        job_id: &str,
        value: &StructuredFormationGroup,
    ) -> Result<()> {
        let run = self.structured_formation(job_id).await?;
        if run.is_retired() {
            return Err(Error::new(
                ErrorCode::Conflict,
                "structured formation input was retired",
            ));
        }
        if matches!(
            run.state,
            FormationState::Succeeded | FormationState::Cancelled
        ) {
            return Err(Error::new(
                ErrorCode::Conflict,
                "structured formation is terminal",
            ));
        }
        let groups = self.structured_formation_groups(job_id).await?;
        let previous = groups
            .iter()
            .find(|group| group.key == value.key)
            .ok_or_else(not_found)?;
        if previous.ordinal != value.ordinal || previous.depends_on != value.depends_on {
            return Err(Error::new(
                ErrorCode::Conflict,
                "structured group identity and dependencies are immutable",
            ));
        }
        if previous.state != StructuredGroupState::Pending {
            if previous == value {
                return Ok(());
            }
            return Err(Error::new(
                ErrorCode::Conflict,
                "structured group is already terminal",
            ));
        }
        if value.state == StructuredGroupState::Pending
            || value.commit_seq != Some(self.commit_seq())
        {
            return Err(Error::invalid(
                "settled group must use its authority transaction sequence",
            ));
        }
        let dependencies = value
            .depends_on
            .iter()
            .map(|key| {
                groups
                    .iter()
                    .find(|group| &group.key == key)
                    .ok_or_else(not_found)
            })
            .collect::<Result<Vec<_>>>()?;
        if value.state == StructuredGroupState::Applied
            && dependencies
                .iter()
                .any(|group| group.state != StructuredGroupState::Applied)
        {
            return Err(Error::new(
                ErrorCode::Conflict,
                "group dependencies have not been applied",
            ));
        }
        if value.state == StructuredGroupState::Skipped
            && !dependencies.iter().any(|group| {
                matches!(
                    group.state,
                    StructuredGroupState::Rejected
                        | StructuredGroupState::Conflict
                        | StructuredGroupState::Skipped
                )
            })
        {
            return Err(Error::invalid(
                "dependency skip requires a failed dependency",
            ));
        }
        sqlx::query("UPDATE structured_formation_groups SET state=?,result=?,error=?,commit_seq=? WHERE job_id=? AND key=? AND state='pending'")
            .bind(enum_name(&value.state)?).bind(json_option(&value.result)?).bind(json_option(&value.error)?).bind(value.commit_seq).bind(job_id).bind(&value.key)
            .execute(&mut *self.tx).await.map_err(write_error)?;
        Ok(())
    }

    /// Validate captured source identities under the enqueue writer lock. A
    /// source read before this transaction cannot race a permanent purge.
    async fn validate_structured_formation_sources(
        &mut self,
        value: &StructuredFormationRun,
    ) -> Result<()> {
        let Some(sources) = value.input.get("sources") else {
            return Ok(());
        };
        let sources = sources
            .as_array()
            .ok_or_else(|| Error::invalid("frozen formation sources must be an array"))?;
        for entry in sources {
            let source: EvidenceSource =
                serde_json::from_value(entry.get("source").unwrap_or(entry).clone())?;
            source.validate()?;
            let dependencies = source_dependencies(
                &mut self.tx,
                &value.namespace_id,
                &value.subject_id,
                &source,
            )
            .await?;
            for dependency in dependencies {
                match dependency {
                    EvidenceSource::Material { material_id } => {
                        self.material(&value.namespace_id, &material_id).await?;
                        let exposed:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM observations WHERE namespace_id=? AND subject_id=? AND material_id=?)")
                            .bind(value.namespace_id.as_str()).bind(value.subject_id.as_str()).bind(material_id.as_str()).fetch_one(&mut *self.tx).await.map_err(storage_error)?;
                        if !exposed {
                            return Err(Error::new(
                                ErrorCode::NotFound,
                                "formation material is unavailable in this subject",
                            ));
                        }
                    }
                    EvidenceSource::Observation { observation_id } => {
                        if self
                            .observation(&value.namespace_id, &observation_id)
                            .await?
                            .subject_id
                            != value.subject_id
                        {
                            return Err(Error::new(
                                ErrorCode::NotFound,
                                "formation observation is unavailable in this subject",
                            ));
                        }
                    }
                    EvidenceSource::MemoryRevision {
                        memory_id,
                        revision,
                    } => {
                        self.validate_reference(
                            &value.namespace_id,
                            &value.subject_id,
                            &CognitiveRef::memory(&memory_id, revision),
                        )
                        .await?;
                        let current = self.memory(&value.namespace_id, &memory_id).await?;
                        let deleted: bool = sqlx::query_scalar(
                            "SELECT deleted FROM memory_revisions WHERE memory_id=? AND revision=?",
                        )
                        .bind(memory_id.as_str())
                        .bind(revision)
                        .fetch_one(&mut *self.tx)
                        .await
                        .map_err(storage_error)?;
                        if current.deleted || deleted {
                            return Err(Error::new(
                                ErrorCode::Conflict,
                                "formation Memory source is retired",
                            ));
                        }
                    }
                    EvidenceSource::CognitiveRevision { reference } => {
                        self.validate_reference(&value.namespace_id, &value.subject_id, &reference)
                            .await?;
                        let id = CognitiveId(reference.id);
                        if self
                            .structured_revision(&value.namespace_id, &id, reference.revision)
                            .await?
                            .deleted
                            || self
                                .structured_object(&value.namespace_id, &id)
                                .await?
                                .deleted
                        {
                            return Err(Error::new(
                                ErrorCode::Conflict,
                                "formation cognition source is retired",
                            ));
                        }
                    }
                }
            }
        }
        Ok(())
    }

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
        let rows=sqlx::query("SELECT job_id,input,output FROM structured_formation_runs WHERE namespace_id=? AND subject_id=?")
            .bind(namespace.as_str()).bind(subject.as_str()).fetch_all(&mut *self.tx).await.map_err(storage_error)?;
        let error = Error::new(
            ErrorCode::NotFound,
            "structured formation source was permanently purged; captured payload retired",
        );
        for row in rows {
            let input: Value = serde_json::from_str(row.get("input"))?;
            let output: Option<Value> = parse_optional(row.get("output"))?;
            if !references_affected(&input, memory, &affected)
                && !output
                    .as_ref()
                    .is_some_and(|output| references_affected(output, memory, &affected))
            {
                continue;
            }
            let job: String = row.get("job_id");
            let redacted =
                serde_json::json!({"schema_version":1,"redacted":true,"reason":"memory_purged"});
            let calls =
                sqlx::query("SELECT id,request,response,error FROM model_calls WHERE job_id=?")
                    .bind(&job)
                    .fetch_all(&mut *self.tx)
                    .await
                    .map_err(storage_error)?;
            for call in calls {
                let request: Value = serde_json::from_str(call.get("request"))?;
                let mut request = retain_fields(
                    &request,
                    &[
                        "schema_version",
                        "call_id",
                        "role",
                        "resource",
                        "prompt_version",
                        "prompt_sha256",
                        "parameters",
                        "max_calls_per_day",
                    ],
                );
                request.extend(redacted.as_object().expect("redacted marker").clone());
                let response: Option<Value> = parse_optional(call.get("response"))?;
                let response = response.map(|response| {
                    let record = response.get("record").unwrap_or(&response);
                    let metadata = retain_fields(
                        record,
                        &[
                            "provider_model",
                            "usage",
                            "cost",
                            "started_at",
                            "completed_at",
                            "status",
                        ],
                    );
                    let mut response = redacted.clone();
                    response["record"] = Value::Object(metadata);
                    response
                });
                sqlx::query("UPDATE model_calls SET request=?,response=?,error=? WHERE id=?")
                    .bind(serde_json::to_string(&request)?)
                    .bind(json_option(&response)?)
                    .bind(if call.get::<Option<&str>, _>("error").is_some() {
                        Some(serde_json::to_string(&error)?)
                    } else {
                        None
                    })
                    .bind(call.get::<String, _>("id"))
                    .execute(&mut *self.tx)
                    .await
                    .map_err(write_error)?;
            }
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
            sqlx::query("UPDATE structured_formation_runs SET state='cancelled',input=?,output=?,error=?,updated_at=strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE job_id=?")
                .bind(serde_json::to_string(&retired_input)?).bind(json_option(&output.map(|_|redacted.clone()))?).bind(serde_json::to_string(&error)?).bind(&job).execute(&mut *self.tx).await.map_err(write_error)?;
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

async fn source_dependencies(
    conn: &mut SqliteConnection,
    namespace: &NamespaceId,
    subject: &SubjectId,
    source: &EvidenceSource,
) -> Result<Vec<EvidenceSource>> {
    let Some(reference) = source.cognitive_ref() else {
        return Ok(vec![source.clone()]);
    };
    let kind = enum_name(&reference.kind)?;
    let rows=sqlx::query("WITH RECURSIVE walk(kind,id,revision) AS (SELECT ?,?,? UNION SELECT l.source_kind,l.source_id,l.source_revision FROM walk w JOIN structured_revisions r ON r.kind=w.kind AND r.object_id=w.id AND r.revision=w.revision JOIN structured_links l ON l.object_id=r.object_id AND l.revision=r.revision WHERE l.namespace_id=? AND l.subject_id=?) SELECT kind,id,revision FROM walk LIMIT 1025")
        .bind(kind).bind(reference.id).bind(reference.revision).bind(namespace.as_str()).bind(subject.as_str()).fetch_all(conn).await.map_err(storage_error)?;
    if rows.len() > 1024 {
        return Err(Error::new(
            ErrorCode::ResourceExhausted,
            "formation source closure exceeds 1024 references",
        ));
    }
    rows.into_iter()
        .map(|row| {
            let kind: String = row.get("kind");
            let id: String = row.get("id");
            let revision: Option<i64> = row.get("revision");
            Ok(match kind.as_str() {
                "material" => EvidenceSource::Material {
                    material_id: nous_domain::MaterialId(id),
                },
                "observation" => EvidenceSource::Observation {
                    observation_id: nous_domain::ObservationId(id),
                },
                "memory" => EvidenceSource::MemoryRevision {
                    memory_id: MemoryId(id),
                    revision: revision
                        .ok_or_else(|| Error::invalid("source revision is missing"))?,
                },
                _ => EvidenceSource::CognitiveRevision {
                    reference: CognitiveRef {
                        kind: parse_enum::<CognitiveKind>(&kind)?,
                        id,
                        revision: revision
                            .ok_or_else(|| Error::invalid("source revision is missing"))?,
                    },
                },
            })
        })
        .collect()
}

fn validate_groups(values: &[StructuredFormationGroup]) -> Result<()> {
    if values.len() > 64 {
        return Err(Error::invalid(
            "structured proposal must have at most 64 groups",
        ));
    }
    let mut keys = HashMap::new();
    let mut ordinals = HashSet::new();
    for value in values {
        nous_domain::validate_name(&value.key, "structured group key")?;
        if value.ordinal < 0
            || !ordinals.insert(value.ordinal)
            || keys.insert(value.key.as_str(), value).is_some()
            || value.state != StructuredGroupState::Pending
            || value.commit_seq.is_some()
            || value.result.is_some()
            || value.error.is_some()
        {
            return Err(Error::invalid(
                "new group needs a unique key/ordinal and an empty pending outcome",
            ));
        }
    }
    for value in values {
        let unique = value.depends_on.iter().collect::<HashSet<_>>();
        if unique.len() != value.depends_on.len()
            || value
                .depends_on
                .iter()
                .any(|key| key == &value.key || !keys.contains_key(key.as_str()))
        {
            return Err(Error::invalid("structured group has invalid dependencies"));
        }
    }
    let mut settled = HashSet::new();
    while settled.len() < values.len() {
        let before = settled.len();
        for value in values {
            if value
                .depends_on
                .iter()
                .all(|key| settled.contains(key.as_str()))
            {
                settled.insert(value.key.as_str());
            }
        }
        if settled.len() == before {
            return Err(Error::invalid(
                "structured group dependencies contain a cycle",
            ));
        }
    }
    Ok(())
}

async fn run(conn: &mut SqliteConnection, id: &str) -> Result<StructuredFormationRun> {
    let row = sqlx::query("SELECT * FROM structured_formation_runs WHERE job_id=?")
        .bind(id)
        .fetch_optional(conn)
        .await
        .map_err(storage_error)?
        .ok_or_else(not_found)?;
    Ok(StructuredFormationRun {
        job_id: row.get("job_id"),
        namespace_id: NamespaceId(row.get("namespace_id")),
        subject_id: SubjectId(row.get("subject_id")),
        state: parse_enum(row.get("state"))?,
        input: serde_json::from_str(row.get("input"))?,
        output: parse_optional(row.get("output"))?,
        model_call_id: row
            .get::<Option<String>, _>("model_call_id")
            .map(ModelCallId),
        error: parse_optional(row.get("error"))?,
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    })
}
async fn groups(conn: &mut SqliteConnection, id: &str) -> Result<Vec<StructuredFormationGroup>> {
    sqlx::query("SELECT * FROM structured_formation_groups WHERE job_id=? ORDER BY ordinal,key")
        .bind(id)
        .fetch_all(conn)
        .await
        .map_err(storage_error)?
        .into_iter()
        .map(group_row)
        .collect()
}
fn group_row(row: SqliteRow) -> Result<StructuredFormationGroup> {
    Ok(StructuredFormationGroup {
        key: row.get("key"),
        ordinal: row.get("ordinal"),
        depends_on: serde_json::from_str(row.get("depends_on"))?,
        state: parse_enum(row.get("state"))?,
        result: parse_optional(row.get("result"))?,
        error: parse_optional(row.get("error"))?,
        commit_seq: row.get("commit_seq"),
    })
}
fn enum_name<T: Serialize>(value: &T) -> Result<String> {
    serde_json::to_value(value)?
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| Error::invalid("expected enum name"))
}
fn parse_enum<T: serde::de::DeserializeOwned>(value: &str) -> Result<T> {
    Ok(serde_json::from_value(Value::String(value.into()))?)
}
fn json_option<T: Serialize>(value: &Option<T>) -> Result<Option<String>> {
    value
        .as_ref()
        .map(serde_json::to_string)
        .transpose()
        .map_err(storage_error)
}
fn parse_optional<T: serde::de::DeserializeOwned>(value: Option<&str>) -> Result<Option<T>> {
    value
        .map(serde_json::from_str)
        .transpose()
        .map_err(storage_error)
}
