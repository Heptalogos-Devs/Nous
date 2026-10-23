//! Formation records and memory revisions use the same authority transaction
//! as their source objects, operation receipts, and durable queue messages.
use nous_domain::{
    Error, ErrorCode, FormationRun, Memory, MemoryId, ModelCall, ModelCallId, NamespaceId,
    ObservationId, Result, SubjectId,
};
use serde::{Deserialize, Serialize};
use sqlx::{Row, SqliteConnection, sqlite::SqliteRow};

use crate::{
    BeginOperation, Database, JOB_QUEUE, OperationKey, WriteTransaction, not_found, storage_error,
    write_error,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryBatch {
    pub through_commit_seq: i64,
    pub memories: Vec<Memory>,
    pub has_more: bool,
}

impl Database {
    pub async fn formation(&self, job_id: &str) -> Result<FormationRun> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let run = formation(&mut tx, job_id).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(run)
    }

    pub async fn model_call(&self, id: &ModelCallId) -> Result<ModelCall> {
        model_call(&mut *self.pool.acquire().await.map_err(storage_error)?, id).await
    }

    pub async fn model_calls(&self, job_id: &str) -> Result<Vec<ModelCall>> {
        sqlx::query("SELECT * FROM model_calls WHERE job_id=? ORDER BY attempt,id")
            .bind(job_id)
            .fetch_all(&self.pool)
            .await
            .map_err(storage_error)?
            .into_iter()
            .map(model_call_row)
            .collect()
    }

    pub async fn memory_namespace(&self, id: &MemoryId) -> Result<NamespaceId> {
        sqlx::query_scalar::<_, String>("SELECT namespace_id FROM memories WHERE id=?")
            .bind(id.as_str())
            .fetch_optional(&self.pool)
            .await
            .map_err(storage_error)?
            .map(NamespaceId)
            .ok_or_else(not_found)
    }

    pub async fn observation_namespace(&self, id: &ObservationId) -> Result<NamespaceId> {
        sqlx::query_scalar::<_, String>("SELECT namespace_id FROM observations WHERE id=?")
            .bind(id.as_str())
            .fetch_optional(&self.pool)
            .await
            .map_err(storage_error)?
            .map(NamespaceId)
            .ok_or_else(not_found)
    }

    pub async fn memory(&self, namespace: &NamespaceId, id: &MemoryId) -> Result<Memory> {
        memory(
            &mut *self.pool.acquire().await.map_err(storage_error)?,
            namespace,
            id,
        )
        .await
    }

    pub async fn memory_history(
        &self,
        namespace: &NamespaceId,
        id: &MemoryId,
    ) -> Result<Vec<Memory>> {
        let rows = sqlx::query(
            "SELECT * FROM memory_revisions WHERE namespace_id=? AND memory_id=? ORDER BY revision",
        )
        .bind(namespace.as_str())
        .bind(id.as_str())
        .fetch_all(&self.pool)
        .await
        .map_err(storage_error)?;
        if rows.is_empty() {
            return Err(not_found());
        }
        rows.into_iter().map(memory_row).collect()
    }

    pub async fn memory_changes(&self, after_seq: i64, commit_limit: usize) -> Result<MemoryBatch> {
        self.changes(None, after_seq, commit_limit).await
    }

    pub async fn subject_memory_changes(
        &self,
        namespace: &NamespaceId,
        subject: &SubjectId,
        after_seq: i64,
        commit_limit: usize,
    ) -> Result<MemoryBatch> {
        self.changes(Some((namespace, subject)), after_seq, commit_limit)
            .await
    }

    async fn changes(
        &self,
        scope: Option<(&NamespaceId, &SubjectId)>,
        after_seq: i64,
        commit_limit: usize,
    ) -> Result<MemoryBatch> {
        let limit = i64::try_from(commit_limit)
            .ok()
            .and_then(|v| v.checked_add(1))
            .filter(|_| commit_limit > 0 && after_seq >= 0)
            .ok_or_else(|| {
                Error::invalid(
                    "change cursor must be nonnegative and commit limit must be positive",
                )
            })?;
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        // This read establishes the WAL snapshot used by both pagination and
        // materialization; a concurrent commit is visible on the next batch.
        let upper: i64 =
            sqlx::query_scalar("SELECT commit_seq FROM authority_state WHERE singleton=1")
                .fetch_one(&mut *tx)
                .await
                .map_err(storage_error)?;
        if after_seq > upper {
            return Err(Error::new(
                ErrorCode::Conflict,
                "change cursor is ahead of authority",
            ));
        }
        let (namespace, subject) = scope
            .map(|(ns, subject)| (Some(ns.as_str()), Some(subject.as_str())))
            .unwrap_or((None, None));
        let sequences: Vec<i64> = sqlx::query_scalar("SELECT DISTINCT commit_seq FROM memory_revisions WHERE commit_seq>? AND commit_seq<=? AND (? IS NULL OR namespace_id=?) AND (? IS NULL OR subject_id=?) ORDER BY commit_seq LIMIT ?")
            .bind(after_seq).bind(upper).bind(namespace).bind(namespace).bind(subject).bind(subject).bind(limit)
            .fetch_all(&mut *tx).await.map_err(storage_error)?;
        let has_more = sequences.len() > commit_limit;
        let through = if has_more {
            sequences[commit_limit - 1]
        } else {
            upper
        };
        let rows = sqlx::query("SELECT r.* FROM memory_revisions r WHERE r.commit_seq>? AND r.commit_seq<=? AND (? IS NULL OR r.namespace_id=?) AND (? IS NULL OR r.subject_id=?) AND r.revision=(SELECT max(latest.revision) FROM memory_revisions latest WHERE latest.memory_id=r.memory_id AND latest.commit_seq<=?) ORDER BY r.commit_seq,r.memory_id")
            .bind(after_seq).bind(through).bind(namespace).bind(namespace).bind(subject).bind(subject).bind(through)
            .fetch_all(&mut *tx).await.map_err(storage_error)?;
        let memories = rows
            .into_iter()
            .map(memory_row)
            .collect::<Result<Vec<_>>>()?;
        tx.commit().await.map_err(storage_error)?;
        Ok(MemoryBatch {
            through_commit_seq: through,
            memories,
            has_more,
        })
    }

    /// Called before workers start while the application owns the instance lock.
    /// A waiting formation is retried with its original queue identity.
    pub async fn resume_waiting_formations(&self) -> Result<u64> {
        let key = OperationKey {
            caller_id: "nous.recovery".into(),
            operation: "formation.resume_waiting".into(),
            scope: "instance".into(),
            operation_id: ulid::Ulid::generate().to_string(),
            request_hash: "nous.formation.resume_waiting.v1".into(),
        };
        let BeginOperation::New(mut tx) = self.begin_operation(key).await? else {
            unreachable!("fresh recovery operation")
        };
        let ids: Vec<String> = sqlx::query_scalar("SELECT f.job_id FROM formation_runs f JOIN Jobs j ON j.id=f.job_id WHERE f.state IN ('waiting_for_configuration','waiting_for_budget') AND j.status IN ('Done','Failed','Killed') ORDER BY f.job_id")
            .fetch_all(&mut *tx.tx).await.map_err(storage_error)?;
        if ids.is_empty() {
            tx.rollback().await?;
            return Ok(0);
        }
        for id in &ids {
            sqlx::query("UPDATE Jobs SET status='Pending',attempts=0,lock_by=NULL,lock_at=NULL,done_at=NULL,last_result=NULL,run_at=strftime('%s','now') WHERE id=?")
                .bind(id).execute(&mut *tx.tx).await.map_err(write_error)?;
            sqlx::query("UPDATE formation_runs SET state='queued',error=NULL,updated_at=strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE job_id=?")
                .bind(id).execute(&mut *tx.tx).await.map_err(write_error)?;
        }
        let count = ids.len() as u64;
        tx.commit(serde_json::json!({"resumed":count})).await?;
        Ok(count)
    }

    /// Called only at host startup, before any worker exists, while the caller
    /// holds the instance's exclusive process lock. Domain outcomes are retained;
    /// the worker handles uncertain external calls before making another request.
    pub async fn recover_interrupted_jobs(&self) -> Result<u64> {
        let key = OperationKey {
            caller_id: "nous.recovery".into(),
            operation: "job.recover_interrupted".into(),
            scope: "instance".into(),
            operation_id: ulid::Ulid::generate().to_string(),
            request_hash: "nous.job.recover_interrupted.v1".into(),
        };
        let BeginOperation::New(mut tx) = self.begin_operation(key).await? else {
            unreachable!("fresh recovery operation")
        };
        let workers: i64 = sqlx::query_scalar("SELECT count(*) FROM Workers WHERE worker_type=?")
            .bind(JOB_QUEUE)
            .fetch_one(&mut *tx.tx)
            .await
            .map_err(storage_error)?;
        let recovered = sqlx::query("UPDATE Jobs SET status='Pending',attempts=attempts+1,done_at=NULL,run_at=strftime('%s','now') WHERE job_type=? AND status IN ('Running','Queued')")
            .bind(JOB_QUEUE).execute(&mut *tx.tx).await.map_err(write_error)?.rows_affected();
        // ACKed jobs retain their worker reference upstream. Clear those
        // references too before removing the departed host's worker rows.
        sqlx::query(
            "UPDATE Jobs SET lock_by=NULL,lock_at=NULL WHERE job_type=? AND lock_by IS NOT NULL",
        )
        .bind(JOB_QUEUE)
        .execute(&mut *tx.tx)
        .await
        .map_err(write_error)?;
        sqlx::query("DELETE FROM Workers WHERE worker_type=?")
            .bind(JOB_QUEUE)
            .execute(&mut *tx.tx)
            .await
            .map_err(write_error)?;
        if recovered == 0 && workers == 0 {
            tx.rollback().await?;
            return Ok(0);
        }
        tx.commit(serde_json::json!({"recovered_jobs":recovered,"departed_workers":workers}))
            .await?;
        Ok(recovered)
    }
}

impl WriteTransaction {
    pub async fn formation(&mut self, job_id: &str) -> Result<FormationRun> {
        formation(&mut self.tx, job_id).await
    }

    pub async fn model_call(&mut self, id: &ModelCallId) -> Result<ModelCall> {
        model_call(&mut self.tx, id).await
    }

    pub async fn memory(&mut self, namespace: &NamespaceId, id: &MemoryId) -> Result<Memory> {
        memory(&mut self.tx, namespace, id).await
    }

    pub async fn insert_formation(&mut self, run: &FormationRun) -> Result<()> {
        sqlx::query("INSERT INTO formation_runs(job_id,namespace_id,subject_id,observation_id,material_id,subject_revision,material_hash,state,model_call_id,error,created_at,updated_at) VALUES (?,?,?,?,?,?,?,?,?,?,?,?)")
            .bind(&run.job_id).bind(run.namespace_id.as_str()).bind(run.subject_id.as_str()).bind(run.observation_id.as_str()).bind(run.material_id.as_str())
            .bind(run.subject_revision).bind(&run.material_hash).bind(enum_name(&run.state)?).bind(run.model_call_id.as_ref().map(ModelCallId::as_str))
            .bind(json_option(&run.error)?).bind(&run.created_at).bind(&run.updated_at)
            .execute(&mut *self.tx).await.map_err(write_error)?;
        self.replace_formation_memories(run).await
    }

    pub async fn update_formation(&mut self, run: &FormationRun) -> Result<()> {
        let changed = sqlx::query("UPDATE formation_runs SET state=?,model_call_id=?,error=?,updated_at=? WHERE job_id=? AND namespace_id=? AND subject_id=? AND observation_id=? AND material_id=? AND subject_revision=? AND material_hash=?")
            .bind(enum_name(&run.state)?).bind(run.model_call_id.as_ref().map(ModelCallId::as_str)).bind(json_option(&run.error)?).bind(&run.updated_at)
            .bind(&run.job_id).bind(run.namespace_id.as_str()).bind(run.subject_id.as_str()).bind(run.observation_id.as_str()).bind(run.material_id.as_str()).bind(run.subject_revision).bind(&run.material_hash)
            .execute(&mut *self.tx).await.map_err(write_error)?.rows_affected();
        if changed == 0 {
            return Err(Error::new(
                ErrorCode::Conflict,
                "formation identity or source version does not match",
            ));
        }
        self.replace_formation_memories(run).await
    }

    async fn replace_formation_memories(&mut self, run: &FormationRun) -> Result<()> {
        sqlx::query("DELETE FROM formation_memories WHERE job_id=?")
            .bind(&run.job_id)
            .execute(&mut *self.tx)
            .await
            .map_err(write_error)?;
        for (ordinal, id) in run.memory_ids.iter().enumerate() {
            let memory = self.memory(&run.namespace_id, id).await?;
            if memory.subject_id != run.subject_id {
                return Err(Error::invalid(
                    "formation memory belongs to a different subject",
                ));
            }
            sqlx::query("INSERT INTO formation_memories(job_id,memory_id,ordinal) VALUES (?,?,?)")
                .bind(&run.job_id)
                .bind(id.as_str())
                .bind(ordinal as i64)
                .execute(&mut *self.tx)
                .await
                .map_err(write_error)?;
        }
        Ok(())
    }

    /// Every persisted call consumes one attempt, including failed and uncertain
    /// network outcomes. Call IDs and (job, attempt) are unique reservations.
    pub async fn daily_model_calls(&mut self, resource_id: &str, date_prefix: &str) -> Result<i64> {
        sqlx::query_scalar(
            "SELECT count(*) FROM model_calls WHERE resource_id=? AND substr(created_at,1,10)=?",
        )
        .bind(resource_id)
        .bind(date_prefix)
        .fetch_one(&mut *self.tx)
        .await
        .map_err(storage_error)
    }

    pub async fn insert_model_call(&mut self, call: &ModelCall) -> Result<()> {
        sqlx::query("INSERT INTO model_calls(id,job_id,namespace_id,subject_id,attempt,resource_id,request,response,state,error,cost_microunits,created_at,completed_at) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?)")
            .bind(call.id.as_str()).bind(&call.job_id).bind(call.namespace_id.as_str()).bind(call.subject_id.as_str()).bind(call.attempt).bind(&call.resource_id)
            .bind(serde_json::to_string(&call.request).map_err(storage_error)?).bind(json_option(&call.response)?).bind(enum_name(&call.state)?).bind(json_option(&call.error)?)
            .bind(call.cost_microunits).bind(&call.created_at).bind(&call.completed_at)
            .execute(&mut *self.tx).await.map_err(write_error)?;
        Ok(())
    }

    pub async fn update_model_call(&mut self, call: &ModelCall) -> Result<()> {
        let previous = self.model_call(&call.id).await?;
        if previous.request != call.request
            || previous.resource_id != call.resource_id
            || previous.attempt != call.attempt
            || previous.job_id != call.job_id
            || previous.namespace_id != call.namespace_id
            || previous.subject_id != call.subject_id
            || previous.created_at != call.created_at
        {
            return Err(Error::new(
                ErrorCode::Conflict,
                "model call request and reservation identity are immutable",
            ));
        }
        sqlx::query("UPDATE model_calls SET response=?,state=?,error=?,cost_microunits=?,completed_at=? WHERE id=?")
            .bind(json_option(&call.response)?).bind(enum_name(&call.state)?).bind(json_option(&call.error)?).bind(call.cost_microunits).bind(&call.completed_at).bind(call.id.as_str())
            .execute(&mut *self.tx).await.map_err(write_error)?;
        Ok(())
    }

    /// `None` creates revision 1. `Some(n)` appends revision n+1 and moves the
    /// current pointer only when n is still current; all older snapshots remain.
    pub async fn save_memory(
        &mut self,
        value: &Memory,
        expected_revision: Option<i64>,
    ) -> Result<()> {
        self.check_sequence(value.commit_seq)?;
        let revision = expected_revision
            .unwrap_or(0)
            .checked_add(1)
            .filter(|n| *n > 0)
            .ok_or_else(|| Error::invalid("memory revision is out of range"))?;
        if value.revision != revision {
            return Err(Error::invalid(
                "memory revision must follow expected_revision",
            ));
        }
        match expected_revision {
            None => {
                sqlx::query("INSERT INTO memories(id,namespace_id,subject_id,current_revision,commit_seq) VALUES (?,?,?,?,?)")
                    .bind(value.id.as_str()).bind(value.namespace_id.as_str()).bind(value.subject_id.as_str()).bind(value.revision).bind(value.commit_seq)
                    .execute(&mut *self.tx).await.map_err(write_error)?;
            }
            Some(expected) => {
                let changed = sqlx::query("UPDATE memories SET current_revision=?,commit_seq=? WHERE id=? AND namespace_id=? AND subject_id=? AND current_revision=?")
                    .bind(value.revision).bind(value.commit_seq).bind(value.id.as_str()).bind(value.namespace_id.as_str()).bind(value.subject_id.as_str()).bind(expected)
                    .execute(&mut *self.tx).await.map_err(write_error)?.rows_affected();
                if changed == 0 {
                    return Err(Error::new(
                        ErrorCode::Conflict,
                        "memory revision or scope has changed",
                    ));
                }
            }
        }
        sqlx::query("INSERT INTO memory_revisions(memory_id,namespace_id,subject_id,revision,title,content,epistemic_status,observation_id,material_id,model_call_id,prompt_version,times,recorded_at,deleted,commit_seq) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)")
            .bind(value.id.as_str()).bind(value.namespace_id.as_str()).bind(value.subject_id.as_str()).bind(value.revision).bind(&value.title).bind(&value.content).bind(&value.epistemic_status)
            .bind(value.observation_id.as_str()).bind(value.material_id.as_str()).bind(value.model_call_id.as_str()).bind(&value.prompt_version)
            .bind(serde_json::to_string(&value.times).map_err(storage_error)?).bind(&value.recorded_at).bind(value.deleted).bind(value.commit_seq)
            .execute(&mut *self.tx).await.map_err(write_error)?;
        Ok(())
    }
}

async fn formation(connection: &mut SqliteConnection, job_id: &str) -> Result<FormationRun> {
    let row = sqlx::query("SELECT * FROM formation_runs WHERE job_id=?")
        .bind(job_id)
        .fetch_optional(&mut *connection)
        .await
        .map_err(storage_error)?
        .ok_or_else(not_found)?;
    let memory_ids: Vec<String> = sqlx::query_scalar(
        "SELECT memory_id FROM formation_memories WHERE job_id=? ORDER BY ordinal",
    )
    .bind(job_id)
    .fetch_all(connection)
    .await
    .map_err(storage_error)?;
    Ok(FormationRun {
        job_id: row.get("job_id"),
        namespace_id: NamespaceId(row.get("namespace_id")),
        subject_id: SubjectId(row.get("subject_id")),
        observation_id: ObservationId(row.get("observation_id")),
        material_id: nous_domain::MaterialId(row.get("material_id")),
        subject_revision: row.get("subject_revision"),
        material_hash: row.get("material_hash"),
        state: parse_enum(row.get("state"))?,
        model_call_id: row
            .get::<Option<String>, _>("model_call_id")
            .map(ModelCallId),
        memory_ids: memory_ids.into_iter().map(MemoryId).collect(),
        error: parse_optional(row.get("error"))?,
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    })
}

async fn model_call(connection: &mut SqliteConnection, id: &ModelCallId) -> Result<ModelCall> {
    model_call_row(
        sqlx::query("SELECT * FROM model_calls WHERE id=?")
            .bind(id.as_str())
            .fetch_optional(connection)
            .await
            .map_err(storage_error)?
            .ok_or_else(not_found)?,
    )
}

fn model_call_row(row: SqliteRow) -> Result<ModelCall> {
    Ok(ModelCall {
        id: ModelCallId(row.get("id")),
        job_id: row.get("job_id"),
        namespace_id: NamespaceId(row.get("namespace_id")),
        subject_id: SubjectId(row.get("subject_id")),
        attempt: row.get("attempt"),
        resource_id: row.get("resource_id"),
        request: serde_json::from_str(row.get("request")).map_err(storage_error)?,
        response: parse_optional(row.get("response"))?,
        state: parse_enum(row.get("state"))?,
        error: parse_optional(row.get("error"))?,
        cost_microunits: row.get("cost_microunits"),
        created_at: row.get("created_at"),
        completed_at: row.get("completed_at"),
    })
}

async fn memory(
    connection: &mut SqliteConnection,
    namespace: &NamespaceId,
    id: &MemoryId,
) -> Result<Memory> {
    memory_row(sqlx::query("SELECT r.* FROM memory_revisions r JOIN memories m ON m.id=r.memory_id AND m.current_revision=r.revision WHERE m.namespace_id=? AND m.id=?")
        .bind(namespace.as_str()).bind(id.as_str()).fetch_optional(connection).await.map_err(storage_error)?.ok_or_else(not_found)?)
}

fn memory_row(row: SqliteRow) -> Result<Memory> {
    Ok(Memory {
        id: MemoryId(row.get("memory_id")),
        namespace_id: NamespaceId(row.get("namespace_id")),
        subject_id: SubjectId(row.get("subject_id")),
        revision: row.get("revision"),
        title: row.get("title"),
        content: row.get("content"),
        epistemic_status: row.get("epistemic_status"),
        observation_id: ObservationId(row.get("observation_id")),
        material_id: nous_domain::MaterialId(row.get("material_id")),
        model_call_id: ModelCallId(row.get("model_call_id")),
        prompt_version: row.get("prompt_version"),
        times: serde_json::from_str(row.get("times")).map_err(storage_error)?,
        recorded_at: row.get("recorded_at"),
        deleted: row.get("deleted"),
        commit_seq: row.get("commit_seq"),
    })
}

fn enum_name<T: Serialize>(value: &T) -> Result<String> {
    serde_json::to_value(value)
        .map_err(storage_error)?
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| Error::internal("stored state must serialize to a string"))
}

fn parse_enum<T: for<'de> Deserialize<'de>>(value: &str) -> Result<T> {
    serde_json::from_value(serde_json::Value::String(value.into())).map_err(storage_error)
}

fn json_option<T: Serialize>(value: &Option<T>) -> Result<Option<String>> {
    value
        .as_ref()
        .map(serde_json::to_string)
        .transpose()
        .map_err(storage_error)
}

fn parse_optional<T: for<'de> Deserialize<'de>>(value: Option<&str>) -> Result<Option<T>> {
    value
        .map(serde_json::from_str)
        .transpose()
        .map_err(storage_error)
}
