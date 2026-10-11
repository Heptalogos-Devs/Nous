use super::*;

impl CognitiveRuntimeService {
    pub async fn create_work_context(
        &self,
        input: CreateWorkContextInput,
    ) -> Result<WorkContextView> {
        validate_payload(
            &input.purpose,
            &input.unresolved_questions,
            &input.constraints,
            &input.resume_conditions,
            &input.budget_summary,
            &input.cognition_anchors,
        )?;
        validate_anchors(
            &input.context_text,
            &input.entity_anchors,
            &input.tag_anchors,
        )?;
        self.store.require_subject(input.subject).await?;
        let digest = canonical_request_digest(
            "create_work_context",
            input.subject,
            &serde_json::json!({
                "purpose": input.purpose,
                "unresolved_questions": input.unresolved_questions,
                "constraints": input.constraints,
                "resume_conditions": input.resume_conditions,
                "budget_summary": input.budget_summary,
                "cognition_anchors": input.cognition_anchors,
                "context_text": input.context_text,
                "entity_anchors": input.entity_anchors,
                "tag_anchors": input.tag_anchors,
            }),
        )?;
        let mut tx = self.store.begin().await?;
        lock_operation(&mut tx, input.subject, input.operation_id).await?;
        if let Some(existing) = check_receipt(
            &mut tx,
            input.subject,
            input.operation_id,
            "create_work_context",
            &digest,
        )
        .await?
        {
            tx.commit().await.map_err(db)?;
            return self
                .work_context(input.subject, replay_context(existing)?)
                .await;
        }
        let work_context_id = Uuid::now_v7();
        validate_refs_in_tx(
            &self.store,
            &mut tx,
            input.subject,
            &input.cognition_anchors,
        )
        .await?;
        let typed = typed_anchors(&input.entity_anchors, &input.tag_anchors);
        validate_refs_in_tx(&self.store, &mut tx, input.subject, &typed).await?;
        let now = self.now(input.subject);
        sqlx::query(
            "INSERT INTO work_contexts(work_context_id,subject_id,state,purpose,unresolved_questions,constraints,resume_conditions,budget_summary,context_text,revision,created_at,updated_at) VALUES($1,$2,'open',$3,$4,$5,$6,$7,$9,1,$8,$8)",
        )
        .bind(work_context_id)
        .bind(input.subject.0)
        .bind(&input.purpose)
        .bind(&input.unresolved_questions)
        .bind(&input.constraints)
        .bind(&input.resume_conditions)
        .bind(&input.budget_summary)
        .bind(now)
        .bind(&input.context_text)
        .execute(&mut *tx)
        .await
        .map_err(db)?;
        write_anchors(&mut tx, work_context_id, &input.cognition_anchors, &typed).await?;
        self.store
            .ensure_identity_addresses_in(
                &mut tx,
                input.subject,
                &[CognitiveRef::WorkContext(nous_core::WorkContextId(
                    work_context_id,
                ))],
                &input.purpose,
            )
            .await?;
        commit_receipt(
            &mut tx,
            input.subject,
            input.operation_id,
            "work_context",
            Some(&work_context_id.to_string()),
            None,
            None,
        )
        .await?;
        tx.commit().await.map_err(db)?;
        self.work_context(input.subject, work_context_id).await
    }
    pub async fn update_work_context(
        &self,
        input: UpdateWorkContextInput,
    ) -> Result<WorkContextView> {
        validate_payload(
            &input.purpose,
            &input.unresolved_questions,
            &input.constraints,
            &input.resume_conditions,
            &input.budget_summary,
            &input.cognition_anchors,
        )?;
        validate_anchors(
            &input.context_text,
            &input.entity_anchors,
            &input.tag_anchors,
        )?;
        self.store.require_subject(input.subject).await?;
        let digest = canonical_request_digest(
            "update_work_context",
            input.subject,
            &serde_json::json!({
                "work_context_id": input.work_context_id,
                "expected_revision": input.expected_revision,
                "purpose": input.purpose,
                "unresolved_questions": input.unresolved_questions,
                "constraints": input.constraints,
                "resume_conditions": input.resume_conditions,
                "budget_summary": input.budget_summary,
                "cognition_anchors": input.cognition_anchors,
                "context_text": input.context_text,
                "entity_anchors": input.entity_anchors,
                "tag_anchors": input.tag_anchors,
            }),
        )?;
        let mut tx = self.store.begin().await?;
        lock_operation(&mut tx, input.subject, input.operation_id).await?;
        if let Some(existing) = check_receipt(
            &mut tx,
            input.subject,
            input.operation_id,
            "update_work_context",
            &digest,
        )
        .await?
        {
            tx.commit().await.map_err(db)?;
            return self
                .work_context(input.subject, replay_context(existing)?)
                .await;
        }
        let row = sqlx::query(
            "SELECT state,revision FROM work_contexts WHERE subject_id=$1 AND work_context_id=$2 FOR UPDATE",
        )
        .bind(input.subject.0)
        .bind(input.work_context_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(db)?
        .ok_or_else(|| Error::NotFound("WorkContext not found".into()))?;
        let state: String = row.try_get("state").map_err(db)?;
        let revision: i64 = row.try_get("revision").map_err(db)?;
        if state == WorkContextState::Ended.as_str() {
            return Err(Error::FailedPrecondition("WorkContext is ended".into()));
        }
        if revision != input.expected_revision {
            return Err(nous_core::DomainError::new(
                nous_core::DomainErrorCode::StaleRevision,
                "WorkContext revision is stale",
            )
            .with_context("work_context_id", input.work_context_id)
            .with_context("expected_revision", input.expected_revision)
            .with_context("actual_revision", revision)
            .into());
        }
        validate_refs_in_tx(
            &self.store,
            &mut tx,
            input.subject,
            &input.cognition_anchors,
        )
        .await?;
        let typed = typed_anchors(&input.entity_anchors, &input.tag_anchors);
        validate_refs_in_tx(&self.store, &mut tx, input.subject, &typed).await?;
        let now = self.now(input.subject);
        sqlx::query(
            "UPDATE work_contexts SET purpose=$3,unresolved_questions=$4,constraints=$5,resume_conditions=$6,budget_summary=$7,context_text=$9,revision=revision+1,updated_at=$8 WHERE subject_id=$1 AND work_context_id=$2",
        )
        .bind(input.subject.0)
        .bind(input.work_context_id)
        .bind(&input.purpose)
        .bind(&input.unresolved_questions)
        .bind(&input.constraints)
        .bind(&input.resume_conditions)
        .bind(&input.budget_summary)
        .bind(now)
        .bind(&input.context_text)
        .execute(&mut *tx)
        .await
        .map_err(db)?;
        write_anchors(
            &mut tx,
            input.work_context_id,
            &input.cognition_anchors,
            &typed,
        )
        .await?;
        commit_receipt(
            &mut tx,
            input.subject,
            input.operation_id,
            "work_context",
            Some(&input.work_context_id.to_string()),
            None,
            None,
        )
        .await?;
        tx.commit().await.map_err(db)?;
        self.work_context(input.subject, input.work_context_id)
            .await
    }
}

async fn validate_refs_in_tx(
    store: &nous_persistence::AuthorityStore,
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    subject: SubjectId,
    cognition_anchors: &[CognitiveRef],
) -> Result<()> {
    for reference in cognition_anchors {
        if !store
            .reference_in_subject_tx(tx, subject, reference)
            .await?
        {
            return Err(Error::FailedPrecondition(
                "WorkContext reference is not owned by Subject".into(),
            ));
        }
    }
    Ok(())
}

async fn write_anchors(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    id: Uuid,
    cognition: &[CognitiveRef],
    typed: &[CognitiveRef],
) -> Result<()> {
    for (delete, insert, references) in [
        (
            "DELETE FROM work_context_refs WHERE work_context_id=$1",
            "INSERT INTO work_context_refs(work_context_id,ordinal,ref_kind,ref_value) VALUES($1,$2,$3,$4)",
            cognition,
        ),
        (
            "DELETE FROM work_context_anchors WHERE work_context_id=$1",
            "INSERT INTO work_context_anchors(work_context_id,ordinal,ref_kind,ref_value) VALUES($1,$2,$3,$4)",
            typed,
        ),
    ] {
        sqlx::query(delete)
            .bind(id)
            .execute(&mut **tx)
            .await
            .map_err(db)?;
        for (ordinal, reference) in references.iter().enumerate() {
            let (kind, value) = reference_parts(reference);
            sqlx::query(insert)
                .bind(id)
                .bind(ordinal as i32)
                .bind(kind)
                .bind(value)
                .execute(&mut **tx)
                .await
                .map_err(db)?;
        }
    }
    Ok(())
}
