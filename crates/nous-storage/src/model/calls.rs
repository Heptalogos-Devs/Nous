use crate::codec::*;
use crate::{Database, WriteTransaction, not_found, storage_error, write_error};
use nous_domain::*;
use serde_json::Value;
use sqlx::{Row, SqliteConnection, sqlite::SqliteRow};

impl Database {
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
}

impl Database {
    pub async fn model_step_calls(&self, job: &str, step: &str) -> Result<Vec<ModelCall>> {
        steps(
            &mut *self.pool.acquire().await.map_err(storage_error)?,
            job,
            step,
        )
        .await
    }
    pub async fn latest_model_call(&self, job: &str, step: &str) -> Result<Option<ModelCall>> {
        latest(
            &mut *self.pool.acquire().await.map_err(storage_error)?,
            job,
            step,
        )
        .await
    }
    pub async fn recover_interrupted_calls(&self) -> Result<u64> {
        let crate::BeginOperation::New(mut tx) = self
            .begin_operation(crate::OperationKey {
                caller_id: "nous.recovery".into(),
                trace_id: TraceId::new(),
                operation: "calls.recover".into(),
                scope: "instance".into(),
                operation_id: ulid::Ulid::generate().to_string(),
                request_hash: "calls.recover".into(),
            })
            .await?
        else {
            unreachable!("fresh recovery")
        };
        let error = Error::new(
            ErrorCode::DependencyUnavailable,
            "host stopped before the remote response was saved",
        );
        let processed_at = tx.time().recorded();
        let count=sqlx::query("UPDATE model_calls SET state='uncertain',error=json_set(?,'$.trace_id',trace_id),completed_at=? WHERE state='started'").bind(serde_json::to_string(&error)?).bind(processed_at).execute(&mut *tx.tx).await.map_err(write_error)?.rows_affected();
        if count == 0 {
            tx.rollback().await?;
        } else {
            tx.commit(serde_json::json!({"uncertain":count})).await?;
        }
        Ok(count)
    }
}
impl WriteTransaction {
    pub async fn model_step_calls(&mut self, job: &str, step: &str) -> Result<Vec<ModelCall>> {
        steps(&mut self.tx, job, step).await
    }
    pub async fn latest_model_call(&mut self, job: &str, step: &str) -> Result<Option<ModelCall>> {
        latest(&mut self.tx, job, step).await
    }
    pub(crate) async fn redact_job_calls(&mut self, job: &str, reason: &str) -> Result<()> {
        let calls = sqlx::query("SELECT id,request,response FROM model_calls WHERE job_id=?")
            .bind(job)
            .fetch_all(&mut *self.tx)
            .await
            .map_err(storage_error)?;
        for row in calls {
            let request: Value = serde_json::from_str(row.get("request"))?;
            let response: Option<Value> = parse_optional(row.get("response"))?;
            let mut metadata = serde_json::Map::new();
            for key in [
                "resource",
                "role",
                "step_key",
                "parameters",
                "prompt_digest",
            ] {
                if let Some(value) = request.get(key) {
                    metadata.insert(key.into(), value.clone());
                }
            }
            metadata.insert("redacted".into(), Value::Bool(true));
            metadata.insert("reason".into(), Value::String(reason.into()));
            let response = response.map(|response| {
                let record = response.get("record");
                let mut details = serde_json::Map::new();
                for key in [
                    "provider_model",
                    "usage",
                    "cost",
                    "started_at",
                    "completed_at",
                    "status",
                ] {
                    if let Some(value) = record.and_then(|record| record.get(key)) {
                        details.insert(key.into(), value.clone());
                    }
                }
                serde_json::json!({"redacted":true,"reason":reason,"record":details})
            });
            sqlx::query("UPDATE model_calls SET request=?,response=? WHERE id=?")
                .bind(serde_json::to_string(&metadata)?)
                .bind(json_option(&response)?)
                .bind(row.get::<String, _>("id"))
                .execute(&mut *self.tx)
                .await
                .map_err(write_error)?;
        }
        Ok(())
    }

    pub async fn model_call(&mut self, id: &ModelCallId) -> Result<ModelCall> {
        model_call(&mut self.tx, id).await
    }

    pub(crate) async fn insert_model_call(&mut self, call: &ModelCall) -> Result<()> {
        call.trace_id.validate()?;
        let job = self.job_record(&call.job_id).await?;
        if call.trace_id != job.trace_id {
            return Err(
                Error::invalid("model call trace differs from its accepted task")
                    .with_trace(self.trace_id().clone()),
            );
        }
        sqlx::query("INSERT INTO model_calls(id,job_id,namespace_id,subject_id,step_key,attempt,resource_id,request,response,state,error,usage,cost,created_at,completed_at,trace_id,reservation_commit_seq) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)")
            .bind(call.id.as_str()).bind(&call.job_id).bind(call.namespace_id.as_str()).bind(call.subject_id.as_str()).bind(&call.step_key).bind(call.attempt).bind(&call.resource_id)
            .bind(serde_json::to_string(&call.request).map_err(storage_error)?).bind(json_option(&call.response)?).bind(enum_name(&call.state)?).bind(json_option(&call.error)?)
            .bind(json_option(&call.usage)?).bind(json_option(&call.cost)?).bind(&call.created_at).bind(&call.completed_at).bind(call.trace_id.as_str()).bind(self.commit_seq())
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
            || previous.step_key != call.step_key
            || previous.trace_id != call.trace_id
        {
            return Err(Error::new(
                ErrorCode::Conflict,
                "model call request and reservation identity are immutable",
            ));
        }
        sqlx::query("UPDATE model_calls SET response=?,state=?,error=?,usage=?,cost=?,completed_at=? WHERE id=?")
            .bind(json_option(&call.response)?).bind(enum_name(&call.state)?).bind(json_option(&call.error)?).bind(json_option(&call.usage)?).bind(json_option(&call.cost)?).bind(&call.completed_at).bind(call.id.as_str())
            .execute(&mut *self.tx).await.map_err(write_error)?;
        Ok(())
    }
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
        trace_id: TraceId(row.get("trace_id")),
        job_id: row.get("job_id"),
        step_key: row.get("step_key"),
        namespace_id: NamespaceId(row.get("namespace_id")),
        subject_id: SubjectId(row.get("subject_id")),
        attempt: row.get("attempt"),
        resource_id: row.get("resource_id"),
        request: serde_json::from_str(row.get("request")).map_err(storage_error)?,
        response: parse_optional(row.get("response"))?,
        state: parse_enum(row.get("state"))?,
        error: parse_optional(row.get("error"))?,
        usage: parse_optional(row.get("usage"))?,
        cost: parse_optional(row.get("cost"))?,
        created_at: row.get("created_at"),
        completed_at: row.get("completed_at"),
    })
}

async fn steps(conn: &mut SqliteConnection, job: &str, step: &str) -> Result<Vec<ModelCall>> {
    sqlx::query("SELECT * FROM model_calls WHERE job_id=? AND step_key=? ORDER BY attempt,id")
        .bind(job)
        .bind(step)
        .fetch_all(conn)
        .await
        .map_err(storage_error)?
        .into_iter()
        .map(model_call_row)
        .collect()
}
async fn latest(conn: &mut SqliteConnection, job: &str, step: &str) -> Result<Option<ModelCall>> {
    sqlx::query("SELECT * FROM model_calls WHERE job_id=? AND step_key=? ORDER BY attempt DESC,id DESC LIMIT 1").bind(job).bind(step).fetch_optional(conn).await.map_err(storage_error)?.map(model_call_row).transpose()
}
