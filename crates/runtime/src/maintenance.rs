//! Durable work obligations and trigger-fenced acknowledgement for host-granted maintenance.

// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use crate::*;
use nous_persistence::database_error as db;
use serde::{Deserialize, Serialize};
use sqlx::{Postgres, Row, Transaction};

pub const MAINTENANCE_KINDS: &[&str] = &[
    "episode_segment",
    "episode_resegment",
    "journal_review",
    "journal_revalidate",
    "memory_consolidate",
    "concept_maintenance",
];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MaintenanceNeed {
    pub need_id: Uuid,
    pub subject_id: SubjectId,
    pub kind: String,
    pub scope_kind: String,
    pub scope_ref: String,
    pub trigger_authority_seq: i64,
    pub trigger_revision: u64,
    pub due_at: DateTime<Utc>,
    pub priority: i32,
    pub state: String,
    pub retry_count: u32,
    pub last_problem_code: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct MaintenanceRequest {
    pub subject: SubjectId,
    pub kind: String,
    pub scope_kind: String,
    pub scope_ref: String,
    pub trigger_authority_seq: i64,
    pub due_at: DateTime<Utc>,
    pub priority: i32,
}

#[derive(Debug, Clone)]
pub enum MaintenanceDisposition {
    Satisfied,
    Obsolete,
    Blocked {
        problem_code: String,
    },
    Retry {
        delay_seconds: u32,
        problem_code: String,
    },
    Pending {
        due_at: Option<DateTime<Utc>>,
        problem_code: Option<String>,
    },
}

impl CognitiveRuntimeService {
    pub async fn enqueue_maintenance(&self, request: MaintenanceRequest) -> Result<Uuid> {
        self.require_subject(request.subject).await?;
        let mut tx = self.store.begin().await?;
        let id = self.enqueue_maintenance_in(&mut tx, &request).await?;
        tx.commit().await.map_err(db)?;
        Ok(id)
    }

    pub async fn enqueue_maintenance_in(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        request: &MaintenanceRequest,
    ) -> Result<Uuid> {
        if !MAINTENANCE_KINDS.contains(&request.kind.as_str())
            || !valid_identifier(&request.scope_kind)
            || request.scope_ref.is_empty()
            || request.scope_ref.len() > 512
            || request.trigger_authority_seq < 0
            || !(0..=100).contains(&request.priority)
        {
            return Err(Error::Invalid("invalid maintenance scope or policy".into()));
        }
        sqlx::query_scalar(
            "INSERT INTO maintenance_needs(need_id,subject_id,kind,scope_kind,scope_ref,trigger_authority_seq,due_at,priority,state,created_at,updated_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,'pending',$9,$9) ON CONFLICT(subject_id,kind,scope_kind,scope_ref) WHERE state IN ('pending','blocked') DO UPDATE SET trigger_revision=maintenance_needs.trigger_revision+1,state=CASE WHEN maintenance_needs.state='blocked' THEN 'pending' ELSE maintenance_needs.state END,retry_not_before=NULL,retry_count=0,blocked_config_digest=NULL, trigger_authority_seq=GREATEST(maintenance_needs.trigger_authority_seq,excluded.trigger_authority_seq),due_at=LEAST(maintenance_needs.due_at,excluded.due_at),priority=GREATEST(maintenance_needs.priority,excluded.priority),updated_at=excluded.updated_at RETURNING need_id",
        )
        .bind(Uuid::now_v7())
        .bind(request.subject.0)
        .bind(&request.kind)
        .bind(&request.scope_kind)
        .bind(&request.scope_ref)
        .bind(request.trigger_authority_seq)
        .bind(request.due_at)
        .bind(request.priority)
        .bind(self.now(request.subject))
        .fetch_one(&mut **tx)
        .await
        .map_err(db)
    }

    pub async fn wake_blocked_maintenance_in(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        request: &MaintenanceRequest,
    ) -> Result<()> {
        sqlx::query("UPDATE maintenance_needs SET state='pending',trigger_revision=trigger_revision+1,trigger_authority_seq=GREATEST(trigger_authority_seq,$5),due_at=$6,retry_not_before=NULL,retry_count=0,blocked_config_digest=NULL,updated_at=$6 WHERE subject_id=$1 AND kind=$2 AND scope_kind=$3 AND scope_ref=$4 AND state='blocked'")
            .bind(request.subject.0).bind(&request.kind).bind(&request.scope_kind).bind(&request.scope_ref).bind(request.trigger_authority_seq).bind(request.due_at).execute(&mut **tx).await.map_err(db)?;
        Ok(())
    }

    pub async fn select_maintenance(
        &self,
        subject: SubjectId,
        allowed_kinds: &[String],
        limit: u32,
        model_execution_digest: Option<&str>,
    ) -> Result<Vec<MaintenanceNeed>> {
        self.require_subject(subject).await?;
        if model_execution_digest.is_some_and(|digest| {
            digest.len() != 64
                || !digest
                    .bytes()
                    .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
        }) {
            return Err(Error::Invalid("invalid model execution digest".into()));
        }
        if limit == 0
            || limit > 128
            || allowed_kinds
                .iter()
                .any(|kind| !MAINTENANCE_KINDS.contains(&kind.as_str()))
        {
            return Err(Error::Invalid(
                "invalid maintenance selection envelope".into(),
            ));
        }
        let mut tx = self.store.begin().await?;
        let now = self.now(subject);
        let blocked_digest = self.maintenance_block_digest(subject)?;
        sqlx::query("UPDATE maintenance_needs SET state='pending',trigger_revision=trigger_revision+1,blocked_config_digest=NULL,retry_not_before=NULL,retry_count=0 WHERE subject_id=$1 AND state='blocked' AND (blocked_config_digest IS DISTINCT FROM $2 OR (last_problem_code='maintenance_retry_exhausted' AND $3::text IS NOT NULL AND model_execution_digest IS DISTINCT FROM $3))")
            .bind(subject.0).bind(blocked_digest).bind(model_execution_digest).execute(&mut *tx).await.map_err(db)?;
        self.cleanup_maintenance_in(&mut tx, subject).await?;
        let rows = sqlx::query(
            "WITH due AS (SELECT need_id FROM maintenance_needs WHERE subject_id=$1 AND kind=ANY($2::text[]) AND state='pending' AND EXISTS(SELECT 1 FROM subjects s WHERE s.subject_id=maintenance_needs.subject_id AND s.status='active') AND due_at<=$3 AND (retry_not_before IS NULL OR retry_not_before<=clock_timestamp()) ORDER BY priority DESC,due_at,need_id LIMIT $4), selected AS (UPDATE maintenance_needs n SET model_execution_digest=$5 FROM due WHERE n.need_id=due.need_id RETURNING n.*) SELECT * FROM selected ORDER BY priority DESC,due_at,need_id",
        )
        .bind(subject.0)
        .bind(allowed_kinds)
        .bind(now)
        .bind(i64::from(limit))
        .bind(model_execution_digest)
        .fetch_all(&mut *tx)
        .await
        .map_err(db)?;
        let needs = rows
            .into_iter()
            .map(need_from_row)
            .collect::<Result<Vec<_>>>()?;
        tx.commit().await.map_err(db)?;
        Ok(needs)
    }

    pub async fn acknowledge_maintenance(
        &self,
        selected: &MaintenanceNeed,
        disposition: MaintenanceDisposition,
    ) -> Result<()> {
        let retry_delay = match &disposition {
            MaintenanceDisposition::Retry { delay_seconds, .. } => {
                let maximum = self
                    .configuration
                    .snapshot_for_subject(selected.subject_id)?
                    .get(RETRY_MAX)?;
                if *delay_seconds == 0 || u64::from(*delay_seconds) > maximum {
                    return Err(Error::Invalid(
                        "retry delay exceeds maintenance policy".into(),
                    ));
                }
                Some(f64::from(*delay_seconds))
            }
            _ => None,
        };
        let (state, due, problem) = match disposition {
            MaintenanceDisposition::Blocked { problem_code } => {
                ("blocked", selected.due_at, Some(problem_code))
            }
            MaintenanceDisposition::Retry { problem_code, .. } => {
                ("pending", selected.due_at, Some(problem_code))
            }
            MaintenanceDisposition::Satisfied => ("satisfied", selected.due_at, None),
            MaintenanceDisposition::Obsolete => ("obsolete", selected.due_at, None),
            MaintenanceDisposition::Pending {
                due_at,
                problem_code,
            } => ("pending", due_at.unwrap_or(selected.due_at), problem_code),
        };
        // The captured trigger is the acknowledgement fence; new work remains pending.
        let mut tx = self.store.begin().await?;
        sqlx::query("UPDATE maintenance_needs SET state=$5,due_at=$6,last_problem_code=$7,updated_at=$8,retry_count=CASE WHEN $9::double precision IS NULL THEN 0 ELSE retry_count+1 END,retry_not_before=CASE WHEN $9::double precision IS NULL THEN NULL ELSE clock_timestamp()+make_interval(secs=>$9) END,terminal_at=CASE WHEN $5 IN ('satisfied','obsolete') THEN clock_timestamp() ELSE NULL END,blocked_config_digest=CASE WHEN $5='blocked' THEN $10 ELSE NULL END WHERE subject_id=$1 AND need_id=$2 AND state='pending' AND trigger_authority_seq=$3 AND trigger_revision=$4")
            .bind(selected.subject_id.0).bind(selected.need_id)
            .bind(selected.trigger_authority_seq).bind(selected.trigger_revision as i64)
            .bind(state).bind(due).bind(problem).bind(self.now(selected.subject_id))
            .bind(retry_delay).bind(self.maintenance_block_digest(selected.subject_id)?)
            .execute(&mut *tx).await.map_err(db)?;
        // Only maintenance-bound completed workflows lose their runtime purpose after ack.
        sqlx::query("DELETE FROM model_results WHERE subject_id=$1 AND maintenance_need_id=$2 AND maintenance_trigger_revision=$3 AND outcome IS NOT NULL")
            .bind(selected.subject_id.0).bind(selected.need_id).bind(selected.trigger_revision as i64).execute(&mut *tx).await.map_err(db)?;
        self.cleanup_maintenance_in(&mut tx, selected.subject_id)
            .await?;
        tx.commit().await.map_err(db)?;
        Ok(())
    }

    fn maintenance_block_digest(&self, subject: SubjectId) -> Result<String> {
        Ok(self
            .configuration
            .snapshot_for_subject(subject)?
            .effective_digest)
    }

    async fn cleanup_maintenance_in(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        subject: SubjectId,
    ) -> Result<()> {
        sqlx::query("DELETE FROM model_results w USING maintenance_needs n WHERE w.subject_id=$1 AND w.maintenance_need_id=n.need_id AND w.maintenance_trigger_revision<n.trigger_revision")
            .bind(subject.0).execute(&mut **tx).await.map_err(db)?;
        let retention = self
            .configuration
            .snapshot_for_subject(subject)?
            .get(TERMINAL_RETENTION)?;
        sqlx::query("DELETE FROM maintenance_needs WHERE subject_id=$1 AND state IN ('satisfied','obsolete') AND terminal_at<clock_timestamp()-make_interval(secs=>$2)")
            .bind(subject.0).bind(retention as f64).execute(&mut **tx).await.map_err(db)?;
        Ok(())
    }

    pub async fn maintenance_needs(&self, subject: SubjectId) -> Result<Vec<MaintenanceNeed>> {
        self.require_subject(subject).await?;
        sqlx::query("SELECT * FROM maintenance_needs WHERE subject_id=$1 AND state IN ('pending','blocked') ORDER BY priority DESC,due_at,need_id LIMIT 128")
            .bind(subject.0)
            .fetch_all(self.store.pool())
            .await
            .map_err(db)?
            .into_iter()
            .map(need_from_row)
            .collect()
    }
}

fn valid_identifier(value: &str) -> bool {
    (1..=64).contains(&value.len())
        && value.as_bytes()[0].is_ascii_lowercase()
        && value
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_')
}

fn need_from_row(row: sqlx::postgres::PgRow) -> Result<MaintenanceNeed> {
    Ok(MaintenanceNeed {
        need_id: row.try_get("need_id").map_err(db)?,
        subject_id: SubjectId(row.try_get("subject_id").map_err(db)?),
        kind: row.try_get("kind").map_err(db)?,
        scope_kind: row.try_get("scope_kind").map_err(db)?,
        scope_ref: row.try_get("scope_ref").map_err(db)?,
        trigger_authority_seq: row.try_get("trigger_authority_seq").map_err(db)?,
        trigger_revision: row.try_get::<i64, _>("trigger_revision").map_err(db)? as u64,
        due_at: row.try_get("due_at").map_err(db)?,
        priority: row.try_get("priority").map_err(db)?,
        state: row.try_get("state").map_err(db)?,
        retry_count: row.try_get::<i32, _>("retry_count").map_err(db)? as u32,
        last_problem_code: row.try_get("last_problem_code").map_err(db)?,
        created_at: row.try_get("created_at").map_err(db)?,
        updated_at: row.try_get("updated_at").map_err(db)?,
    })
}
