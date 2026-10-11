// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use crate::*;
use nous_persistence::database_error as db;
use sqlx::Row;
use std::collections::HashMap;
use uuid::Uuid;

impl CognitiveRuntimeService {
    #[expect(
        clippy::too_many_lines,
        clippy::excessive_nesting,
        reason = "batch UseEvent transaction owns atomic idempotency and runtime side effects"
    )]
    pub async fn use_feedback(&self, input: UseFeedback) -> Result<(u32, u32, Option<i64>)> {
        self.require_subject(input.subject).await?;
        validate_consumer_ref(&input.consumer_ref)?;
        if input.events.is_empty() || input.events.len() > 256 {
            return Err(Error::Invalid(
                "ReportUse events must contain 1..256 items".into(),
            ));
        }
        for event in &input.events {
            if event.occurred_at < DateTime::<Utc>::UNIX_EPOCH
                || event.occurred_at > self.now(input.subject) + chrono::Duration::minutes(5)
            {
                return Err(Error::Invalid(
                    "UseEvent occurred_at is outside the allowed range".into(),
                ));
            }
            if !matches!(
                event.reference,
                CognitiveRef::MemoryRevision(_)
                    | CognitiveRef::CognitiveSchemaRevision(_)
                    | CognitiveRef::EpisodeRevision(_)
                    | CognitiveRef::JournalRevision(_)
            ) {
                return Err(Error::Invalid(
                    "UseEvent requires an exact cognition revision".into(),
                ));
            }
            if serde_json::to_vec(&event.context)
                .map_err(|error| Error::Invalid(error.to_string()))?
                .len()
                > 16 * 1024
            {
                return Err(Error::Invalid("UseEvent context exceeds 16 KiB".into()));
            }
            validate_context_metadata(&event.context)?;
        }
        let mut tx = self.store.begin().await?;
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
            .bind(format!("use:{}:{}", input.subject.0, input.consumer_ref))
            .execute(&mut *tx)
            .await
            .map_err(db)?;
        if let Some(session) = input.session_id {
            let same_subject: Option<Uuid> = sqlx::query_scalar("SELECT session_id FROM cognitive_sessions WHERE subject_id=$1 AND session_id=$2 AND closed_at IS NULL FOR UPDATE")
                .bind(input.subject.0).bind(session.0).fetch_optional(&mut *tx).await.map_err(db)?;
            if same_subject.is_none() {
                return Err(Error::FailedPrecondition(
                    "session is closed or not owned by Subject".into(),
                ));
            }
        }
        #[derive(Clone)]
        struct Prepared {
            event: UseFeedbackEvent,
            kind: String,
            value: String,
            digest: String,
            duplicate: bool,
        }
        let mut prepared = Vec::with_capacity(input.events.len());
        let mut seen = HashMap::new();
        for event in input.events {
            let (kind, value) = reference_parts(&event.reference);
            let digest = canonical_request_digest(
                "use_event",
                input.subject,
                &serde_json::json!({"consumer_ref":input.consumer_ref,"event_id":event.event_id,"session_id":input.session_id,"reference":event.reference,"kind":event.use_kind,"occurred_at":event.occurred_at,"context":event.context,"query_id":event.query_id}),
            )?;
            if let Some(existing) =
                seen.insert((input.consumer_ref.clone(), event.event_id), digest.clone())
            {
                if existing != digest {
                    return Err(DomainError::new(
                        DomainErrorCode::OperationIdConflict,
                        "Duplicate event in ReportUse has different content",
                    )
                    .with_context("event_id", event.event_id.0)
                    .into());
                }
                prepared.push(Prepared {
                    event,
                    kind,
                    value,
                    digest,
                    duplicate: true,
                });
                continue;
            }
            let existing = sqlx::query("SELECT request_digest FROM cognitive_use_events WHERE subject_id=$1 AND consumer_ref=$2 AND event_id=$3")
                .bind(input.subject.0).bind(&input.consumer_ref).bind(event.event_id.0).fetch_optional(&mut *tx).await.map_err(db)?;
            let purged = sqlx::query("SELECT request_digest FROM purged_use_receipts WHERE subject_id=$1 AND consumer_ref=$2 AND event_id=$3")
                .bind(input.subject.0).bind(&input.consumer_ref).bind(event.event_id.0).fetch_optional(&mut *tx).await.map_err(db)?;
            let stored_digest = existing
                .as_ref()
                .map(|row| row.try_get::<String, _>("request_digest"))
                .transpose()
                .map_err(db)?
                .or(purged
                    .as_ref()
                    .map(|row| row.try_get::<String, _>("request_digest"))
                    .transpose()
                    .map_err(db)?);
            if let Some(stored) = stored_digest.as_ref()
                && stored != &digest
            {
                return Err(DomainError::new(
                    DomainErrorCode::OperationIdConflict,
                    "UseEvent idempotency key has a different request digest",
                )
                .with_context("event_id", event.event_id.0)
                .into());
            }
            if stored_digest.is_none() {
                validate_use_target_in(&mut tx, input.subject, &event.reference).await?;
                if let Some(query_id) = event.query_id {
                    crate::query_feedback::validate_query_use_in(
                        &mut tx,
                        input.subject,
                        query_id,
                        &event.reference,
                        self.now(input.subject),
                    )
                    .await?;
                }
            }
            prepared.push(Prepared {
                event,
                kind,
                value,
                digest,
                duplicate: stored_digest.is_some(),
            });
        }
        let recorded_at = self.now(input.subject);
        let mut accepted = 0u32;
        let mut duplicates = 0u32;
        let mut meaningful_times = Vec::new();
        let mut concept_times = Vec::new();
        for item in prepared {
            if item.duplicate {
                duplicates += 1;
                continue;
            }
            sqlx::query("INSERT INTO cognitive_use_events(subject_id,consumer_ref,event_id,ref_kind,ref_value,use_kind,session_id,occurred_at,recorded_at,context,request_digest,query_id) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12)")
                .bind(input.subject.0).bind(&input.consumer_ref).bind(item.event.event_id.0).bind(&item.kind).bind(&item.value).bind(item.event.use_kind.as_str()).bind(input.session_id.map(|id|id.0)).bind(item.event.occurred_at).bind(recorded_at).bind(&item.event.context).bind(&item.digest).bind(item.event.query_id).execute(&mut *tx).await.map_err(db)?;
            accepted += 1;
            if matches!(
                item.event.use_kind,
                UseKind::Referenced
                    | UseKind::ActedOn
                    | UseKind::ResultSupported
                    | UseKind::ResultRefuted
                    | UseKind::Corrected
                    | UseKind::Pinned
            ) {
                concept_times.push((
                    item.kind.clone(),
                    item.value.clone(),
                    item.event.occurred_at,
                ));
            }
            if item.event.use_kind.meaningful() {
                meaningful_times.push((item.kind, item.value, item.event.occurred_at));
            }
        }
        let session_revision = if let Some(session) = input.session_id {
            if accepted == 0 {
                let revision: i64 = sqlx::query_scalar(
                    "SELECT runtime_revision FROM cognitive_sessions WHERE session_id=$1",
                )
                .bind(session.0)
                .fetch_one(&mut *tx)
                .await
                .map_err(db)?;
                Some(revision)
            } else {
                for (kind, value, occurred_at) in &meaningful_times {
                    let updated = sqlx::query("UPDATE resident_refs SET state='resident',last_meaningful_use_at=GREATEST(COALESCE(last_meaningful_use_at,$4),$4) WHERE session_id=$1 AND ref_kind=$2 AND ref_value=$3")
                    .bind(session.0).bind(kind).bind(value).bind(occurred_at).execute(&mut *tx).await.map_err(db)?;
                    if updated.rows_affected() == 0 {
                        sqlx::query("INSERT INTO resident_refs(session_id,ref_kind,ref_value,entered_at,entry_reason,last_meaningful_use_at,state,metadata) VALUES($1,$2,$3,$4,'meaningful_use',$4,'resident','{}') ON CONFLICT DO NOTHING")
                        .bind(session.0).bind(kind).bind(value).bind(occurred_at).execute(&mut *tx).await.map_err(db)?;
                    }
                }
                let last_meaningful = meaningful_times.iter().map(|(_, _, time)| *time).max();
                let row = sqlx::query("UPDATE cognitive_sessions SET last_activity_at=$2,last_meaningful_use_at=COALESCE(GREATEST(last_meaningful_use_at,$3),last_meaningful_use_at),runtime_revision=runtime_revision+1 WHERE session_id=$1 RETURNING runtime_revision")
                .bind(session.0).bind(recorded_at).bind(last_meaningful).fetch_one(&mut *tx).await.map_err(db)?;
                Some(row.try_get::<i64, _>("runtime_revision").map_err(db)?)
            }
        } else {
            None
        };
        if !concept_times.is_empty() {
            let threshold = self
                .configuration
                .snapshot_for_subject(input.subject)?
                .get(crate::policy::maintenance::CONCEPT_USE_REVIEW_INTERVAL)?;
            let sequence: i64 =
                sqlx::query_scalar("SELECT authority_seq FROM subjects WHERE subject_id=$1")
                    .bind(input.subject.0)
                    .fetch_one(&mut *tx)
                    .await
                    .map_err(db)?;
            let mut queued = std::collections::HashSet::new();
            for (kind, value, _) in &concept_times {
                if !queued.insert((kind, value)) {
                    continue;
                }
                let count:i64=sqlx::query_scalar("SELECT count(*) FROM cognitive_use_events WHERE subject_id=$1 AND ref_kind=$2 AND ref_value=$3 AND use_kind IN ('referenced','acted_on','result_supported','result_refuted','corrected','pinned')")
                    .bind(input.subject.0).bind(kind).bind(value).fetch_one(&mut *tx).await.map_err(db)?;
                let count = u64::try_from(count)
                    .map_err(|_| Error::Infrastructure("invalid use count".into()))?;
                let added = u64::try_from(
                    concept_times
                        .iter()
                        .filter(|(k, v, _)| k == kind && v == value)
                        .count(),
                )
                .map_err(|_| Error::Infrastructure("invalid use count".into()))?;
                if count / threshold > count.saturating_sub(added) / threshold {
                    self.enqueue_maintenance_in(
                        &mut tx,
                        &MaintenanceRequest {
                            subject: input.subject,
                            kind: "concept_maintenance".into(),
                            scope_kind: kind.clone(),
                            scope_ref: value.clone(),
                            trigger_authority_seq: sequence,
                            due_at: recorded_at,
                            priority: 25,
                        },
                    )
                    .await?;
                }
            }
        }
        tx.commit().await.map_err(db)?;
        Ok((accepted, duplicates, session_revision))
    }
}

fn validate_consumer_ref(value: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > 512
        || value.chars().any(char::is_whitespace)
        || value.matches(':').count() < 2
    {
        return Err(Error::Invalid(
            "consumer_ref must be a namespaced opaque reference".into(),
        ));
    }
    Ok(())
}

fn validate_context_metadata(value: &serde_json::Value) -> Result<()> {
    let Some(object) = value.as_object() else {
        return Ok(());
    };
    for key in object.keys() {
        if matches!(
            key.as_str(),
            "raw" | "prompt" | "content" | "memory_text" | "artifact_bytes"
        ) {
            return Err(Error::Invalid(
                "UseEvent context may contain metadata only; raw content fields are forbidden"
                    .into(),
            ));
        }
    }
    Ok(())
}

async fn validate_use_target_in(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    subject: SubjectId,
    reference: &CognitiveRef,
) -> Result<()> {
    let (query, id) = match reference {
        CognitiveRef::MemoryRevision(id) => (
            "SELECT o.purge_state FROM memory_revisions r JOIN memory_objects o USING(memory_id) WHERE r.subject_id=$1 AND r.memory_revision_id=$2 FOR SHARE OF o",
            id.0,
        ),
        CognitiveRef::CognitiveSchemaRevision(id) => (
            "SELECT o.purge_state FROM cognitive_schema_revisions r JOIN cognitive_schemas o USING(schema_id) WHERE o.subject_id=$1 AND r.schema_revision_id=$2 FOR SHARE OF o",
            id.0,
        ),
        CognitiveRef::EpisodeRevision(id) => (
            "SELECT o.purge_state FROM episode_revisions r JOIN episode_objects o USING(episode_id) WHERE r.subject_id=$1 AND r.episode_revision_id=$2 FOR SHARE OF o",
            id.0,
        ),
        CognitiveRef::JournalRevision(id) => (
            "SELECT o.purge_state FROM journal_revisions r JOIN journal_objects o USING(journal_id) WHERE r.subject_id=$1 AND r.journal_revision_id=$2 FOR SHARE OF o",
            id.0,
        ),
        _ => {
            return Err(Error::Invalid(
                "UseEvent requires an exact cognition revision".into(),
            ));
        }
    };
    let state: Option<String> = sqlx::query_scalar(query)
        .bind(subject.0)
        .bind(id)
        .fetch_optional(&mut **tx)
        .await
        .map_err(db)?;
    match state.as_deref() {
        None => Err(Error::NotFound(
            "UseEvent revision is not in Subject".into(),
        )),
        Some("normal") => Ok(()),
        Some(_) => Err(Error::FailedPrecondition(
            "UseEvent target is purging".into(),
        )),
    }
}
