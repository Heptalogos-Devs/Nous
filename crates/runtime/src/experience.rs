//! Session experience captured in the Material Authority transaction.

// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use crate::*;
use nous_persistence::database_error as db;
use sqlx::{Postgres, Row, Transaction};

pub struct ExperienceInput {
    pub subject: SubjectId,
    pub recorded_seq: i64,
    pub occurrence: OccurrenceId,
    pub session: SessionId,
    pub observed_at: DateTime<Utc>,
    pub source_class: String,
    pub conversation_ref: Option<String>,
    pub actor_entity_ref: Option<String>,
}

impl CognitiveRuntimeService {
    pub async fn capture_experience_in(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        input: ExperienceInput,
    ) -> Result<()> {
        let row = sqlx::query(
            "SELECT s.active_work_context_id,w.revision AS work_context_revision FROM cognitive_sessions s LEFT JOIN work_contexts w ON w.work_context_id=s.active_work_context_id AND w.subject_id=s.subject_id WHERE s.subject_id=$1 AND s.session_id=$2 AND s.closed_at IS NULL FOR SHARE OF s",
        )
        .bind(input.subject.0)
        .bind(input.session.0)
        .fetch_optional(&mut **tx)
        .await
        .map_err(db)?
        .ok_or_else(|| crate::sessions::stale_session(input.session))?;
        sqlx::query(
            "INSERT INTO experience_items(subject_id,recorded_seq,occurrence_id,session_id,observed_at,source_class,conversation_ref,actor_entity_ref,active_work_context_id,active_work_context_revision) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)",
        )
        .bind(input.subject.0)
        .bind(input.recorded_seq)
        .bind(input.occurrence.0)
        .bind(input.session.0)
        .bind(input.observed_at)
        .bind(input.source_class)
        .bind(input.conversation_ref)
        .bind(input.actor_entity_ref)
        .bind(row.try_get::<Option<Uuid>, _>("active_work_context_id").map_err(db)?)
        .bind(row.try_get::<Option<i64>, _>("work_context_revision").map_err(db)?)
        .execute(&mut **tx)
        .await
        .map_err(db)?;
        self.enqueue_maintenance_in(
            tx,
            &MaintenanceRequest {
                subject: input.subject,
                kind: "episode_segment".into(),
                scope_kind: "track".into(),
                scope_ref: "interaction".into(),
                trigger_authority_seq: input.recorded_seq,
                due_at: self.now(input.subject),
                priority: 50,
            },
        )
        .await?;
        Ok(())
    }
}
