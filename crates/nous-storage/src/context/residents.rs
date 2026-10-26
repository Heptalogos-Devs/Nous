use super::*;
use crate::{Database, WriteTransaction, write_error};
use chrono::{DateTime, SecondsFormat, Utc};

pub(super) async fn read(
    conn: &mut SqliteConnection,
    id: &ContextSessionId,
    include_evicted: bool,
) -> Result<Vec<ContextResident>> {
    let rows=sqlx::query("SELECT * FROM context_residents WHERE session_id=? AND (? OR state<>'evicted') ORDER BY (state='resident') DESC,last_meaningful_use_at DESC,entered_at DESC,target")
        .bind(id.as_str()).bind(include_evicted).fetch_all(conn).await.map_err(storage_error)?;
    rows.into_iter()
        .map(|row| {
            Ok(ContextResident {
                target: decode(row.get("target"))?,
                state: serde_json::from_value(serde_json::Value::String(row.get("state")))?,
                entry_reason: row.get("entry_reason"),
                entered_at: row.get("entered_at"),
                last_meaningful_use_at: row.get("last_meaningful_use_at"),
                hold_until: row.get("hold_until"),
            })
        })
        .collect()
}
impl Database {
    pub async fn context_residents(
        &self,
        id: &ContextSessionId,
        include_evicted: bool,
    ) -> Result<Vec<ContextResident>> {
        read(
            &mut *self.pool.acquire().await.map_err(storage_error)?,
            id,
            include_evicted,
        )
        .await
    }
}
impl WriteTransaction {
    pub async fn apply_context_residents(
        &mut self,
        id: &ContextSessionId,
        expected_revision: i64,
        admissions: &[ContextResidentAdmission],
        limit: usize,
    ) -> Result<ContextResidentOutcome> {
        if admissions.len() > 256 || limit == 0 {
            return Err(Error::invalid("resident batch or limit is out of range"));
        }
        let mut current = session(&mut self.tx, id).await?;
        self.context()
            .require_subject(&current.namespace_id, &current.subject_id)?;
        if current.revision != expected_revision || current.closed_at.is_some() {
            return Err(Error::new(
                ErrorCode::Conflict,
                "resident Session changed or closed",
            ));
        }
        let targets = admissions
            .iter()
            .map(|item| item.target.clone())
            .collect::<Vec<_>>();
        self.validate_context_targets(&current.namespace_id, &current.subject_id, &targets)
            .await?;
        let mut changed = false;
        let mut admitted = 0;
        for admission in admissions {
            admission.validate()?;
            let existing=sqlx::query("SELECT state,hold_until,last_meaningful_use_at FROM context_residents WHERE session_id=? AND target=?").bind(id.as_str()).bind(encode(&admission.target)?).fetch_optional(&mut *self.tx).await.map_err(storage_error)?;
            let (state, hold, last, reentered) = if let Some(row) = existing {
                let state: String = row.get("state");
                let desired =
                    if state == "resident" || admission.state == ContextResidentState::Resident {
                        "resident"
                    } else {
                        "provisional"
                    };
                let hold = later(row.get("hold_until"), admission.hold_until.clone())?;
                let last = later(
                    row.get("last_meaningful_use_at"),
                    admission.last_meaningful_use_at.clone(),
                )?;
                changed |= desired != state
                    || hold != row.get::<Option<String>, _>("hold_until")
                    || last != row.get::<Option<String>, _>("last_meaningful_use_at");
                (desired, hold, last, state == "evicted")
            } else {
                changed = true;
                (
                    if admission.state == ContextResidentState::Resident {
                        "resident"
                    } else {
                        "provisional"
                    },
                    canonical(admission.hold_until.clone())?,
                    canonical(admission.last_meaningful_use_at.clone())?,
                    true,
                )
            };
            admitted += usize::from(reentered);
            sqlx::query("INSERT INTO context_residents(session_id,target,state,entry_reason,entered_at,last_meaningful_use_at,hold_until) VALUES(?,?,?,?,?,?,?) ON CONFLICT(session_id,target) DO UPDATE SET state=excluded.state,hold_until=excluded.hold_until,last_meaningful_use_at=excluded.last_meaningful_use_at,entered_at=CASE WHEN context_residents.state='evicted' THEN excluded.entered_at ELSE context_residents.entered_at END")
                .bind(id.as_str()).bind(encode(&admission.target)?).bind(state).bind(&admission.reason).bind(self.time().recorded()).bind(last).bind(hold).execute(&mut *self.tx).await.map_err(write_error)?;
        }
        let active = read(&mut self.tx, id, false).await?;
        let excess = active.len().saturating_sub(limit);
        let now = self.time().utc();
        let mut victims = active
            .into_iter()
            .filter(|resident| {
                resident
                    .hold_until
                    .as_ref()
                    .and_then(|time| DateTime::parse_from_rfc3339(time).ok())
                    .is_none_or(|hold| hold.with_timezone(&Utc) < now)
            })
            .collect::<Vec<_>>();
        victims.sort_by_key(|resident| {
            (
                resident.state != ContextResidentState::Provisional,
                resident.last_meaningful_use_at.is_some(),
                resident.last_meaningful_use_at.clone(),
                resident.entered_at.clone(),
            )
        });
        let mut evicted = 0;
        for resident in victims.into_iter().take(excess) {
            evicted += self.evict_resident_row(id, &resident.target).await?;
        }
        changed |= evicted > 0;
        if changed {
            let expected = current.revision;
            current.revision = self.commit_seq();
            current.commit_seq = self.commit_seq();
            current.recorded_at = self.time().recorded();
            self.save_context_session(&current, expected).await?;
        }
        Ok(ContextResidentOutcome {
            revision: current.revision,
            admitted,
            evicted,
        })
    }
    pub async fn evict_context_residents(
        &mut self,
        id: &ContextSessionId,
        expected_revision: i64,
        targets: Option<&[RetrievalTarget]>,
    ) -> Result<ContextResidentOutcome> {
        let mut current = session(&mut self.tx, id).await?;
        self.context()
            .require_subject(&current.namespace_id, &current.subject_id)?;
        if current.revision != expected_revision || current.closed_at.is_some() {
            return Err(Error::new(
                ErrorCode::Conflict,
                "resident Session changed or closed",
            ));
        }
        let targets = if let Some(targets) = targets {
            targets.to_vec()
        } else {
            read(&mut self.tx, id, false)
                .await?
                .into_iter()
                .map(|resident| resident.target)
                .collect()
        };
        let mut evicted = 0;
        for target in &targets {
            target.validate()?;
            evicted += self.evict_resident_row(id, target).await?;
        }
        if evicted > 0 {
            let expected = current.revision;
            current.revision = self.commit_seq();
            current.commit_seq = self.commit_seq();
            current.recorded_at = self.time().recorded();
            self.save_context_session(&current, expected).await?;
        }
        Ok(ContextResidentOutcome {
            revision: current.revision,
            admitted: 0,
            evicted,
        })
    }
    async fn evict_resident_row(
        &mut self,
        id: &ContextSessionId,
        target: &RetrievalTarget,
    ) -> Result<usize> {
        Ok(sqlx::query("UPDATE context_residents SET state='evicted' WHERE session_id=? AND target=? AND state<>'evicted'").bind(id.as_str()).bind(encode(target)?).execute(&mut *self.tx).await.map_err(write_error)?.rows_affected() as usize)
    }
}
fn canonical(time: Option<String>) -> Result<Option<String>> {
    time.map(|time| {
        DateTime::parse_from_rfc3339(&time)
            .map(|value| {
                value
                    .with_timezone(&Utc)
                    .to_rfc3339_opts(SecondsFormat::Nanos, true)
            })
            .map_err(|_| Error::invalid("resident timestamp is invalid"))
    })
    .transpose()
}
fn later(left: Option<String>, right: Option<String>) -> Result<Option<String>> {
    let left = canonical(left)?;
    let right = canonical(right)?;
    Ok(left.max(right))
}
