// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use crate::*;
use nous_persistence::database_error as db;
use sqlx::Row;
use std::collections::BTreeMap;
use uuid::Uuid;

impl CognitiveRuntimeService {
    pub async fn open_session(
        &self,
        subject: SubjectId,
        metadata: serde_json::Value,
    ) -> Result<SessionView> {
        self.require_subject(subject).await?;
        let session = SessionId::new();
        let now = self.now(subject);
        let mut tx = self.store.begin().await?;
        sqlx::query("INSERT INTO cognitive_sessions(session_id,subject_id,opened_at,last_activity_at,metadata) VALUES($1,$2,$3,$3,$4)")
            .bind(session.0).bind(subject.0).bind(now).bind(metadata)
            .execute(&mut *tx).await.map_err(db)?;
        self.store
            .ensure_identity_addresses_in(&mut tx, subject, &[CognitiveRef::Session(session)], "")
            .await?;
        tx.commit().await.map_err(db)?;
        self.session(subject, session).await
    }

    pub async fn close_session(
        &self,
        subject: SubjectId,
        session: SessionId,
    ) -> Result<SessionView> {
        self.require_session(subject, session).await?;
        sqlx::query("UPDATE cognitive_sessions SET closed_at=$3,last_activity_at=$3,runtime_revision=runtime_revision+1 WHERE subject_id=$1 AND session_id=$2")
            .bind(subject.0).bind(session.0).bind(self.now(subject))
            .execute(self.store.pool()).await.map_err(db)?;
        self.session(subject, session).await
    }

    pub async fn session(&self, subject: SubjectId, session: SessionId) -> Result<SessionView> {
        let mut tx = self.store.begin().await?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
            .execute(&mut *tx)
            .await
            .map_err(db)?;
        let session = self.session_in(subject, session, &mut tx).await?;
        tx.commit().await.map_err(db)?;
        Ok(session)
    }
    pub(crate) async fn session_in(
        &self,
        subject: SubjectId,
        session: SessionId,
        connection: &mut sqlx::PgConnection,
    ) -> Result<SessionView> {
        let row = sqlx::query("SELECT session_id,subject_id,opened_at,last_activity_at,last_meaningful_use_at,closed_at,runtime_revision,active_work_context_id FROM cognitive_sessions WHERE subject_id=$1 AND session_id=$2")
            .bind(subject.0).bind(session.0).fetch_optional(&mut *connection).await.map_err(db)?
            .ok_or_else(|| Error::NotFound("session not found".into()))?;
        let refs = sqlx::query("SELECT ref_kind,ref_value,entry_reason,state,entered_at,last_meaningful_use_at,hold_until FROM resident_refs WHERE session_id=$1 AND state <> 'evicted' ORDER BY entered_at")
            .bind(session.0).fetch_all(&mut *connection).await.map_err(db)?;
        let mut resident = Vec::with_capacity(refs.len());
        for row in refs {
            let kind: String = row.try_get("ref_kind").map_err(db)?;
            let value: String = row.try_get("ref_value").map_err(db)?;
            resident.push(ResidentView {
                reference: parse_reference(&kind, &value)?,
                entry_reason: row.try_get("entry_reason").map_err(db)?,
                state: row.try_get("state").map_err(db)?,
                entered_at: row.try_get("entered_at").map_err(db)?,
                last_meaningful_use_at: row.try_get("last_meaningful_use_at").map_err(db)?,
                hold_until: row.try_get("hold_until").map_err(db)?,
            });
        }
        Ok(SessionView {
            session_id: SessionId(row.try_get("session_id").map_err(db)?),
            subject_id: SubjectId(row.try_get("subject_id").map_err(db)?),
            opened_at: row.try_get("opened_at").map_err(db)?,
            last_activity_at: row.try_get("last_activity_at").map_err(db)?,
            last_meaningful_use_at: row.try_get("last_meaningful_use_at").map_err(db)?,
            closed_at: row.try_get("closed_at").map_err(db)?,
            runtime_revision: row.try_get("runtime_revision").map_err(db)?,
            active_work_context_id: row.try_get("active_work_context_id").map_err(db)?,
            resident,
        })
    }
    pub async fn require_session(&self, subject: SubjectId, session: SessionId) -> Result<()> {
        let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM cognitive_sessions WHERE subject_id=$1 AND session_id=$2 AND closed_at IS NULL)")
            .bind(subject.0)
            .bind(session.0)
            .fetch_one(self.store.pool())
            .await
            .map_err(db)?;
        if exists {
            Ok(())
        } else {
            Err(stale_session(session))
        }
    }

    pub async fn admit_batch(
        &self,
        subject: SubjectId,
        session: SessionId,
        admissions: Vec<ResidentAdmission>,
    ) -> Result<ResidentMutationOutcome> {
        self.require_subject(subject).await?;
        let merged = merge_admissions(admissions)?;
        let mut tx = self.store.begin().await?;
        let session_row = sqlx::query(
            "SELECT subject_id,closed_at,runtime_revision FROM cognitive_sessions WHERE session_id=$1 FOR UPDATE",
        )
        .bind(session.0)
        .fetch_optional(&mut *tx)
        .await
        .map_err(db)?
        .ok_or_else(|| stale_session(session))?;
        let owner: Uuid = session_row.try_get("subject_id").map_err(db)?;
        let closed_at: Option<DateTime<Utc>> = session_row.try_get("closed_at").map_err(db)?;
        if owner != subject.0 || closed_at.is_some() {
            return Err(stale_session(session));
        }
        let current_revision: i64 = session_row.try_get("runtime_revision").map_err(db)?;
        for admission in &merged {
            if !self
                .store
                .reference_in_subject_tx(&mut tx, subject, &admission.reference)
                .await?
            {
                return Err(Error::FailedPrecondition(
                    "resident reference is not owned by Subject".into(),
                ));
            }
        }
        let now = self.now(subject);
        let mut changed = false;
        let mut admitted_count = 0u32;
        for admission in &merged {
            let (kind, value) = reference_parts(&admission.reference);
            let existing = sqlx::query("SELECT state,hold_until,entry_reason FROM resident_refs WHERE session_id=$1 AND ref_kind=$2 AND ref_value=$3 FOR UPDATE")
                .bind(session.0)
                .bind(&kind)
                .bind(&value)
                .fetch_optional(&mut *tx)
                .await
                .map_err(db)?;
            if let Some(row) = existing {
                let current_state: String = row.try_get("state").map_err(db)?;
                let current_hold: Option<DateTime<Utc>> = row.try_get("hold_until").map_err(db)?;
                let next_state = stronger_state(&current_state, admission.state.as_str());
                let next_hold = later_hold(current_hold, admission.hold_until);
                if next_state != current_state || next_hold != current_hold {
                    sqlx::query("UPDATE resident_refs SET state=$4,hold_until=$5 WHERE session_id=$1 AND ref_kind=$2 AND ref_value=$3")
                        .bind(session.0)
                        .bind(&kind)
                        .bind(&value)
                        .bind(next_state)
                        .bind(next_hold)
                        .execute(&mut *tx)
                        .await
                        .map_err(db)?;
                    changed = true;
                }
            } else {
                sqlx::query("INSERT INTO resident_refs(session_id,ref_kind,ref_value,entered_at,entry_reason,hold_until,state,metadata) VALUES($1,$2,$3,$4,$5,$6,$7,'{}')")
                    .bind(session.0)
                    .bind(&kind)
                    .bind(&value)
                    .bind(now)
                    .bind(&admission.reason)
                    .bind(admission.hold_until)
                    .bind(admission.state.as_str())
                    .execute(&mut *tx)
                    .await
                    .map_err(db)?;
                changed = true;
                admitted_count += 1;
            }
        }
        let count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM resident_refs WHERE session_id=$1 AND state IN ('resident','provisional')",
        )
        .bind(session.0)
        .fetch_one(&mut *tx)
        .await
        .map_err(db)?;
        let mut evicted_count = 0u32;
        if count as usize > self.resident_limit {
            let excess = count as usize - self.resident_limit;
            let victims = sqlx::query("WITH victims AS (SELECT session_id,ref_kind,ref_value FROM resident_refs WHERE session_id=$1 AND state IN ('resident','provisional') AND (hold_until IS NULL OR hold_until < $2) ORDER BY (state='provisional') DESC,(last_meaningful_use_at IS NOT NULL) ASC,last_meaningful_use_at ASC NULLS FIRST,entered_at ASC LIMIT $3 FOR UPDATE) UPDATE resident_refs r SET state='evicted' FROM victims v WHERE r.session_id=v.session_id AND r.ref_kind=v.ref_kind AND r.ref_value=v.ref_value RETURNING r.ref_value")
                .bind(session.0)
                .bind(now)
                .bind(excess as i64)
                .fetch_all(&mut *tx)
                .await
                .map_err(db)?;
            evicted_count = victims.len() as u32;
            changed |= evicted_count > 0;
        }
        let runtime_revision = if changed {
            sqlx::query_scalar::<_, i64>(
                "UPDATE cognitive_sessions SET last_activity_at=$2,runtime_revision=runtime_revision+1 WHERE session_id=$1 RETURNING runtime_revision",
            )
            .bind(session.0)
            .bind(now)
            .fetch_one(&mut *tx)
            .await
            .map_err(db)?
        } else {
            current_revision
        };
        tx.commit().await.map_err(db)?;
        Ok(ResidentMutationOutcome {
            changed,
            runtime_revision,
            admitted_count,
            evicted_count,
        })
    }

    pub async fn resident_reference_strings(&self, session: SessionId) -> Result<HashSet<String>> {
        Ok(sqlx::query(
            "SELECT ref_kind,ref_value FROM resident_refs WHERE session_id=$1 AND state IN ('resident','provisional')",
        )
        .bind(session.0)
        .fetch_all(self.store.pool())
        .await
        .map_err(db)?
        .into_iter()
        .map(|row| {
            format!(
                "{}:{}",
                row.get::<String, _>("ref_kind"),
                row.get::<String, _>("ref_value")
            )
        })
        .collect())
    }
}

fn merge_admissions(admissions: Vec<ResidentAdmission>) -> Result<Vec<ResidentAdmission>> {
    let mut merged = BTreeMap::<String, ResidentAdmission>::new();
    for admission in admissions {
        if admission.state == ResidentState::Evicted {
            return Err(Error::Invalid(
                "resident admission state cannot be evicted".into(),
            ));
        }
        let key = admission.reference.to_string();
        if let Some(existing) = merged.get_mut(&key) {
            if admission.state == ResidentState::Resident {
                existing.state = ResidentState::Resident;
            }
            existing.hold_until = later_hold(existing.hold_until, admission.hold_until);
        } else {
            merged.insert(key, admission);
        }
    }
    Ok(merged.into_values().collect())
}

fn stronger_state(current: &str, requested: &str) -> &'static str {
    if current == "resident" || requested == "resident" {
        "resident"
    } else {
        "provisional"
    }
}

fn later_hold(
    current: Option<DateTime<Utc>>,
    requested: Option<DateTime<Utc>>,
) -> Option<DateTime<Utc>> {
    match (current, requested) {
        (Some(current), Some(requested)) => Some(current.max(requested)),
        (Some(current), None) => Some(current),
        (None, requested) => requested,
    }
}

pub(super) fn stale_session(session: SessionId) -> Error {
    DomainError::new(
        DomainErrorCode::StaleContext,
        "Session is closed or unavailable to the Subject",
    )
    .with_context("session_id", session.0)
    .into()
}
