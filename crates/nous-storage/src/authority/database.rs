use crate::{not_found, storage_error};
use apalis_sqlite::SqliteStorage;
use nous_domain::*;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::Row;
use sqlx::{
    SqlitePool,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous},
};
use std::{path::Path, time::Duration};

/// Durable work envelope used by all role owners.
pub const JOB_QUEUE: &str = "nous.work";

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OperationKey {
    pub context: CallContext,
    pub operation: String,
    pub scope: String,
    pub operation_id: String,
    pub request_hash: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OperationReceipt {
    pub operation_id: String,
    pub context: CallContext,
    pub time: nous_domain::OperationTime,
    pub commit_seq: i64,
    pub result: Value,
    pub job_ids: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JobEnvelope {
    pub id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct QueueRecord {
    pub id: String,
    pub status: String,
    pub attempts: i64,
    pub max_attempts: i64,
    pub last_result: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StorageStatus {
    pub commit_seq: i64,
    pub namespaces: i64,
    pub subjects: i64,
    pub materials: i64,
    pub observations: i64,
    pub pending_jobs: i64,
    pub running_jobs: i64,
    pub failed_jobs: i64,
}

/// A pool configured for local, durable WAL storage. The instance owner holds
/// the process lock; this component owns schema and write transactions.
#[derive(Clone, Debug)]
pub struct Database {
    pub(crate) pool: SqlitePool,
    pub(crate) operation_time: fn() -> nous_domain::OperationTime,
}

impl Database {
    pub async fn open_read_only(
        path: impl AsRef<Path>,
        operation_time: fn() -> nous_domain::OperationTime,
    ) -> Result<Self> {
        let path = path.as_ref();
        if !path.is_absolute() {
            return Err(Error::invalid("database path must be absolute"));
        }
        let options = SqliteConnectOptions::new()
            .filename(path)
            .read_only(true)
            .foreign_keys(true)
            .busy_timeout(Duration::from_secs(5));
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await
            .map_err(storage_error)?;
        Ok(Self {
            pool,
            operation_time,
        })
    }

    pub async fn open(
        path: impl AsRef<Path>,
        operation_time: fn() -> nous_domain::OperationTime,
    ) -> Result<Self> {
        let path = path.as_ref();
        if !path.is_absolute() {
            return Err(Error::new(
                ErrorCode::InvalidArgument,
                "database path must be absolute",
            ));
        }
        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .foreign_keys(true)
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Full)
            .busy_timeout(Duration::from_secs(5));
        let pool = SqlitePoolOptions::new()
            .max_connections(4)
            .connect_with(options)
            .await
            .map_err(storage_error)?;
        // setup() in apalis-sqlite 1.0 disables synchronous durability.
        // Running its actual embedded migrations leaves our connection policy intact.
        SqliteStorage::migrations()
            .run(&pool)
            .await
            .map_err(storage_error)?;
        let mut tx = pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(storage_error)?;
        sqlx::raw_sql(include_str!("../schema.sql"))
            .execute(&mut *tx)
            .await
            .map_err(storage_error)?;
        sqlx::raw_sql(include_str!("../context/schema.sql"))
            .execute(&mut *tx)
            .await
            .map_err(storage_error)?;
        sqlx::raw_sql(include_str!("../bundle/schema.sql"))
            .execute(&mut *tx)
            .await
            .map_err(storage_error)?;
        sqlx::raw_sql(include_str!("../directory/schema.sql"))
            .execute(&mut *tx)
            .await
            .map_err(storage_error)?;
        sqlx::raw_sql(include_str!("../imports/schema.sql"))
            .execute(&mut *tx)
            .await
            .map_err(storage_error)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(Self {
            pool,
            operation_time,
        })
    }

    pub async fn close(&self) {
        self.pool.close().await;
    }

    pub async fn health(&self) -> Result<()> {
        sqlx::query("SELECT 1")
            .execute(&self.pool)
            .await
            .map_err(storage_error)?;
        Ok(())
    }

    pub fn queue_backend(&self, wake: std::sync::Arc<tokio::sync::Notify>) -> super::QueueBackend {
        super::QueueBackend::new(self.pool.clone(), wake)
    }

    pub async fn status(&self) -> Result<StorageStatus> {
        let row = sqlx::query("SELECT (SELECT commit_seq FROM authority_state WHERE singleton=1) AS commit_seq, (SELECT count(*) FROM namespaces) AS namespaces, (SELECT count(*) FROM subjects) AS subjects, (SELECT count(*) FROM materials) AS materials, (SELECT count(*) FROM observations) AS observations, (SELECT count(*) FROM Jobs WHERE status='Pending') AS pending_jobs, (SELECT count(*) FROM Jobs WHERE status IN ('Running','Queued')) AS running_jobs, (SELECT count(*) FROM Jobs WHERE status IN ('Failed','Killed')) AS failed_jobs")
            .fetch_one(&self.pool).await.map_err(storage_error)?;
        Ok(StorageStatus {
            commit_seq: row.get("commit_seq"),
            namespaces: row.get("namespaces"),
            subjects: row.get("subjects"),
            materials: row.get("materials"),
            observations: row.get("observations"),
            pending_jobs: row.get("pending_jobs"),
            running_jobs: row.get("running_jobs"),
            failed_jobs: row.get("failed_jobs"),
        })
    }

    pub async fn namespace(&self, id: &NamespaceId) -> Result<Namespace> {
        crate::authority::records::namespace(
            &mut *self.pool.acquire().await.map_err(storage_error)?,
            id,
        )
        .await?
        .ok_or_else(not_found)
    }

    pub async fn subject(&self, namespace: &NamespaceId, id: &SubjectId) -> Result<Subject> {
        crate::authority::records::subject(
            &mut *self.pool.acquire().await.map_err(storage_error)?,
            namespace,
            id,
        )
        .await?
        .ok_or_else(not_found)
    }

    pub async fn material(&self, namespace: &NamespaceId, id: &MaterialId) -> Result<Material> {
        crate::authority::records::material(
            &mut *self.pool.acquire().await.map_err(storage_error)?,
            namespace,
            id,
        )
        .await?
        .ok_or_else(not_found)
    }

    pub async fn observation(
        &self,
        namespace: &NamespaceId,
        id: &ObservationId,
    ) -> Result<Observation> {
        crate::authority::records::observation(
            &mut *self.pool.acquire().await.map_err(storage_error)?,
            namespace,
            id,
        )
        .await?
        .ok_or_else(not_found)
    }

    pub async fn material_namespace(&self, id: &MaterialId) -> Result<NamespaceId> {
        sqlx::query_scalar::<_, String>("SELECT namespace_id FROM materials WHERE id=?")
            .bind(id.as_str())
            .fetch_optional(&self.pool)
            .await
            .map_err(storage_error)?
            .map(NamespaceId)
            .ok_or_else(not_found)
    }

    pub async fn subject_namespace(&self, id: &SubjectId) -> Result<NamespaceId> {
        crate::authority::records::subject_namespace(
            &mut *self.pool.acquire().await.map_err(storage_error)?,
            id,
        )
        .await
    }

    pub async fn list_namespaces(&self) -> Result<Vec<Namespace>> {
        sqlx::query("SELECT * FROM namespaces ORDER BY commit_seq,id")
            .fetch_all(&self.pool)
            .await
            .map_err(storage_error)?
            .into_iter()
            .map(crate::authority::records::namespace_row)
            .collect()
    }

    pub async fn list_subjects(&self, namespace: &NamespaceId) -> Result<Vec<Subject>> {
        sqlx::query("SELECT * FROM subjects WHERE namespace_id=? ORDER BY commit_seq,id")
            .bind(namespace.as_str())
            .fetch_all(&self.pool)
            .await
            .map_err(storage_error)?
            .into_iter()
            .map(crate::authority::records::subject_row)
            .collect()
    }

    pub async fn list_materials(&self, namespace: &NamespaceId) -> Result<Vec<Material>> {
        sqlx::query("SELECT * FROM materials WHERE namespace_id=? ORDER BY commit_seq,id")
            .bind(namespace.as_str())
            .fetch_all(&self.pool)
            .await
            .map_err(storage_error)?
            .into_iter()
            .map(crate::authority::records::material_row)
            .collect()
    }

    pub async fn list_observations(&self, subject: &SubjectId) -> Result<Vec<Observation>> {
        sqlx::query("SELECT * FROM observations WHERE subject_id=? ORDER BY commit_seq,id")
            .bind(subject.as_str())
            .fetch_all(&self.pool)
            .await
            .map_err(storage_error)?
            .into_iter()
            .map(crate::authority::records::observation_row)
            .collect()
    }

    pub async fn queue_record(&self, id: &str) -> Result<Option<QueueRecord>> {
        crate::authority::queue_record(&mut *self.pool.acquire().await.map_err(storage_error)?, id)
            .await
    }
}
