// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
use crate::*;
use sqlx::Row;
pub struct ReflectionStoredResult {
    pub operation: String,
    pub digest: String,
    pub proposal: Option<String>,
    pub outcome: Option<String>,
    pub omitted: bool,
    pub recorded_at: chrono::DateTime<chrono::Utc>,
}
pub struct ReflectionStatusRows {
    pub through: i64,
    pub last_operation: Option<String>,
    pub last_status: Option<String>,
    pub last_problem: Option<String>,
    pub pending: Vec<(uuid::Uuid, i64, i64, String, Option<String>)>,
    pub recent: Vec<ReflectionStoredResult>,
    pub more: bool,
}
impl AuthorityStore {
    pub async fn reflection_status(
        &self,
        subject: SubjectId,
        operation: Option<&str>,
        limit: usize,
        max_bytes: usize,
    ) -> Result<ReflectionStatusRows> {
        if limit == 0 || limit > 128 || max_bytes == 0 || max_bytes > 8 * 1024 * 1024 {
            return Err(Error::Invalid(
                "invalid reflection inspection budget".into(),
            ));
        }
        self.require_subject(subject).await?;
        let mut tx = self.read_view().await?;
        let row=sqlx::query("SELECT last_recorded_seq,last_operation_key,last_status,last_problem_code FROM subject_reflection_progress WHERE subject_id=$1").bind(subject.0).fetch_optional(&mut *tx).await.map_err(database_error)?;
        let (through, last_operation, last_status, last_problem) = match row {
            Some(r) => (
                r.try_get("last_recorded_seq").map_err(database_error)?,
                r.try_get("last_operation_key").map_err(database_error)?,
                r.try_get("last_status").map_err(database_error)?,
                r.try_get("last_problem_code").map_err(database_error)?,
            ),
            None => (0, None, None, None),
        };
        let pending=sqlx::query_as("SELECT need_id,trigger_revision,trigger_authority_seq,state,last_problem_code FROM maintenance_needs WHERE subject_id=$1 AND kind='subject_reflection' AND state IN ('pending','blocked') ORDER BY created_at LIMIT 128").bind(subject.0).fetch_all(&mut *tx).await.map_err(database_error)?;
        let rows=sqlx::query("WITH selected AS (SELECT *,sum(COALESCE(octet_length(proposal::text),0)+COALESCE(octet_length(outcome::text),0)) OVER(ORDER BY created_at DESC,operation_key) AS read_bytes FROM model_results WHERE subject_id=$1 AND owner='subject_reflection' AND ($2::text IS NULL OR operation_key=$2)) SELECT operation_key,semantic_digest,CASE WHEN read_bytes<=$3 THEN proposal->>'payload' END AS proposal_json,CASE WHEN read_bytes<=$3 THEN outcome->>'payload' END AS outcome_json,read_bytes>$3 AS omitted,created_at FROM selected ORDER BY created_at DESC,operation_key LIMIT $4").bind(subject.0).bind(operation).bind(max_bytes as i64).bind((limit+1) as i64).fetch_all(&mut *tx).await.map_err(database_error)?;
        let more = rows.len() > limit;
        let recent = rows
            .into_iter()
            .take(limit)
            .map(|r| {
                Ok(ReflectionStoredResult {
                    operation: r.try_get("operation_key").map_err(database_error)?,
                    digest: r.try_get("semantic_digest").map_err(database_error)?,
                    proposal: r.try_get("proposal_json").map_err(database_error)?,
                    outcome: r.try_get("outcome_json").map_err(database_error)?,
                    omitted: r.try_get("omitted").map_err(database_error)?,
                    recorded_at: r.try_get("created_at").map_err(database_error)?,
                })
            })
            .collect::<Result<_>>()?;
        tx.commit().await.map_err(database_error)?;
        Ok(ReflectionStatusRows {
            through,
            last_operation,
            last_status,
            last_problem,
            pending,
            recent,
            more,
        })
    }
}
