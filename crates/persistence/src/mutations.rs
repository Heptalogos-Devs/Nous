//! Subject-scoped mutation envelopes and receipt mechanics.
// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use crate::{AuthorityStore, ProjectionInvalidation, database_error as db, lock_operation};
use chrono::Utc;
use nous_core::{DomainError, DomainErrorCode, OperationId, Result, SubjectId};
use sqlx::Row;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct MutationReceipt {
    pub state: String,
    pub result_ref: Option<String>,
    pub result_revision: Option<Uuid>,
}

pub async fn check_receipt(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    subject: SubjectId,
    operation: OperationId,
    kind: &str,
    digest: &str,
) -> Result<Option<MutationReceipt>> {
    let row = sqlx::query("SELECT state,result_ref,result_revision,request_digest,operation_kind FROM mutation_receipts WHERE subject_id=$1 AND operation_id=$2")
        .bind(subject.0)
        .bind(operation.0)
        .fetch_optional(&mut **tx)
        .await
        .map_err(db)?;
    if let Some(row) = row {
        let existing: String = row.try_get("request_digest").map_err(db)?;
        if existing != digest || row.try_get::<String, _>("operation_kind").map_err(db)? != kind {
            return Err(DomainError::new(
                DomainErrorCode::OperationIdConflict,
                format!("{kind} operation_id was used with a different request"),
            )
            .with_context("operation_id", operation.0)
            .with_context("operation_kind", kind)
            .into());
        }
        return Ok(Some(MutationReceipt {
            state: row.try_get("state").map_err(db)?,
            result_ref: row.try_get("result_ref").map_err(db)?,
            result_revision: row.try_get("result_revision").map_err(db)?,
        }));
    }
    sqlx::query("INSERT INTO mutation_receipts(subject_id,operation_id,operation_kind,request_digest,state,created_at) VALUES($1,$2,$3,$4,'in_progress',$5)")
        .bind(subject.0)
        .bind(operation.0)
        .bind(kind)
        .bind(digest)
        .bind(Utc::now())
        .execute(&mut **tx)
        .await
        .map_err(db)?;
    Ok(None)
}

pub async fn commit_receipt(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    subject: SubjectId,
    operation: OperationId,
    result_kind: &str,
    result_ref: Option<&str>,
    result_revision: Option<Uuid>,
    result_epoch: Option<i64>,
) -> Result<()> {
    sqlx::query("UPDATE mutation_receipts SET state='committed',result_kind=$3,result_ref=$4,result_revision=$5,result_epoch=$6,committed_at=$7 WHERE subject_id=$1 AND operation_id=$2")
        .bind(subject.0)
        .bind(operation.0)
        .bind(result_kind)
        .bind(result_ref)
        .bind(result_revision)
        .bind(result_epoch)
        .bind(Utc::now())
        .execute(&mut **tx)
        .await
        .map_err(db)?;
    Ok(())
}

/// The semantic owner chooses its Subject serialization boundary.
#[derive(Clone, Copy)]
pub struct OwnerLock(pub &'static str);

pub enum MutationStart<'a> {
    Replay(MutationReceipt),
    Active(MutationEnvelope<'a>),
}

/// Transaction, identity, receipt and projection mechanics shared by semantic owners.
pub struct MutationEnvelope<'a> {
    tx: sqlx::Transaction<'a, sqlx::Postgres>,
    subject: SubjectId,
    operation: OperationId,
    invalidation: ProjectionInvalidation,
    sequence: Option<i64>,
}

impl<'a> MutationEnvelope<'a> {
    pub async fn begin(
        store: &'a AuthorityStore,
        subject: SubjectId,
        operation: OperationId,
        kind: &str,
        digest: &str,
        owner: Option<OwnerLock>,
    ) -> Result<MutationStart<'a>> {
        Self::start(store, subject, operation, kind, digest, owner, false).await
    }

    /// Resume an intentional checkpoint while committed results still replay.
    pub async fn resume(
        store: &'a AuthorityStore,
        subject: SubjectId,
        operation: OperationId,
        kind: &str,
        digest: &str,
        owner: Option<OwnerLock>,
    ) -> Result<MutationStart<'a>> {
        Self::start(store, subject, operation, kind, digest, owner, true).await
    }

    async fn start(
        store: &'a AuthorityStore,
        subject: SubjectId,
        operation: OperationId,
        kind: &str,
        digest: &str,
        owner: Option<OwnerLock>,
        resume: bool,
    ) -> Result<MutationStart<'a>> {
        let mut tx = store.begin().await?;
        if let Some(owner) = owner {
            sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
                .bind(format!("{}:{}", owner.0, subject.0))
                .execute(&mut *tx)
                .await
                .map_err(db)?;
        }
        lock_operation(&mut tx, subject, operation).await?;
        if let Some(receipt) = check_receipt(&mut tx, subject, operation, kind, digest).await?
            && (!resume || receipt.state == "committed")
        {
            tx.commit().await.map_err(db)?;
            return Ok(MutationStart::Replay(receipt));
        }
        Ok(MutationStart::Active(Self {
            tx,
            subject,
            operation,
            invalidation: ProjectionInvalidation::default(),
            sequence: None,
        }))
    }

    pub async fn capture_authority_time(
        &mut self,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<()> {
        sqlx::query("SELECT set_config('nous.authority_time',$1,true)")
            .bind(now.to_rfc3339_opts(chrono::SecondsFormat::Micros, true))
            .execute(&mut *self.tx)
            .await
            .map_err(db)?;
        Ok(())
    }
    pub fn tx(&mut self) -> &mut sqlx::Transaction<'a, sqlx::Postgres> {
        &mut self.tx
    }

    /// Publish actual owner results before commit, including the host's acknowledgement gap.
    pub async fn publish_workflow_results(
        &mut self,
        owner: &str,
        references: &[nous_core::CognitiveRef],
    ) -> Result<()> {
        AuthorityStore::add_workflow_dependencies_in(
            &mut self.tx,
            self.subject,
            owner,
            self.operation,
            references,
        )
        .await
    }

    /// Accumulate owner-selected families; allocate one Authority sequence for this transaction.
    pub async fn invalidate(&mut self, changes: ProjectionInvalidation) -> Result<i64> {
        self.invalidation.merge(changes);
        if let Some(sequence) = self.sequence {
            return Ok(sequence);
        }
        let sequence = sqlx::query_scalar("UPDATE subjects SET authority_seq=authority_seq+1 WHERE subject_id=$1 RETURNING authority_seq")
            .bind(self.subject.0).fetch_one(&mut *self.tx).await.map_err(db)?;
        self.sequence = Some(sequence);
        Ok(sequence)
    }

    async fn apply_invalidation(&mut self) -> Result<()> {
        if let Some(sequence) = self.sequence {
            AuthorityStore::mark_projection_families_in(
                &mut self.tx,
                self.subject,
                sequence,
                std::mem::take(&mut self.invalidation),
            )
            .await?;
        }
        Ok(())
    }

    pub async fn commit(
        mut self,
        result_kind: &str,
        result_ref: Option<&str>,
        result_revision: Option<Uuid>,
        result_epoch: Option<i64>,
    ) -> Result<()> {
        self.apply_invalidation().await?;
        commit_receipt(
            &mut self.tx,
            self.subject,
            self.operation,
            result_kind,
            result_ref,
            result_revision,
            result_epoch,
        )
        .await?;
        self.tx.commit().await.map_err(db)
    }

    /// Persist an intentional multi-phase operation without acknowledging completion.
    pub async fn checkpoint(mut self) -> Result<()> {
        self.apply_invalidation().await?;
        self.tx.commit().await.map_err(db)
    }
}

impl AuthorityStore {
    pub async fn authority_time_in(
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    ) -> Result<chrono::DateTime<chrono::Utc>> {
        sqlx::query_scalar("SELECT authority_recording_time()")
            .fetch_one(&mut **tx)
            .await
            .map_err(db)
    }
}
