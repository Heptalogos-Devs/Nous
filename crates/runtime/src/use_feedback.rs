// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use crate::*;
use nous_persistence::database_error as db;
use sqlx::Row;
use std::collections::HashMap;

impl CognitiveRuntimeService {
    /// Erase Runtime references while the cognition owner holds its purge transaction.
    pub async fn purge_cognition_refs_in(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        subject: SubjectId,
        kind: &str,
        revisions: &[String],
    ) -> Result<()> {
        sqlx::query("INSERT INTO purged_use_receipts(subject_id,consumer_ref,event_id,request_digest,purged_at) SELECT subject_id,consumer_ref,event_id,request_digest,$4 FROM cognitive_use_events WHERE subject_id=$1 AND ref_kind=$2 AND ref_value=ANY($3::text[]) ON CONFLICT DO NOTHING")
            .bind(subject.0).bind(kind).bind(revisions).bind(self.now(subject)).execute(&mut **tx).await.map_err(db)?;
        sqlx::query("DELETE FROM cognitive_use_events WHERE subject_id=$1 AND ref_kind=$2 AND ref_value=ANY($3::text[])")
            .bind(subject.0).bind(kind).bind(revisions).execute(&mut **tx).await.map_err(db)?;
        self.forget_resident_cognition_in(tx, subject, kind, revisions)
            .await?;
        sqlx::query("DELETE FROM work_context_refs wr USING work_contexts w WHERE wr.work_context_id=w.work_context_id AND w.subject_id=$1 AND wr.ref_kind=$2 AND wr.ref_value=ANY($3::text[])")
            .bind(subject.0).bind(kind).bind(revisions).execute(&mut **tx).await.map_err(db)?;
        Ok(())
    }

    pub async fn forget_resident_cognition_in(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        subject: SubjectId,
        kind: &str,
        revisions: &[String],
    ) -> Result<()> {
        sqlx::query("DELETE FROM resident_refs rr USING cognitive_sessions s WHERE rr.session_id=s.session_id AND s.subject_id=$1 AND rr.ref_kind=$2 AND rr.ref_value=ANY($3::text[])")
            .bind(subject.0).bind(kind).bind(revisions).execute(&mut **tx).await.map_err(db)?;
        Ok(())
    }

    #[expect(
        clippy::too_many_lines,
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
        let current_session_revision = if let Some(session) = input.session_id {
            Some(
                self.lock_open_session_in(&mut tx, input.subject, session)
                    .await?,
            )
        } else {
            None
        };
        #[derive(Clone)]
        struct Prepared {
            event: UseFeedbackEvent,
            kind: String,
            value: String,
            digest: String,
            duplicate: bool,
        }
        let event_ids = input
            .events
            .iter()
            .map(|event| event.event_id.0)
            .collect::<Vec<_>>();
        let rows = sqlx::query("SELECT event_id,request_digest,0 AS source FROM cognitive_use_events WHERE subject_id=$1 AND consumer_ref=$2 AND event_id=ANY($3) UNION ALL SELECT event_id,request_digest,1 AS source FROM purged_use_receipts WHERE subject_id=$1 AND consumer_ref=$2 AND event_id=ANY($3) ORDER BY source")
            .bind(input.subject.0).bind(&input.consumer_ref).bind(event_ids).fetch_all(&mut *tx).await.map_err(db)?;
        let mut receipts = HashMap::new();
        for row in rows {
            receipts
                .entry(UseEventId(row.try_get("event_id").map_err(db)?))
                .or_insert(row.try_get::<String, _>("request_digest").map_err(db)?);
        }
        let mut validated_targets = HashSet::new();
        let mut validated_queries = HashSet::new();
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
            let stored_digest = receipts.get(&event.event_id);
            if let Some(stored) = stored_digest
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
                if validated_targets.insert(event.reference.clone()) {
                    validate_use_target_in(&mut tx, input.subject, &event.reference).await?;
                }
                if let Some(query_id) = event.query_id
                    && validated_queries.insert((query_id, event.reference.clone()))
                {
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
        for item in prepared {
            if item.duplicate {
                duplicates += 1;
                continue;
            }
            sqlx::query("INSERT INTO cognitive_use_events(subject_id,consumer_ref,event_id,ref_kind,ref_value,use_kind,session_id,occurred_at,recorded_at,context,request_digest,query_id) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12)")
                .bind(input.subject.0).bind(&input.consumer_ref).bind(item.event.event_id.0).bind(&item.kind).bind(&item.value).bind(item.event.use_kind.as_str()).bind(input.session_id.map(|id|id.0)).bind(item.event.occurred_at).bind(recorded_at).bind(&item.event.context).bind(&item.digest).bind(item.event.query_id).execute(&mut *tx).await.map_err(db)?;
            accepted += 1;
            if item.event.use_kind.meaningful() {
                meaningful_times.push((item.event.reference, item.event.occurred_at));
            }
        }
        let session_revision = if let Some(session) = input.session_id {
            let revision =
                current_session_revision.expect("Session was locked before UseEvent writes");
            if accepted == 0 {
                Some(revision)
            } else {
                let admissions = meaningful_times
                    .iter()
                    .map(|(reference, at)| ResidentAdmission {
                        reference: reference.clone(),
                        reason: "meaningful_use".into(),
                        last_meaningful_use_at: Some(*at),
                        hold_until: None,
                        state: ResidentState::Resident,
                    })
                    .collect();
                Some(
                    self.apply_resident_batch_in(
                        &mut tx,
                        input.subject,
                        session,
                        revision,
                        admissions,
                        Some(recorded_at),
                    )
                    .await?
                    .runtime_revision,
                )
            }
        } else {
            None
        };
        if !meaningful_times.is_empty() {
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
            let mut added = HashMap::<(String, String), u64>::new();
            for (reference, _) in &meaningful_times {
                *added.entry(reference_parts(reference)).or_default() += 1;
            }
            let (kinds, values): (Vec<_>, Vec<_>) = added.keys().cloned().unzip();
            let rows = sqlx::query("SELECT e.ref_kind,e.ref_value,count(*) AS count FROM cognitive_use_events e JOIN unnest($2::text[],$3::text[]) refs(kind,value) ON e.ref_kind=refs.kind AND e.ref_value=refs.value WHERE e.subject_id=$1 AND e.use_kind<>'presented' GROUP BY e.ref_kind,e.ref_value")
                .bind(input.subject.0).bind(kinds).bind(values).fetch_all(&mut *tx).await.map_err(db)?;
            for row in rows {
                let kind: String = row.try_get("ref_kind").map_err(db)?;
                let value: String = row.try_get("ref_value").map_err(db)?;
                let count = u64::try_from(row.try_get::<i64, _>("count").map_err(db)?)
                    .map_err(|_| Error::Infrastructure("invalid use count".into()))?;
                let added = added[&(kind.clone(), value.clone())];
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
    if value.is_empty() || value.len() > 512 || value.chars().any(char::is_whitespace) {
        return Err(Error::Invalid(
            "consumer_ref must be a nonempty opaque identity of at most 512 bytes without whitespace".into(),
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
            "SELECT true FROM memory_revisions r JOIN memory_objects o USING(memory_id) WHERE r.subject_id=$1 AND r.memory_revision_id=$2 FOR SHARE OF o",
            id.0,
        ),
        CognitiveRef::CognitiveSchemaRevision(id) => (
            "SELECT true FROM cognitive_schema_revisions r JOIN cognitive_schemas o USING(schema_id) WHERE o.subject_id=$1 AND r.schema_revision_id=$2 FOR SHARE OF o",
            id.0,
        ),
        CognitiveRef::EpisodeRevision(id) => (
            "SELECT true FROM episode_revisions r JOIN episode_objects o USING(episode_id) WHERE r.subject_id=$1 AND r.episode_revision_id=$2 FOR SHARE OF o",
            id.0,
        ),
        CognitiveRef::JournalRevision(id) => (
            "SELECT true FROM journal_revisions r JOIN journal_objects o USING(journal_id) WHERE r.subject_id=$1 AND r.journal_revision_id=$2 FOR SHARE OF o",
            id.0,
        ),
        _ => {
            return Err(Error::Invalid(
                "UseEvent requires an exact cognition revision".into(),
            ));
        }
    };
    let exists: Option<bool> = sqlx::query_scalar(query)
        .bind(subject.0)
        .bind(id)
        .fetch_optional(&mut **tx)
        .await
        .map_err(db)?;
    exists
        .map(|_| ())
        .ok_or_else(|| Error::NotFound("UseEvent revision is not in Subject".into()))
}
