// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
// Adapted from Nous 412b062082afb75ccfae4c3f69fc606d3e3cc862; see NOTICE.
use crate::codec::*;
use crate::{Database, WriteTransaction, not_found, storage_error, write_error};
use nous_domain::*;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{Row, SqliteConnection, sqlite::SqliteRow};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StructuredGroupState {
    Pending,
    Applied,
    Rejected,
    Conflict,
    Skipped,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StructuredFormationGroup {
    pub key: String,
    pub ordinal: i64,
    pub depends_on: Vec<String>,
    pub state: StructuredGroupState,
    pub result: Option<Value>,
    pub error: Option<Error>,
    pub commit_seq: Option<i64>,
}

impl PartialEq for StructuredFormationGroup {
    fn eq(&self, other: &Self) -> bool {
        self.key == other.key
            && self.ordinal == other.ordinal
            && self.depends_on == other.depends_on
            && self.state == other.state
            && self.result == other.result
            && self.commit_seq == other.commit_seq
            && self
                .error
                .as_ref()
                .map(|error| (error.code, error.message.as_str()))
                == other
                    .error
                    .as_ref()
                    .map(|error| (error.code, error.message.as_str()))
    }
}

impl Database {
    pub async fn structured_formation_groups(
        &self,
        job_id: &str,
    ) -> Result<Vec<StructuredFormationGroup>> {
        groups(
            &mut *self.pool.acquire().await.map_err(storage_error)?,
            job_id,
        )
        .await
    }
}

impl WriteTransaction {
    pub async fn structured_formation_groups(
        &mut self,
        job_id: &str,
    ) -> Result<Vec<StructuredFormationGroup>> {
        groups(&mut self.tx, job_id).await
    }

    /// Register the proposal's complete group graph once, in the transaction
    /// that saves the validated output. Dependencies keep stable group keys.
    pub async fn insert_structured_groups(
        &mut self,
        job_id: &str,
        call_id: &ModelCallId,
        values: &[StructuredFormationGroup],
    ) -> Result<()> {
        let run = self.structured_formation(job_id).await?;
        if run.is_retired() {
            return Err(Error::new(
                ErrorCode::Conflict,
                "structured formation input was retired",
            ));
        }
        let call = self.model_call(call_id).await?;
        if call.job_id != job_id || call.state != ModelCallState::Succeeded {
            return Err(Error::invalid(
                "group graph requires a successful call for this task",
            ));
        }
        let existing = self.structured_formation_groups(job_id).await?;
        if !existing.is_empty() {
            if existing == values {
                return Ok(());
            }
            return Err(Error::new(
                ErrorCode::Conflict,
                "structured group graph is already registered",
            ));
        }
        validate_groups(values)?;
        for value in values {
            sqlx::query("INSERT INTO structured_formation_groups(job_id,key,model_call_id,ordinal,depends_on,state,result,error,commit_seq) VALUES (?,?,?,?,?,?,?,?,?)")
                .bind(job_id).bind(&value.key).bind(call_id.as_str()).bind(value.ordinal).bind(serde_json::to_string(&value.depends_on)?).bind(enum_name(&value.state)?).bind(json_option(&value.result)?).bind(json_option(&value.error)?).bind(value.commit_seq)
                .execute(&mut *self.tx).await.map_err(write_error)?;
        }
        Ok(())
    }

    /// Call in the same operation transaction as this group's domain revisions.
    /// Terminal outcomes are immutable, so a crash before queue ACK cannot
    /// adopt the group twice or overwrite its conflict/skip result.
    pub async fn update_structured_group(
        &mut self,
        job_id: &str,
        value: &StructuredFormationGroup,
    ) -> Result<()> {
        let run = self.structured_formation(job_id).await?;
        if run.is_retired() {
            return Err(Error::new(
                ErrorCode::Conflict,
                "structured formation input was retired",
            ));
        }
        if matches!(run.job.state, JobState::Succeeded | JobState::Cancelled) {
            return Err(Error::new(
                ErrorCode::Conflict,
                "structured formation is terminal",
            ));
        }
        let groups = self.structured_formation_groups(job_id).await?;
        let previous = groups
            .iter()
            .find(|group| group.key == value.key)
            .ok_or_else(not_found)?;
        if previous.ordinal != value.ordinal || previous.depends_on != value.depends_on {
            return Err(Error::new(
                ErrorCode::Conflict,
                "structured group identity and dependencies are immutable",
            ));
        }
        if previous.state != StructuredGroupState::Pending {
            if previous == value {
                return Ok(());
            }
            return Err(Error::new(
                ErrorCode::Conflict,
                "structured group is already terminal",
            ));
        }
        if value.state == StructuredGroupState::Pending
            || value.commit_seq != Some(self.commit_seq())
        {
            return Err(Error::invalid(
                "settled group must use its authority transaction sequence",
            ));
        }
        let dependencies = value
            .depends_on
            .iter()
            .map(|key| {
                groups
                    .iter()
                    .find(|group| &group.key == key)
                    .ok_or_else(not_found)
            })
            .collect::<Result<Vec<_>>>()?;
        if value.state == StructuredGroupState::Applied
            && dependencies
                .iter()
                .any(|group| group.state != StructuredGroupState::Applied)
        {
            return Err(Error::new(
                ErrorCode::Conflict,
                "group dependencies have not been applied",
            ));
        }
        if value.state == StructuredGroupState::Skipped
            && !dependencies.iter().any(|group| {
                matches!(
                    group.state,
                    StructuredGroupState::Rejected
                        | StructuredGroupState::Conflict
                        | StructuredGroupState::Skipped
                )
            })
        {
            return Err(Error::invalid(
                "dependency skip requires a failed dependency",
            ));
        }
        sqlx::query("UPDATE structured_formation_groups SET state=?,result=?,error=?,commit_seq=? WHERE job_id=? AND key=? AND state='pending'")
            .bind(enum_name(&value.state)?).bind(json_option(&value.result)?).bind(json_option(&value.error)?).bind(value.commit_seq).bind(job_id).bind(&value.key)
            .execute(&mut *self.tx).await.map_err(write_error)?;
        Ok(())
    }
}

fn validate_groups(values: &[StructuredFormationGroup]) -> Result<()> {
    if values.len() > 64 {
        return Err(Error::invalid(
            "structured proposal must have at most 64 groups",
        ));
    }
    let mut keys = HashMap::new();
    let mut ordinals = HashSet::new();
    for value in values {
        nous_domain::validate_name(&value.key, "structured group key")?;
        if value.ordinal < 0
            || !ordinals.insert(value.ordinal)
            || keys.insert(value.key.as_str(), value).is_some()
            || value.state != StructuredGroupState::Pending
            || value.commit_seq.is_some()
            || value.result.is_some()
            || value.error.is_some()
        {
            return Err(Error::invalid(
                "new group needs a unique key/ordinal and an empty pending outcome",
            ));
        }
    }
    for value in values {
        let unique = value.depends_on.iter().collect::<HashSet<_>>();
        if unique.len() != value.depends_on.len()
            || value
                .depends_on
                .iter()
                .any(|key| key == &value.key || !keys.contains_key(key.as_str()))
        {
            return Err(Error::invalid("structured group has invalid dependencies"));
        }
    }
    let mut settled = HashSet::new();
    while settled.len() < values.len() {
        let before = settled.len();
        for value in values {
            if value
                .depends_on
                .iter()
                .all(|key| settled.contains(key.as_str()))
            {
                settled.insert(value.key.as_str());
            }
        }
        if settled.len() == before {
            return Err(Error::invalid(
                "structured group dependencies contain a cycle",
            ));
        }
    }
    Ok(())
}
async fn groups(conn: &mut SqliteConnection, id: &str) -> Result<Vec<StructuredFormationGroup>> {
    sqlx::query("SELECT * FROM structured_formation_groups WHERE job_id=? ORDER BY ordinal,key")
        .bind(id)
        .fetch_all(conn)
        .await
        .map_err(storage_error)?
        .into_iter()
        .map(group_row)
        .collect()
}
fn group_row(row: SqliteRow) -> Result<StructuredFormationGroup> {
    Ok(StructuredFormationGroup {
        key: row.get("key"),
        ordinal: row.get("ordinal"),
        depends_on: serde_json::from_str(row.get("depends_on"))?,
        state: parse_enum(row.get("state"))?,
        result: parse_optional(row.get("result"))?,
        error: parse_optional(row.get("error"))?,
        commit_seq: row.get("commit_seq"),
    })
}
