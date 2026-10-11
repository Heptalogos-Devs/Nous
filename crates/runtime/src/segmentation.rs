//! Transactional feed cursor and deterministic Episode drafts.

// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use crate::*;
use nous_persistence::database_error as db;
use serde::{Deserialize, Serialize};
use sqlx::{Postgres, Row, Transaction};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpisodeDraft {
    pub draft_id: Uuid,
    pub subject_id: SubjectId,
    pub track_key: String,
    pub first_recorded_seq: i64,
    pub last_recorded_seq: i64,
    pub observed_start: DateTime<Utc>,
    pub observed_end: DateTime<Utc>,
    pub boundary_reason: Option<String>,
    pub members: Vec<OccurrenceId>,
}

#[derive(Debug, Clone)]
pub struct SegmentationProgress {
    pub processed_count: usize,
    pub ready: Vec<EpisodeDraft>,
    pub next_due: Option<DateTime<Utc>>,
}

impl CognitiveRuntimeService {
    pub async fn segment_experience(
        &self,
        subject: SubjectId,
        track: &str,
        limit: u32,
        close: bool,
    ) -> Result<SegmentationProgress> {
        if track != "interaction" || !(1..=256).contains(&limit) {
            return Err(Error::new(
                nous_core::ErrorOwner::Runtime,
                nous_core::ErrorCode::InvalidRequest,
                "invalid automatic segmentation envelope",
            ));
        }
        self.require_subject(subject).await?;
        let policy =
            EpisodePolicy::from_snapshot(&self.configuration.snapshot_for_subject(subject)?)?;
        let now = self.now(subject);
        let mut tx = self.store.begin().await?;
        sqlx::query("INSERT INTO segmentation_cursors(subject_id,track_key,updated_at) VALUES($1,$2,$3) ON CONFLICT DO NOTHING")
            .bind(subject.0).bind(track).bind(now).execute(&mut *tx).await.map_err(db)?;
        let cursor = sqlx::query("SELECT last_recorded_seq,open_draft_id FROM segmentation_cursors WHERE subject_id=$1 AND track_key=$2 FOR UPDATE")
            .bind(subject.0).bind(track).fetch_one(&mut *tx).await.map_err(db)?;
        let mut last_seq: i64 = cursor.try_get("last_recorded_seq").map_err(db)?;
        let mut draft: Option<Uuid> = cursor.try_get("open_draft_id").map_err(db)?;
        let items = sqlx::query("SELECT * FROM experience_items WHERE subject_id=$1 AND recorded_seq>$2 ORDER BY recorded_seq LIMIT $3")
            .bind(subject.0).bind(last_seq).bind(i64::from(limit)).fetch_all(&mut *tx).await.map_err(db)?;
        for item in &items {
            let seq: i64 = item.try_get("recorded_seq").map_err(db)?;
            let at: DateTime<Utc> = item.try_get("observed_at").map_err(db)?;
            let late;
            if let Some(id) = draft {
                let previous = sqlx::query("SELECT d.observed_start,d.observed_end,e.*,w.state AS work_context_state,(SELECT count(*) FROM episode_draft_members WHERE draft_id=d.draft_id) AS member_count FROM episode_drafts d JOIN experience_items e ON e.subject_id=d.subject_id AND e.recorded_seq=d.last_recorded_seq LEFT JOIN work_contexts w ON w.work_context_id=e.active_work_context_id WHERE d.draft_id=$1")
                    .bind(id).fetch_one(&mut *tx).await.map_err(db)?;
                let start: DateTime<Utc> = previous.try_get("observed_start").map_err(db)?;
                let end: DateTime<Utc> = previous.try_get("observed_end").map_err(db)?;
                late = at < start;
                let ended = previous
                    .try_get::<Option<String>, _>("work_context_state")
                    .map_err(db)?
                    .as_deref()
                    == Some("ended");
                let reason = policy
                    .boundary(&context(&previous)?, &context(item)?, end, at, ended)
                    .or_else(|| {
                        (previous.get::<i64, _>("member_count") >= 256).then_some("member_budget")
                    });
                if !late && let Some(reason) = reason {
                    close_draft(&mut tx, id, reason, now).await?;
                    draft = None;
                }
                if at < end {
                    self.late_experience_need(&mut tx, subject, track, seq, now)
                        .await?;
                }
            } else {
                let prior_end: Option<DateTime<Utc>> = sqlx::query_scalar("SELECT max(end_at) FROM (SELECT observed_end AS end_at FROM episode_drafts WHERE subject_id=$1 AND track_key=$2 AND state='ready' UNION ALL SELECT COALESCE(r.experience_time_end,r.experience_time_start) FROM cognition_objects o JOIN episode_revisions r ON r.episode_revision_id=o.current_revision_id WHERE o.object_kind='episode' AND o.subject_id=$1 AND o.track_key=$2 AND o.acceptance_state='accepted') prior")
                    .bind(subject.0).bind(track).fetch_one(&mut *tx).await.map_err(db)?;
                late = prior_end.is_some_and(|end| at <= end);
            }
            if late {
                self.late_experience_need(&mut tx, subject, track, seq, now)
                    .await?;
            } else {
                let id = match draft {
                    Some(id) => id,
                    None => {
                        let id = Uuid::now_v7();
                        sqlx::query("INSERT INTO episode_drafts(draft_id,subject_id,track_key,state,first_recorded_seq,last_recorded_seq,observed_start,observed_end,created_at,updated_at) VALUES($1,$2,$3,'open',$4,$4,$5,$5,$6,$6)")
                            .bind(id).bind(subject.0).bind(track).bind(seq).bind(at).bind(now).execute(&mut *tx).await.map_err(db)?;
                        draft = Some(id);
                        id
                    }
                };
                sqlx::query("INSERT INTO episode_draft_members(draft_id,recorded_seq,occurrence_id) VALUES($1,$2,$3)")
                    .bind(id).bind(seq).bind(item.get::<Uuid,_>("occurrence_id")).execute(&mut *tx).await.map_err(db)?;
                sqlx::query("UPDATE episode_drafts SET last_recorded_seq=$2,observed_end=GREATEST(observed_end,$3),updated_at=$4 WHERE draft_id=$1")
                    .bind(id).bind(seq).bind(at).bind(now).execute(&mut *tx).await.map_err(db)?;
            }
            last_seq = seq;
        }
        let (next_draft, mut next_due) = close_if_due(&mut tx, draft, &policy, now, close).await?;
        draft = next_draft;
        let backlog: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM experience_items WHERE subject_id=$1 AND recorded_seq>$2)",
        )
        .bind(subject.0)
        .bind(last_seq)
        .fetch_one(&mut *tx)
        .await
        .map_err(db)?;
        if backlog {
            next_due = Some(now);
        }
        sqlx::query("UPDATE segmentation_cursors SET last_recorded_seq=$3,open_draft_id=$4,updated_at=$5 WHERE subject_id=$1 AND track_key=$2")
            .bind(subject.0).bind(track).bind(last_seq).bind(draft).bind(now).execute(&mut *tx).await.map_err(db)?;
        let ready = ready_drafts(&mut tx, subject, track).await?;
        tx.commit().await.map_err(db)?;
        Ok(SegmentationProgress {
            processed_count: items.len(),
            ready,
            next_due,
        })
    }

    async fn late_experience_need(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        subject: SubjectId,
        track: &str,
        seq: i64,
        now: DateTime<Utc>,
    ) -> Result<()> {
        self.enqueue_maintenance_in(
            tx,
            &MaintenanceRequest {
                subject,
                kind: "episode_resegment".into(),
                scope_kind: "track".into(),
                scope_ref: track.into(),
                trigger_authority_seq: seq,
                due_at: now,
                priority: 60,
            },
        )
        .await?;
        Ok(())
    }

    pub async fn acknowledge_episode_draft(
        &self,
        subject: SubjectId,
        draft: Uuid,
        revision: EpisodeRevisionId,
    ) -> Result<()> {
        let mut tx = self.store.begin().await?;
        let operation = Uuid::new_v5(
            &Uuid::NAMESPACE_OID,
            format!("episode-draft:{draft}").as_bytes(),
        );
        let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM mutation_receipts r CROSS JOIN LATERAL jsonb_array_elements(r.results) AS receipt_result(outcome) WHERE r.subject_id=$1 AND r.operation_id=$3 AND r.operation_kind='create_episode' AND r.state='committed' AND outcome->>'revision_id'=$2::text AND outcome->'reference'->>'kind'='episode')")
            .bind(subject.0).bind(revision.0.to_string()).bind(operation).fetch_one(&mut *tx).await.map_err(db)?;
        if !exists {
            return Err(Error::new(
                nous_core::ErrorOwner::Runtime,
                nous_core::ErrorCode::FailedPrecondition,
                "committed Episode revision is foreign or missing",
            ));
        }
        let _changed = sqlx::query(
            "DELETE FROM episode_drafts WHERE subject_id=$1 AND draft_id=$2 AND state='ready'",
        )
        .bind(subject.0)
        .bind(draft)
        .execute(&mut *tx)
        .await
        .map_err(db)?;

        tx.commit().await.map_err(db)
    }
}

fn context(row: &sqlx::postgres::PgRow) -> Result<ExperienceContext> {
    Ok(ExperienceContext {
        session: SessionId(row.try_get("session_id").map_err(db)?),
        work_context: row.try_get("active_work_context_id").map_err(db)?,
        conversation: row.try_get("conversation_ref").map_err(db)?,
        actor: row.try_get("actor_entity_ref").map_err(db)?,
        source_class: row.try_get("source_class").map_err(db)?,
    })
}

async fn close_draft(
    tx: &mut Transaction<'_, Postgres>,
    draft: Uuid,
    reason: &str,
    now: DateTime<Utc>,
) -> Result<()> {
    sqlx::query("UPDATE episode_drafts SET state='ready',boundary_reason=$2,updated_at=$3 WHERE draft_id=$1 AND state='open'")
        .bind(draft).bind(reason).bind(now).execute(&mut **tx).await.map_err(db)?;
    Ok(())
}

async fn ready_drafts(
    tx: &mut Transaction<'_, Postgres>,
    subject: SubjectId,
    track: &str,
) -> Result<Vec<EpisodeDraft>> {
    let rows = sqlx::query("SELECT d.*,array_agg(m.occurrence_id ORDER BY e.observed_at,m.recorded_seq) AS members FROM episode_drafts d JOIN episode_draft_members m USING(draft_id) JOIN experience_items e ON e.subject_id=d.subject_id AND e.recorded_seq=m.recorded_seq WHERE d.subject_id=$1 AND d.track_key=$2 AND d.state='ready' GROUP BY d.draft_id ORDER BY d.first_recorded_seq LIMIT 128")
        .bind(subject.0).bind(track).fetch_all(&mut **tx).await.map_err(db)?;
    rows.into_iter()
        .map(|row| {
            Ok(EpisodeDraft {
                draft_id: row.try_get("draft_id").map_err(db)?,
                subject_id: subject,
                track_key: track.into(),
                first_recorded_seq: row.try_get("first_recorded_seq").map_err(db)?,
                last_recorded_seq: row.try_get("last_recorded_seq").map_err(db)?,
                observed_start: row.try_get("observed_start").map_err(db)?,
                observed_end: row.try_get("observed_end").map_err(db)?,
                boundary_reason: row.try_get("boundary_reason").map_err(db)?,
                members: row
                    .try_get::<Vec<Uuid>, _>("members")
                    .map_err(db)?
                    .into_iter()
                    .map(OccurrenceId)
                    .collect(),
            })
        })
        .collect()
}

async fn close_if_due(
    tx: &mut Transaction<'_, Postgres>,
    mut draft: Option<Uuid>,
    policy: &EpisodePolicy,
    now: DateTime<Utc>,
    close: bool,
) -> Result<(Option<Uuid>, Option<DateTime<Utc>>)> {
    let mut next_due = None;
    if let Some(id) = draft {
        let row = sqlx::query("SELECT d.observed_end,w.state AS work_context_state FROM episode_drafts d JOIN experience_items e ON e.subject_id=d.subject_id AND e.recorded_seq=d.last_recorded_seq LEFT JOIN work_contexts w ON w.work_context_id=e.active_work_context_id WHERE d.draft_id=$1")
                .bind(id).fetch_one(&mut **tx).await.map_err(db)?;
        let end: DateTime<Utc> = row.try_get("observed_end").map_err(db)?;
        let due = end.checked_add_signed(policy.hard_idle).ok_or_else(|| {
            Error::new(
                nous_core::ErrorOwner::Runtime,
                nous_core::ErrorCode::InvalidRequest,
                "Episode due time overflow",
            )
        })?;
        let ended = row
            .try_get::<Option<String>, _>("work_context_state")
            .map_err(db)?
            .as_deref()
            == Some("ended");
        if close || ended || now >= due {
            close_draft(
                tx,
                id,
                if close {
                    "explicit_close"
                } else if ended {
                    "work_context_ended"
                } else {
                    "hard_idle"
                },
                now,
            )
            .await?;
            draft = None;
        } else {
            next_due = Some(due);
        }
    }
    Ok((draft, next_due))
}
