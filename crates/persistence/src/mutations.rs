//! Subject-scoped mutation envelopes and receipt mechanics.
// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use crate::{AuthorityStore, ProjectionInvalidation, database_error as db, lock_operation};
use chrono::Utc;
use nous_core::{CognitiveRef, Error, ErrorCode, ErrorOwner, OperationId, Result, SubjectId};
use serde::{Deserialize, Serialize};
use sqlx::{Row, types::Json};
use uuid::Uuid;

/// Completed results returned for an idempotent replay.
#[derive(Debug, Clone)]
pub struct MutationReceipt {
    pub results: Vec<MutationResult>,
}

enum MutationReceiptRecord {
    InProgress,
    Committed(MutationReceipt),
}

/// The exact owner result retained for idempotent replay and operation status.
/// `revision_id` is kept beside the stable object reference for APIs such as
/// Tag whose immutable revision is not itself a general CognitiveRef.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MutationResult {
    pub reference: CognitiveRef,
    pub revision_id: Option<Uuid>,
    pub object_epoch: Option<i64>,
}

impl MutationResult {
    pub fn new(
        reference: CognitiveRef,
        revision_id: Option<Uuid>,
        object_epoch: Option<i64>,
    ) -> Self {
        Self {
            reference,
            revision_id,
            object_epoch,
        }
    }

    pub fn reference(reference: CognitiveRef) -> Self {
        Self::new(reference, None, None)
    }

    pub fn revised(reference: CognitiveRef, revision_id: Uuid, object_epoch: Option<i64>) -> Self {
        Self::new(reference, Some(revision_id), object_epoch)
    }
}

async fn check_receipt(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    subject: SubjectId,
    operation: OperationId,
    kind: &str,
    digest: &str,
    error_owner: nous_core::ErrorOwner,
) -> Result<Option<MutationReceiptRecord>> {
    let row = sqlx::query("SELECT state,results,request_digest,operation_kind FROM mutation_receipts WHERE subject_id=$1 AND operation_id=$2")
        .bind(subject.0)
        .bind(operation.0)
        .fetch_optional(&mut **tx)
        .await
        .map_err(db)?;
    if let Some(row) = row {
        let existing: String = row.try_get("request_digest").map_err(db)?;
        if existing != digest || row.try_get::<String, _>("operation_kind").map_err(db)? != kind {
            return Err(nous_core::Error::new(
                error_owner,
                nous_core::ErrorCode::OperationIdConflict,
                format!("{kind} operation_id was used with a different request"),
            )
            .with_context("operation_id", operation.0)
            .with_context("operation_kind", kind));
        }
        return match row.try_get::<String, _>("state").map_err(db)?.as_str() {
            "in_progress" => Ok(Some(MutationReceiptRecord::InProgress)),
            "committed" => Ok(Some(MutationReceiptRecord::Committed(MutationReceipt {
                results: row
                    .try_get::<Json<Vec<MutationResult>>, _>("results")
                    .map_err(db)?
                    .0,
            }))),
            _ => Err(Error::new(
                ErrorOwner::Persistence,
                ErrorCode::InternalFailure,
                "Mutation receipt has an invalid state",
            )),
        };
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
    results: &[MutationResult],
) -> Result<()> {
    sqlx::query("UPDATE mutation_receipts SET state='committed',results=$3,committed_at=$4 WHERE subject_id=$1 AND operation_id=$2")
        .bind(subject.0)
        .bind(operation.0)
        .bind(Json(results))
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
    /// A completed receipt; ordinary in-progress retries fail in `begin`.
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
        error_owner: nous_core::ErrorOwner,
    ) -> Result<MutationStart<'a>> {
        Self::start(
            store,
            subject,
            operation,
            kind,
            digest,
            owner,
            error_owner,
            false,
        )
        .await
    }

    /// Resume an intentional checkpoint while committed results still replay.
    pub async fn resume(
        store: &'a AuthorityStore,
        subject: SubjectId,
        operation: OperationId,
        kind: &str,
        digest: &str,
        owner: Option<OwnerLock>,
        error_owner: nous_core::ErrorOwner,
    ) -> Result<MutationStart<'a>> {
        Self::start(
            store,
            subject,
            operation,
            kind,
            digest,
            owner,
            error_owner,
            true,
        )
        .await
    }

    async fn start(
        store: &'a AuthorityStore,
        subject: SubjectId,
        operation: OperationId,
        kind: &str,
        digest: &str,
        owner: Option<OwnerLock>,
        error_owner: nous_core::ErrorOwner,
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
        if let Some(receipt) =
            check_receipt(&mut tx, subject, operation, kind, digest, error_owner).await?
        {
            match receipt {
                MutationReceiptRecord::Committed(receipt) => {
                    tx.commit().await.map_err(db)?;
                    return Ok(MutationStart::Replay(receipt));
                }
                MutationReceiptRecord::InProgress if resume => {}
                MutationReceiptRecord::InProgress => {
                    tx.commit().await.map_err(db)?;
                    return Err(Error::new(
                        error_owner,
                        ErrorCode::OperationInProgress,
                        format!("{kind} operation is already in progress"),
                    )
                    .with_context("operation_id", operation.0)
                    .with_context("operation_kind", kind));
                }
            }
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

    pub async fn commit(mut self, results: &[MutationResult]) -> Result<()> {
        self.apply_invalidation().await?;
        commit_receipt(&mut self.tx, self.subject, self.operation, results).await?;
        self.tx.commit().await.map_err(db)
    }

    /// Persist an intentional multi-phase operation without acknowledging completion.
    pub async fn checkpoint(mut self) -> Result<()> {
        self.apply_invalidation().await?;
        self.tx.commit().await.map_err(db)
    }

    /// Persist a typed owner checkpoint in the existing mutation receipt. The
    /// receipt remains in_progress, so a same-operation retry can resume while
    /// another operation cannot adopt the owner's purging state.
    pub async fn checkpoint_with_result(mut self, result: &MutationResult) -> Result<()> {
        self.apply_invalidation().await?;
        sqlx::query("UPDATE mutation_receipts SET results=$3 WHERE subject_id=$1 AND operation_id=$2 AND state='in_progress'")
            .bind(self.subject.0)
            .bind(self.operation.0)
            .bind(Json(std::slice::from_ref(result)))
            .execute(&mut *self.tx)
            .await
            .map_err(db)?;
        self.tx.commit().await.map_err(db)
    }

    /// Verify that this operation owns a previously persisted typed checkpoint.
    pub async fn checkpoint_matches(&mut self, result: &MutationResult) -> Result<bool> {
        let matches: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM mutation_receipts WHERE subject_id=$1 AND operation_id=$2 AND state='in_progress' AND results=$3)")
            .bind(self.subject.0)
            .bind(self.operation.0)
            .bind(Json(std::slice::from_ref(result)))
            .fetch_one(&mut *self.tx)
            .await
            .map_err(db)?;
        Ok(matches)
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
