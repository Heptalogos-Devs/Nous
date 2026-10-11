use super::*;

#[derive(Debug, Clone, Copy)]
enum WorkContextTransition {
    Pause,
    Resume,
    End,
}

impl CognitiveRuntimeService {
    pub async fn pause_work_context(
        &self,
        subject: SubjectId,
        work_context_id: Uuid,
        expected_revision: i64,
        operation_id: OperationId,
    ) -> Result<WorkContextView> {
        self.transition_work_context(
            subject,
            work_context_id,
            expected_revision,
            operation_id,
            WorkContextTransition::Pause,
        )
        .await
    }

    pub async fn resume_work_context(
        &self,
        subject: SubjectId,
        work_context_id: Uuid,
        expected_revision: i64,
        operation_id: OperationId,
    ) -> Result<WorkContextView> {
        self.transition_work_context(
            subject,
            work_context_id,
            expected_revision,
            operation_id,
            WorkContextTransition::Resume,
        )
        .await
    }

    pub async fn end_work_context(
        &self,
        subject: SubjectId,
        work_context_id: Uuid,
        expected_revision: i64,
        operation_id: OperationId,
    ) -> Result<WorkContextView> {
        self.transition_work_context(
            subject,
            work_context_id,
            expected_revision,
            operation_id,
            WorkContextTransition::End,
        )
        .await
    }

    async fn transition_work_context(
        &self,
        subject: SubjectId,
        work_context_id: Uuid,
        expected_revision: i64,
        operation_id: OperationId,
        transition: WorkContextTransition,
    ) -> Result<WorkContextView> {
        let kind = match transition {
            WorkContextTransition::Pause => "pause_work_context",
            WorkContextTransition::Resume => "resume_work_context",
            WorkContextTransition::End => "end_work_context",
        };
        let digest = canonical_request_digest(
            kind,
            subject,
            &serde_json::json!({
                "work_context_id": work_context_id,
                "expected_revision": expected_revision,
            }),
        )?;
        let mut tx = self.store.begin().await?;
        lock_operation(&mut tx, subject, operation_id).await?;
        if let Some(existing) = check_receipt(&mut tx, subject, operation_id, kind, &digest).await?
        {
            tx.commit().await.map_err(db)?;
            return self.work_context(subject, replay_context(existing)?).await;
        }
        let row = sqlx::query(
            "SELECT state,revision FROM work_contexts WHERE subject_id=$1 AND work_context_id=$2 FOR UPDATE",
        )
        .bind(subject.0)
        .bind(work_context_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(db)?
        .ok_or_else(|| Error::NotFound("WorkContext not found".into()))?;
        let state: String = row.try_get("state").map_err(db)?;
        let revision: i64 = row.try_get("revision").map_err(db)?;
        if revision != expected_revision {
            return Err(nous_core::DomainError::new(
                nous_core::DomainErrorCode::StaleRevision,
                "WorkContext revision is stale",
            )
            .with_context("work_context_id", work_context_id)
            .with_context("expected_revision", expected_revision)
            .with_context("actual_revision", revision)
            .into());
        }
        let next = match transition {
            WorkContextTransition::Pause if state == "open" => "paused",
            WorkContextTransition::Resume if state == "paused" => "open",
            WorkContextTransition::End if matches!(state.as_str(), "open" | "paused") => "ended",
            WorkContextTransition::Pause if state == "paused" => {
                return Err(Error::FailedPrecondition(
                    "WorkContext is already paused".into(),
                ));
            }
            WorkContextTransition::Resume if state == "ended" => {
                return Err(Error::FailedPrecondition(
                    "ended WorkContext cannot resume".into(),
                ));
            }
            WorkContextTransition::End if state == "ended" => {
                return Err(Error::FailedPrecondition(
                    "WorkContext is already ended".into(),
                ));
            }
            _ => {
                return Err(Error::FailedPrecondition(
                    "invalid WorkContext transition".into(),
                ));
            }
        };
        if matches!(
            transition,
            WorkContextTransition::Pause | WorkContextTransition::End
        ) {
            let sessions = sqlx::query_scalar::<_, Uuid>(
                "SELECT session_id FROM cognitive_sessions WHERE subject_id=$1 AND active_work_context_id=$2 ORDER BY session_id FOR UPDATE",
            )
            .bind(subject.0)
            .bind(work_context_id)
            .fetch_all(&mut *tx)
            .await
            .map_err(db)?;
            for session in sessions {
                sqlx::query(
                    "UPDATE cognitive_sessions SET active_work_context_id=NULL,runtime_revision=runtime_revision+1,last_activity_at=$3 WHERE subject_id=$1 AND session_id=$2",
                )
                .bind(subject.0)
                .bind(session)
                .bind(self.now(subject))
                .execute(&mut *tx)
                .await
                .map_err(db)?;
            }
        }
        let ended_at = (next == "ended").then(|| self.now(subject));
        sqlx::query(
            "UPDATE work_contexts SET state=$3,revision=revision+1,updated_at=$4,ended_at=$5 WHERE subject_id=$1 AND work_context_id=$2",
        )
        .bind(subject.0)
        .bind(work_context_id)
        .bind(next)
        .bind(self.now(subject))
        .bind(ended_at)
        .execute(&mut *tx)
        .await
        .map_err(db)?;
        if next == "ended" {
            self.wake_ended_context_in(&mut tx, subject, work_context_id)
                .await?;
        }
        commit_receipt(
            &mut tx,
            subject,
            operation_id,
            "work_context",
            Some(&work_context_id.to_string()),
            None,
            None,
        )
        .await?;
        tx.commit().await.map_err(db)?;
        self.work_context(subject, work_context_id).await
    }

    pub async fn set_active_work_context(
        &self,
        subject: SubjectId,
        session: SessionId,
        work_context_id: Option<Uuid>,
        expected_runtime_revision: i64,
        operation_id: OperationId,
    ) -> Result<SessionView> {
        let digest = canonical_request_digest(
            "set_active_work_context",
            subject,
            &serde_json::json!({
                "session_id": session,
                "work_context_id": work_context_id,
                "expected_runtime_revision": expected_runtime_revision,
            }),
        )?;
        let mut tx = self.store.begin().await?;
        lock_operation(&mut tx, subject, operation_id).await?;
        if let Some(existing) = check_receipt(
            &mut tx,
            subject,
            operation_id,
            "set_active_work_context",
            &digest,
        )
        .await?
        {
            tx.commit().await.map_err(db)?;
            if existing.state != "committed" {
                return Err(Error::Unavailable(
                    "foreground WorkContext operation is already in progress".into(),
                ));
            }
            return self.session(subject, session).await;
        }
        let row = sqlx::query(
            "SELECT subject_id,closed_at,runtime_revision FROM cognitive_sessions WHERE session_id=$1 FOR UPDATE",
        )
        .bind(session.0)
        .fetch_optional(&mut *tx)
        .await
        .map_err(db)?
        .ok_or_else(|| crate::sessions::stale_session(session))?;
        let owner: Uuid = row.try_get("subject_id").map_err(db)?;
        let closed: Option<DateTime<Utc>> = row.try_get("closed_at").map_err(db)?;
        let current: i64 = row.try_get("runtime_revision").map_err(db)?;
        if owner != subject.0 || closed.is_some() {
            return Err(crate::sessions::stale_session(session));
        }
        if current != expected_runtime_revision {
            return Err(nous_core::DomainError::new(
                nous_core::DomainErrorCode::StaleRevision,
                "Expected runtime revision is stale",
            )
            .with_context("session_id", session.0)
            .with_context("expected_revision", expected_runtime_revision)
            .with_context("actual_revision", current)
            .into());
        }
        if let Some(work_context_id) = work_context_id {
            let context = sqlx::query(
                "SELECT subject_id,state FROM work_contexts WHERE subject_id=$1 AND work_context_id=$2 FOR SHARE",
            )
            .bind(subject.0)
            .bind(work_context_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(db)?
            .ok_or_else(|| Error::NotFound("WorkContext not found".into()))?;
            if context.try_get::<String, _>("state").map_err(db)? != "open" {
                return Err(Error::FailedPrecondition(
                    "only an open WorkContext can be foreground".into(),
                ));
            }
        }
        sqlx::query(
            "UPDATE cognitive_sessions SET active_work_context_id=$3,runtime_revision=runtime_revision+1,last_activity_at=$4 WHERE subject_id=$1 AND session_id=$2",
        )
        .bind(subject.0)
        .bind(session.0)
        .bind(work_context_id)
        .bind(self.now(subject))
        .execute(&mut *tx)
        .await
        .map_err(db)?;
        commit_receipt(
            &mut tx,
            subject,
            operation_id,
            "work_context",
            None,
            None,
            None,
        )
        .await?;
        tx.commit().await.map_err(db)?;
        self.session(subject, session).await
    }
}

impl CognitiveRuntimeService {
    async fn wake_ended_context_in(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        subject: SubjectId,
        context: Uuid,
    ) -> Result<()> {
        let sequence:Option<i64>=sqlx::query_scalar("SELECT authority_seq FROM subjects WHERE subject_id=$1 AND EXISTS(SELECT 1 FROM experience_items WHERE subject_id=$1 AND active_work_context_id=$2)")
            .bind(subject.0).bind(context).fetch_optional(&mut **tx).await.map_err(db)?;
        if let Some(sequence) = sequence {
            for kind in ["episode_segment", "journal_review"] {
                self.enqueue_maintenance_in(
                    tx,
                    &MaintenanceRequest {
                        subject,
                        kind: kind.into(),
                        scope_kind: "track".into(),
                        scope_ref: "interaction".into(),
                        trigger_authority_seq: sequence,
                        due_at: self.now(subject),
                        priority: 60,
                    },
                )
                .await?;
            }
        }
        Ok(())
    }
}
