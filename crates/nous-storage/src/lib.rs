//! SQLite authority and durable jobs share one short transaction.
//!
//! Queue integration is adapted from apalis-sqlite `src/persist.rs` and
//! `examples/basic.rs`, commit 9c87189360b0e212b23821a1a177e9cbe2ad1d4a.
//! The upstream MIT license is retained in LICENSE.apalis.

mod backup;
mod cognition;
mod embeddings;
mod management;
mod records;
mod transaction;

use std::{collections::HashSet, path::Path, time::Duration};

use apalis_sqlite::{Config, SqliteStorage};
use nous_domain::{
    Error, ErrorCode, Material, MaterialId, Namespace, NamespaceId, Observation, ObservationId,
    Result, Subject, SubjectId,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{
    Row, SqlitePool,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous},
};

pub use backup::ObjectReference;
pub use cognition::MemoryBatch;
pub use embeddings::{EmbeddingClaim, RetrievalEntry, RetrievalSnapshot};
pub use management::{MaintenancePage, PurgedMemory};
pub use transaction::{BeginOperation, WriteTransaction};

/// Versioned wire contract consumed by the worker, independent of Rust type names.
pub const JOB_QUEUE: &str = "nous.work.v1";

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OperationKey {
    pub caller_id: String,
    pub operation: String,
    pub scope: String,
    pub operation_id: String,
    pub request_hash: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OperationReceipt {
    pub operation_id: String,
    pub commit_seq: i64,
    pub result: Value,
    pub job_ids: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JobEnvelope {
    pub id: String,
    pub kind: String,
    pub namespace_id: String,
    pub subject_id: Option<String>,
    pub expected_revision: Option<i64>,
    pub payload: Value,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JobRecord {
    pub id: String,
    pub kind: String,
    pub status: String,
    pub attempts: i64,
    pub max_attempts: i64,
    pub last_result: Option<String>,
    pub envelope: JobEnvelope,
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
    pool: SqlitePool,
}

impl Database {
    pub async fn open(path: impl AsRef<Path>) -> Result<Self> {
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
        let queue = SqliteStorage::migrations();
        let authority = sqlx::migrate!("./migrations");
        let migrations = queue
            .iter()
            .chain(authority.iter())
            .cloned()
            .collect::<Vec<_>>();
        let mut versions = HashSet::new();
        if migrations
            .iter()
            .any(|migration| !versions.insert(migration.version))
        {
            return Err(Error::new(
                ErrorCode::Internal,
                "queue and authority migration versions overlap",
            ));
        }
        sqlx::migrate::Migrator::with_migrations(migrations)
            .run(&pool)
            .await
            .map_err(storage_error)?;
        Ok(Self { pool })
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

    pub fn queue_backend(&self) -> SqliteStorage<JobEnvelope> {
        SqliteStorage::new(&self.pool).with_config(Config::default().queue(JOB_QUEUE))
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
        records::namespace(&mut *self.pool.acquire().await.map_err(storage_error)?, id)
            .await?
            .ok_or_else(not_found)
    }

    pub async fn subject(&self, namespace: &NamespaceId, id: &SubjectId) -> Result<Subject> {
        records::subject(
            &mut *self.pool.acquire().await.map_err(storage_error)?,
            namespace,
            id,
        )
        .await?
        .ok_or_else(not_found)
    }

    pub async fn material(&self, namespace: &NamespaceId, id: &MaterialId) -> Result<Material> {
        records::material(
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
        records::observation(
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
        sqlx::query_scalar::<_, String>("SELECT namespace_id FROM subjects WHERE id=?")
            .bind(id.as_str())
            .fetch_optional(&self.pool)
            .await
            .map_err(storage_error)?
            .map(NamespaceId)
            .ok_or_else(not_found)
    }

    pub async fn subject_has_material_access(
        &self,
        namespace: &NamespaceId,
        subject: &SubjectId,
        material: &MaterialId,
    ) -> Result<bool> {
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM observations WHERE namespace_id=? AND subject_id=? AND material_id=?)")
            .bind(namespace.as_str()).bind(subject.as_str()).bind(material.as_str())
            .fetch_one(&self.pool).await.map_err(storage_error)
    }

    pub async fn list_namespaces(&self) -> Result<Vec<Namespace>> {
        sqlx::query("SELECT * FROM namespaces ORDER BY commit_seq,id")
            .fetch_all(&self.pool)
            .await
            .map_err(storage_error)?
            .into_iter()
            .map(records::namespace_row)
            .collect()
    }

    pub async fn list_subjects(&self, namespace: &NamespaceId) -> Result<Vec<Subject>> {
        sqlx::query("SELECT * FROM subjects WHERE namespace_id=? ORDER BY commit_seq,id")
            .bind(namespace.as_str())
            .fetch_all(&self.pool)
            .await
            .map_err(storage_error)?
            .into_iter()
            .map(records::subject_row)
            .collect()
    }

    pub async fn list_materials(&self, namespace: &NamespaceId) -> Result<Vec<Material>> {
        sqlx::query("SELECT * FROM materials WHERE namespace_id=? ORDER BY commit_seq,id")
            .bind(namespace.as_str())
            .fetch_all(&self.pool)
            .await
            .map_err(storage_error)?
            .into_iter()
            .map(records::material_row)
            .collect()
    }

    pub async fn list_observations(&self, subject: &SubjectId) -> Result<Vec<Observation>> {
        sqlx::query("SELECT * FROM observations WHERE subject_id=? ORDER BY commit_seq,id")
            .bind(subject.as_str())
            .fetch_all(&self.pool)
            .await
            .map_err(storage_error)?
            .into_iter()
            .map(records::observation_row)
            .collect()
    }

    pub async fn job(&self, id: &str) -> Result<JobRecord> {
        let row = sqlx::query("SELECT * FROM Jobs WHERE id=? AND job_type=?")
            .bind(id)
            .bind(JOB_QUEUE)
            .fetch_optional(&self.pool)
            .await
            .map_err(storage_error)?;
        row.map(|row| {
            let bytes: Vec<u8> = row.try_get("job").map_err(storage_error)?;
            let envelope: JobEnvelope = serde_json::from_slice(&bytes).map_err(storage_error)?;
            Ok::<_, Error>(JobRecord {
                id: row.get("id"),
                kind: envelope.kind.clone(),
                status: row.get("status"),
                attempts: row.get("attempts"),
                max_attempts: row.get("max_attempts"),
                last_result: row.get("last_result"),
                envelope,
            })
        })
        .transpose()?
        .ok_or_else(not_found)
    }
}

fn not_found() -> Error {
    Error::new(ErrorCode::NotFound, "object was not found")
}

fn storage_error(error: impl std::fmt::Display) -> Error {
    Error::new(ErrorCode::Internal, format!("storage: {error}"))
}

fn write_error(error: sqlx::Error) -> Error {
    if let sqlx::Error::Database(ref database) = error {
        if database.is_unique_violation() {
            return Error::new(ErrorCode::Conflict, "object already exists");
        }
        if database.is_foreign_key_violation() || database.is_check_violation() {
            return Error::new(
                ErrorCode::InvalidArgument,
                "object scope or field violates the storage contract",
            );
        }
    }
    storage_error(error)
}
