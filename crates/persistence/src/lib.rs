//! PostgreSQL Authority repositories and migration ownership.
// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

mod identity;
mod proquint;
pub use proquint::{decode_proquint, encode_proquint};
mod mutations;
pub use mutations::{
    MutationEnvelope, MutationReceipt, MutationResult, MutationStart, OwnerLock, check_receipt,
    commit_receipt,
};
mod read_stamp;
pub use read_stamp::AuthorityReadStamp;
mod model_workflow;
pub use model_workflow::{
    CompletedWorkflow, PendingWorkflow, WORKFLOW_VALUE_MAX_BYTES, WorkflowLease, WorkflowOwner,
    WorkflowRecord, WorkflowReservation,
};
mod episode_text;
pub use episode_text::EpisodeTextBudget;
mod embedding_migration;
pub use embedding_migration::EmbeddingUpgradeInstants;
mod historical;
pub use historical::projection::HistoricalProjectionInput;
pub use historical::*;
mod producer;
mod projection;
pub use projection::concept::{ConceptProjectionInput, ConceptProjectionTag};
mod query_descriptors;
mod query_time;
pub use query_time::{TimeColumns, push_time_predicate};
mod references;
pub use identity::{
    IdentityAddress, IdentityAddressTarget, IdentityBinding, lexical_prefix, validate_lexical,
};
pub use query_descriptors::QueryDescriptor;
mod semantic_catalog;
mod serving;
mod tags;
pub use projection::text::{TextProjectionFragment, TextProjectionInput, TextProjectionSource};
pub use projection::topology::{
    CognitiveProjectionRows, ProjectionEdgeProvenance, TopologyEdgeSource, TopologyProjectionInput,
};
pub use projection::{DenseInvalidation, ProjectionInvalidation};
pub use serving::{ServingRecord, ServingView};

use nous_core::{Error, OperationId, Result, SubjectId};
use sqlx::{Connection, PgConnection, PgPool, Postgres, Transaction, postgres::PgPoolOptions};
use std::sync::Arc;
use tokio::sync::Mutex;

/// Serialize a Subject-scoped mutation receipt within its Authority transaction.
pub async fn lock_operation(
    tx: &mut Transaction<'_, Postgres>,
    subject: SubjectId,
    operation: OperationId,
) -> Result<()> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
        .bind(format!("{}:{}", subject.0, operation.0))
        .execute(&mut **tx)
        .await
        .map_err(database_error)?;
    Ok(())
}

#[derive(Clone)]
pub struct AuthorityStore {
    pool: PgPool,
    instance_lock: Arc<Mutex<Option<PgConnection>>>,
}

impl AuthorityStore {
    pub async fn active_subjects(&self) -> Result<Vec<SubjectId>> {
        Ok(sqlx::query_scalar::<_, uuid::Uuid>(
            "SELECT subject_id FROM subjects WHERE status='active' ORDER BY subject_id",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(database_error)?
        .into_iter()
        .map(SubjectId)
        .collect())
    }

    pub async fn database_name(&self) -> Result<String> {
        sqlx::query_scalar("SELECT current_database()")
            .fetch_one(self.pool())
            .await
            .map_err(database_error)
    }

    pub async fn connect(
        url: &str,
        max_connections: u32,
        acquire_timeout: std::time::Duration,
    ) -> Result<Self> {
        if max_connections == 0 {
            return Err(Error::new(
                nous_core::ErrorOwner::Persistence,
                nous_core::ErrorCode::InvalidRequest,
                "max_connections must be positive",
            ));
        }
        // Hold the database-scoped Kernel lease on a dedicated PostgreSQL
        // session, never on a pooled connection that another query may reuse.
        let mut lock_connection = PgConnection::connect(url).await.map_err(database_error)?;
        let acquired: bool = sqlx::query_scalar(
            "SELECT pg_try_advisory_lock(hashtextextended(current_database() || ':nous-wave:kernel-instance', 0))",
        )
        .fetch_one(&mut lock_connection)
        .await
        .map_err(database_error)?;
        if !acquired {
            return Err(Error::new(
                nous_core::ErrorOwner::Instance,
                nous_core::ErrorCode::InstanceAlreadyRunning,
                "An instance is already running",
            )
            .with_context("reason", "instance_already_running"));
        }
        let pool = PgPoolOptions::new()
            .max_connections(max_connections)
            .acquire_timeout(acquire_timeout)
            .connect(url)
            .await
            .map_err(database_error)?;
        Ok(Self {
            pool,
            instance_lock: Arc::new(Mutex::new(Some(lock_connection))),
        })
    }

    pub async fn migrate(&self) -> Result<()> {
        sqlx::migrate!("./migrations")
            .run(&self.pool)
            .await
            .map_err(|error| {
                Error::new(
                    nous_core::ErrorOwner::Persistence,
                    nous_core::ErrorCode::InternalFailure,
                    "Could not apply persistence schema migrations",
                )
                .with_source(error)
            })
    }

    pub async fn check(&self) -> Result<()> {
        sqlx::query("SELECT 1")
            .execute(&self.pool)
            .await
            .map_err(database_error)?;
        Ok(())
    }

    pub async fn subject_exists(&self, subject: SubjectId) -> Result<bool> {
        sqlx::query_scalar::<_, bool>("SELECT EXISTS(SELECT 1 FROM subjects WHERE subject_id=$1)")
            .bind(subject.0)
            .fetch_one(&self.pool)
            .await
            .map_err(database_error)
    }

    pub async fn authority_seq(&self, subject: SubjectId) -> Result<i64> {
        sqlx::query_scalar::<_, i64>("SELECT authority_seq FROM subjects WHERE subject_id=$1")
            .bind(subject.0)
            .fetch_optional(&self.pool)
            .await
            .map_err(database_error)?
            .ok_or_else(|| {
                Error::new(
                    nous_core::ErrorOwner::Persistence,
                    nous_core::ErrorCode::NotFound,
                    "subject not found",
                )
            })
    }

    /// Owner purge checkpoints call this inside the same Authority transaction
    /// that makes the target unreadable. It intentionally allocates no separate
    /// operation state.
    pub async fn advance_serving_purge_epoch_in(
        tx: &mut Transaction<'_, Postgres>,
        subject: SubjectId,
    ) -> Result<i64> {
        sqlx::query_scalar::<_, i64>(
            "UPDATE subjects SET serving_purge_epoch=serving_purge_epoch+1 WHERE subject_id=$1 RETURNING serving_purge_epoch",
        )
        .bind(subject.0)
        .fetch_optional(&mut **tx)
        .await
        .map_err(database_error)?
        .ok_or_else(|| Error::new(nous_core::ErrorOwner::Subject, nous_core::ErrorCode::NotFound, "subject not found"))
    }

    pub async fn begin(&self) -> Result<Transaction<'_, Postgres>> {
        self.pool.begin().await.map_err(database_error)
    }

    pub async fn close(&self) {
        self.pool.close().await;
        let mut lock = self.instance_lock.lock().await;
        if let Some(connection) = lock.take() {
            let _ = connection.close().await;
        }
    }

    /// Ping the lock-owning session; Kernel main stops serving if this fails.
    /// Reconnecting here would silently relinquish exclusive DB ownership.
    pub async fn check_instance_lock(&self) -> Result<()> {
        let mut lock = self.instance_lock.lock().await;
        let connection = lock.as_mut().ok_or_else(instance_lock_lost)?;
        tokio::time::timeout(
            std::time::Duration::from_secs(3),
            sqlx::query("SELECT 1").execute(&mut *connection),
        )
        .await
        .map_err(|_| instance_lock_lost())?
        .map_err(|error| instance_lock_lost().with_source(error))?;
        Ok(())
    }

    pub fn pool(&self) -> &PgPool {
        &self.pool
    }
}

fn instance_lock_lost() -> Error {
    Error::new(
        nous_core::ErrorOwner::Instance,
        nous_core::ErrorCode::LeaseLost,
        "Kernel database ownership lease was lost",
    )
}

pub fn database_error(error: sqlx::Error) -> Error {
    let mapped = match &error {
        sqlx::Error::RowNotFound => Error::new(
            nous_core::ErrorOwner::Persistence,
            nous_core::ErrorCode::NotFound,
            "Authority record not found",
        ),
        sqlx::Error::Database(e) if e.is_unique_violation() => Error::new(
            nous_core::ErrorOwner::Persistence,
            nous_core::ErrorCode::Conflict,
            "Authority record conflicts with existing state",
        ),
        sqlx::Error::Database(e) if e.is_check_violation() || e.is_foreign_key_violation() => {
            Error::new(
                nous_core::ErrorOwner::Persistence,
                nous_core::ErrorCode::InvalidRequest,
                "Authority record violates a storage constraint",
            )
        }
        _ => Error::new(
            nous_core::ErrorOwner::Persistence,
            nous_core::ErrorCode::InternalFailure,
            "Persistence operation failed",
        ),
    };
    mapped.with_source(error)
}
