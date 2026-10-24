use apalis::prelude::{TaskBuilder, TaskId};
use apalis_sqlite::queries::push_tasks;
use nous_domain::{
    Error, ErrorCode, Material, MaterialId, Namespace, NamespaceId, Observation, ObservationId,
    ObservationKind, Result, Subject, SubjectId,
};
use serde_json::Value;
use sqlx::{Row, Sqlite, Transaction};

use crate::{
    Database, JOB_QUEUE, JobEnvelope, OperationKey, OperationReceipt, not_found, storage_error,
    write_error,
};

#[derive(Debug)]
pub enum BeginOperation {
    Replay(OperationReceipt),
    New(WriteTransaction),
}

/// Owns the sole commit path for one application operation. Dropping it rolls
/// back authority changes, its commit sequence allocation, receipt, and jobs.
#[derive(Debug)]
pub struct WriteTransaction {
    pub(crate) tx: Transaction<'static, Sqlite>,
    key: OperationKey,
    commit_seq: i64,
    job_ids: Vec<String>,
}

impl Database {
    pub async fn begin_operation(&self, key: OperationKey) -> Result<BeginOperation> {
        // Acquire the writer before reading the receipt so concurrent retries
        // serialize and see the committed receipt rather than upgrade a snapshot.
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(storage_error)?;
        let stored = sqlx::query("SELECT request_hash, receipt FROM operation_receipts WHERE caller_id=? AND operation=? AND scope=? AND operation_id=?")
            .bind(&key.caller_id).bind(&key.operation).bind(&key.scope).bind(&key.operation_id)
            .fetch_optional(&mut *tx).await.map_err(storage_error)?;
        if let Some(row) = stored {
            if row.get::<String, _>("request_hash") != key.request_hash {
                return Err(Error::new(
                    ErrorCode::Conflict,
                    "operation ID was already used with a different request",
                ));
            }
            let receipt = serde_json::from_str(row.get("receipt")).map_err(storage_error)?;
            tx.rollback().await.map_err(storage_error)?;
            return Ok(BeginOperation::Replay(receipt));
        }
        let commit_seq = sqlx::query_scalar("UPDATE authority_state SET commit_seq=commit_seq+1 WHERE singleton=1 RETURNING commit_seq")
            .fetch_one(&mut *tx).await.map_err(storage_error)?;
        Ok(BeginOperation::New(WriteTransaction {
            tx,
            key,
            commit_seq,
            job_ids: vec![],
        }))
    }
}

impl WriteTransaction {
    pub fn commit_seq(&self) -> i64 {
        self.commit_seq
    }

    pub async fn namespace(&mut self, id: &NamespaceId) -> Result<Namespace> {
        crate::authority::records::namespace(&mut self.tx, id)
            .await?
            .ok_or_else(not_found)
    }

    pub async fn subject(&mut self, namespace: &NamespaceId, id: &SubjectId) -> Result<Subject> {
        crate::authority::records::subject(&mut self.tx, namespace, id)
            .await?
            .ok_or_else(not_found)
    }

    pub async fn material(&mut self, namespace: &NamespaceId, id: &MaterialId) -> Result<Material> {
        crate::authority::records::material(&mut self.tx, namespace, id)
            .await?
            .ok_or_else(not_found)
    }

    pub async fn observation(
        &mut self,
        namespace: &NamespaceId,
        id: &ObservationId,
    ) -> Result<Observation> {
        crate::authority::records::observation(&mut self.tx, namespace, id)
            .await?
            .ok_or_else(not_found)
    }

    pub async fn insert_namespace(&mut self, value: &Namespace) -> Result<()> {
        self.check_sequence(value.commit_seq)?;
        sqlx::query("INSERT INTO namespaces(id,name,revision,commit_seq) VALUES (?,?,?,?)")
            .bind(value.id.as_str())
            .bind(&value.name)
            .bind(value.revision)
            .bind(self.commit_seq)
            .execute(&mut *self.tx)
            .await
            .map_err(write_error)?;
        Ok(())
    }

    pub async fn insert_subject(&mut self, value: &Subject) -> Result<()> {
        self.check_sequence(value.commit_seq)?;
        sqlx::query("INSERT INTO subjects(id,namespace_id,name,capabilities,revision,commit_seq) VALUES (?,?,?,?,?,?)")
            .bind(value.id.as_str()).bind(value.namespace_id.as_str()).bind(&value.name)
            .bind(serde_json::to_string(&value.capabilities).map_err(storage_error)?).bind(value.revision).bind(self.commit_seq)
            .execute(&mut *self.tx).await.map_err(write_error)?;
        Ok(())
    }

    pub async fn insert_material(&mut self, value: &Material) -> Result<()> {
        self.check_sequence(value.commit_seq)?;
        sqlx::query("INSERT INTO materials(id,namespace_id,content_hash,media_type,byte_len,source,event_time_domain,event_time_coordinate,recorded_at,commit_seq) VALUES (?,?,?,?,?,?,?,?,?,?)")
            .bind(value.id.as_str()).bind(value.namespace_id.as_str()).bind(&value.content_hash).bind(&value.media_type).bind(value.byte_len)
            .bind(&value.source).bind(value.event_time.as_ref().map(|t| t.domain.as_str())).bind(value.event_time.as_ref().map(|t| t.coordinate))
            .bind(&value.recorded_at).bind(self.commit_seq).execute(&mut *self.tx).await.map_err(write_error)?;
        Ok(())
    }

    pub async fn insert_observation(&mut self, value: &Observation) -> Result<()> {
        self.check_sequence(value.commit_seq)?;
        let kind = match &value.kind {
            ObservationKind::Contact => "contact",
            ObservationKind::Import => "import",
        };
        sqlx::query("INSERT INTO observations(id,namespace_id,subject_id,material_id,observed_at_domain,observed_at_coordinate,recorded_at,commit_seq,kind) VALUES (?,?,?,?,?,?,?,?,?)")
            .bind(value.id.as_str()).bind(value.namespace_id.as_str()).bind(value.subject_id.as_str()).bind(value.material_id.as_str())
            .bind(value.observed_at.as_ref().map(|t| t.domain.as_str())).bind(value.observed_at.as_ref().map(|t| t.coordinate))
            .bind(&value.recorded_at).bind(self.commit_seq).bind(kind).execute(&mut *self.tx).await.map_err(write_error)?;
        Ok(())
    }

    pub(crate) async fn enqueue_task(&mut self, id: &str) -> Result<String> {
        super::tasks::validate_job_id(id)?;
        let task = TaskBuilder::new(serde_json::to_vec(&JobEnvelope { id: id.into() })?)
            .task_id(TaskId::from(id.to_owned()))
            .max_attempts(5)
            .build();
        push_tasks(&mut *self.tx, JOB_QUEUE, &[task])
            .await
            .map_err(|error| match error {
                apalis_sqlite::Error::Database(error) => write_error(error),
                error => storage_error(error),
            })?;
        Ok(id.into())
    }

    pub(crate) fn record_task(&mut self, id: &str) {
        self.job_ids.push(id.into());
    }

    pub async fn commit(mut self, result: Value) -> Result<OperationReceipt> {
        let receipt = OperationReceipt {
            operation_id: self.key.operation_id.clone(),
            commit_seq: self.commit_seq,
            result,
            job_ids: self.job_ids,
        };
        sqlx::query("INSERT INTO operation_receipts(caller_id,operation,scope,operation_id,request_hash,commit_seq,receipt) VALUES (?,?,?,?,?,?,?)")
            .bind(&self.key.caller_id).bind(&self.key.operation).bind(&self.key.scope).bind(&self.key.operation_id)
            .bind(&self.key.request_hash).bind(self.commit_seq).bind(serde_json::to_string(&receipt).map_err(storage_error)?)
            .execute(&mut *self.tx).await.map_err(write_error)?;
        self.tx.commit().await.map_err(storage_error)?;
        Ok(receipt)
    }

    pub async fn rollback(self) -> Result<()> {
        self.tx.rollback().await.map_err(storage_error)
    }

    pub(crate) fn check_sequence(&self, sequence: i64) -> Result<()> {
        if sequence != self.commit_seq {
            return Err(Error::new(
                ErrorCode::InvalidArgument,
                "object commit sequence must match its transaction",
            ));
        }
        Ok(())
    }
}
