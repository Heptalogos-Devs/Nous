//! Runtime-owned inputs for cognition maintenance planning.

// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use crate::*;
use nous_persistence::database_error as db;
use sqlx::Row;
use std::collections::BTreeMap;

pub struct UseSummary {
    pub kind: String,
    pub count: u64,
    pub last_used_at: DateTime<Utc>,
}

pub struct UseFact {
    pub reference: CognitiveRef,
    pub kind: String,
    pub occurred_at: DateTime<Utc>,
    pub event: UseEventRef,
}

const EXPERIENCE_ORDER: &str = "SELECT occurrence_id FROM experience_items WHERE subject_id=$1 AND occurrence_id=ANY($2::uuid[]) ORDER BY observed_at,recorded_seq";

impl CognitiveRuntimeService {
    pub async fn experience_order(
        &self,
        subject: SubjectId,
        occurrences: &[OccurrenceId],
    ) -> Result<Vec<OccurrenceId>> {
        let ids = occurrences.iter().map(|id| id.0).collect::<Vec<_>>();
        sqlx::query_scalar(EXPERIENCE_ORDER)
            .bind(subject.0)
            .bind(ids)
            .fetch_all(self.store.pool())
            .await
            .map_err(db)
            .map(|ids: Vec<Uuid>| ids.into_iter().map(OccurrenceId).collect())
    }

    pub async fn lock_experience_order_in(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        subject: SubjectId,
        occurrences: &[OccurrenceId],
    ) -> Result<Vec<OccurrenceId>> {
        let sql = format!("{EXPERIENCE_ORDER} FOR SHARE");
        sqlx::query_scalar(sqlx::AssertSqlSafe(sql.as_str()))
            .bind(subject.0)
            .bind(occurrences.iter().map(|id| id.0).collect::<Vec<_>>())
            .fetch_all(&mut **tx)
            .await
            .map_err(db)
            .map(|ids: Vec<Uuid>| ids.into_iter().map(OccurrenceId).collect())
    }

    pub async fn use_facts_in(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        subject: SubjectId,
        references: &[CognitiveRef],
        as_of: Option<DateTime<Utc>>,
    ) -> Result<Vec<UseFact>> {
        let (kinds, values): (Vec<_>, Vec<_>) = references.iter().map(reference_parts).unzip();
        let rows = sqlx::query("SELECT DISTINCT e.ref_kind,e.ref_value,e.use_kind,e.occurred_at,e.consumer_ref,e.event_id FROM cognitive_use_events e JOIN unnest($2::text[],$3::text[]) refs(kind,value) ON e.ref_kind=refs.kind AND e.ref_value=refs.value WHERE e.subject_id=$1 AND e.use_kind<>'presented' AND ($4::timestamptz IS NULL OR e.occurred_at<=$4) ORDER BY e.occurred_at DESC,e.event_id")
            .bind(subject.0).bind(kinds).bind(values).bind(as_of).fetch_all(&mut **tx).await.map_err(db)?;
        rows.into_iter()
            .map(|row| {
                Ok(UseFact {
                    reference: parse_reference(
                        &row.try_get::<String, _>("ref_kind").map_err(db)?,
                        &row.try_get::<String, _>("ref_value").map_err(db)?,
                    )?,
                    kind: row.try_get("use_kind").map_err(db)?,
                    occurred_at: row.try_get("occurred_at").map_err(db)?,
                    event: UseEventRef {
                        subject_id: subject,
                        consumer_ref: row.try_get("consumer_ref").map_err(db)?,
                        event_id: UseEventId(row.try_get("event_id").map_err(db)?),
                    },
                })
            })
            .collect()
    }

    pub async fn recent_use_events(
        &self,
        subject: SubjectId,
        reference: &CognitiveRef,
        kinds: &[&str],
        limit: usize,
    ) -> Result<Vec<UseEventRef>> {
        if limit == 0 || limit > 256 {
            return Err(Error::Invalid("use event limit must be 1..256".into()));
        }
        let (kind, value) = reference_parts(reference);
        let rows = sqlx::query("SELECT consumer_ref,event_id FROM cognitive_use_events WHERE subject_id=$1 AND ref_kind=$2 AND ref_value=$3 AND use_kind=ANY($4::text[]) ORDER BY occurred_at DESC,event_id LIMIT $5")
            .bind(subject.0).bind(kind).bind(value).bind(kinds).bind(limit as i64).fetch_all(self.store.pool()).await.map_err(db)?;
        rows.into_iter()
            .map(|row| {
                Ok(UseEventRef {
                    subject_id: subject,
                    consumer_ref: row.try_get("consumer_ref").map_err(db)?,
                    event_id: UseEventId(row.try_get("event_id").map_err(db)?),
                })
            })
            .collect()
    }

    /// A purged event remains a known support receipt, with no surviving use payload.
    pub async fn use_event_support(
        &self,
        subject: SubjectId,
        event: &UseEventRef,
    ) -> Result<Option<String>> {
        if event.subject_id != subject {
            return Err(Error::Invalid(
                "UseEvent support belongs to another Subject".into(),
            ));
        }
        let row = sqlx::query("SELECT (SELECT use_kind FROM cognitive_use_events WHERE subject_id=$1 AND consumer_ref=$2 AND event_id=$3) AS kind,EXISTS(SELECT 1 FROM purged_use_receipts WHERE subject_id=$1 AND consumer_ref=$2 AND event_id=$3) AS purged")
            .bind(subject.0).bind(&event.consumer_ref).bind(event.event_id.0).fetch_one(self.store.pool()).await.map_err(db)?;
        let kind: Option<String> = row.try_get("kind").map_err(db)?;
        if kind.is_none() && !row.try_get::<bool, _>("purged").map_err(db)? {
            return Err(Error::Invalid("UseEvent support was not recorded".into()));
        }
        Ok(kind)
    }

    pub async fn require_current_maintenance(&self, selected: &MaintenanceNeed) -> Result<()> {
        let valid: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM maintenance_needs WHERE subject_id=$1 AND need_id=$2 AND state='pending' AND trigger_authority_seq=$3 AND trigger_revision=$4 AND kind=$5 AND scope_kind=$6 AND scope_ref=$7)")
            .bind(selected.subject_id.0).bind(selected.need_id).bind(selected.trigger_authority_seq)
            .bind(selected.trigger_revision as i64).bind(&selected.kind).bind(&selected.scope_kind).bind(&selected.scope_ref)
            .fetch_one(self.store.pool()).await.map_err(db)?;
        if valid {
            Ok(())
        } else {
            Err(DomainError::new(
                DomainErrorCode::StaleRevision,
                "maintenance trigger changed",
            )
            .into())
        }
    }

    pub async fn use_summaries(
        &self,
        subject: SubjectId,
        references: &[CognitiveRef],
    ) -> Result<BTreeMap<String, Vec<UseSummary>>> {
        let (kinds, values): (Vec<_>, Vec<_>) = references.iter().map(reference_parts).unzip();
        let rows = sqlx::query("SELECT e.ref_kind,e.ref_value,e.use_kind,COUNT(*) AS count,MAX(e.occurred_at) AS last_used FROM cognitive_use_events e JOIN unnest($2::text[],$3::text[]) refs(kind,value) ON e.ref_kind=refs.kind AND e.ref_value=refs.value WHERE e.subject_id=$1 AND e.use_kind<>'presented' GROUP BY e.ref_kind,e.ref_value,e.use_kind ORDER BY e.ref_kind,e.ref_value,e.use_kind")
            .bind(subject.0).bind(kinds).bind(values).fetch_all(self.store.pool()).await.map_err(db)?;
        let mut result = BTreeMap::<String, Vec<UseSummary>>::new();
        for row in rows {
            let key = format!(
                "{}:{}",
                row.try_get::<String, _>("ref_kind").map_err(db)?,
                row.try_get::<String, _>("ref_value").map_err(db)?
            );
            result.entry(key).or_default().push(UseSummary {
                kind: row.try_get("use_kind").map_err(db)?,
                count: row.try_get::<i64, _>("count").map_err(db)? as u64,
                last_used_at: row.try_get("last_used").map_err(db)?,
            });
        }
        Ok(result)
    }
}

#[derive(Default)]
pub struct ExperienceMetadata {
    pub recorded_seq: Option<i64>,
    pub session_id: Option<Uuid>,
    pub work_context_id: Option<Uuid>,
    pub work_context_revision: Option<i64>,
}
impl CognitiveRuntimeService {
    pub async fn experience_metadata(
        &self,
        subject: SubjectId,
        occurrences: &[OccurrenceId],
    ) -> Result<std::collections::HashMap<OccurrenceId, ExperienceMetadata>> {
        let ids: Vec<_> = occurrences.iter().map(|id| id.0).collect();
        sqlx::query("SELECT occurrence_id,recorded_seq,session_id,active_work_context_id,active_work_context_revision FROM experience_items WHERE subject_id=$1 AND occurrence_id=ANY($2::uuid[])")
            .bind(subject.0).bind(ids).fetch_all(self.store.pool()).await.map_err(db)?.into_iter().map(|row| Ok((OccurrenceId(row.try_get("occurrence_id").map_err(db)?), ExperienceMetadata { recorded_seq: row.try_get("recorded_seq").map_err(db)?, session_id: row.try_get("session_id").map_err(db)?, work_context_id: row.try_get("active_work_context_id").map_err(db)?, work_context_revision: row.try_get("active_work_context_revision").map_err(db)? }))).collect()
    }
}
