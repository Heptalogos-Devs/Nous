//! Runtime history barriers for host transcripts derived from admitted cognition.
// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
use crate::*;
use nous_persistence::database_error as db;

impl CognitiveRuntimeService {
    /// Run before erasing use lineage, in the owning cognition mutation transaction.
    pub async fn invalidate_session_history_in(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        subject: SubjectId,
        kind: &str,
        revisions: &[String],
    ) -> Result<()> {
        let mut sources =
            AuthorityStore::self_dependency_references_in(tx, subject, kind, revisions).await?;
        sources.extend(
            revisions
                .iter()
                .map(|value| (kind.to_string(), value.clone())),
        );
        let (kinds, values): (Vec<_>, Vec<_>) = sources.into_iter().unzip();
        sqlx::query("WITH RECURSIVE parents(session_id) AS (SELECT e.session_id FROM cognitive_use_events e JOIN unnest($2::text[],$3::text[]) refs(kind,value) ON e.ref_kind=refs.kind AND e.ref_value=refs.value WHERE e.subject_id=$1 AND session_id IS NOT NULL AND (context @> '{\"model_input\":true}'::jsonb OR (context ? 'invocation_id' AND context ? 'projection_id')) UNION SELECT s.parent_session_id FROM cognitive_sessions s JOIN parents c ON c.session_id=s.session_id WHERE s.subject_id=$1 AND s.parent_session_id IS NOT NULL), affected(session_id) AS (SELECT session_id FROM parents UNION SELECT s.session_id FROM cognitive_sessions s JOIN affected p ON s.parent_session_id=p.session_id WHERE s.subject_id=$1) UPDATE cognitive_sessions s SET history_invalidated=true,runtime_revision=runtime_revision+1 WHERE s.subject_id=$1 AND s.session_id IN (SELECT session_id FROM affected) AND NOT s.history_invalidated")
            .bind(subject.0).bind(kinds).bind(values).execute(&mut **tx).await.map_err(db)?;
        Ok(())
    }
    pub(crate) async fn require_safe_history_in(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        subject: SubjectId,
        session: SessionId,
    ) -> Result<()> {
        let invalidated = self
            .history_invalidated_in(&mut *tx, subject, session)
            .await?;
        if invalidated {
            return Err(DomainError::new(DomainErrorCode::StaleContext,
                "Session history contains withdrawn or purged cognition; reset the native session before model submission")
                .with_context("history_reset_required", "true").with_context("session_id", session.0).into());
        }
        Ok(())
    }
    pub(crate) async fn history_invalidated_in(
        &self,
        connection: &mut sqlx::PgConnection,
        subject: SubjectId,
        session: SessionId,
    ) -> Result<bool> {
        // Parent invalidation remains authoritative even if a child was created
        // concurrently with the owner's descendant UPDATE snapshot.
        sqlx::query_scalar("WITH RECURSIVE lineage AS (SELECT session_id,parent_session_id,history_invalidated FROM cognitive_sessions WHERE subject_id=$1 AND session_id=$2 UNION ALL SELECT s.session_id,s.parent_session_id,s.history_invalidated FROM cognitive_sessions s JOIN lineage c ON c.parent_session_id=s.session_id WHERE s.subject_id=$1) SELECT COALESCE(bool_or(history_invalidated),false) FROM lineage")
            .bind(subject.0).bind(session.0).fetch_one(connection).await.map_err(db)
    }
}
