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
        let mut mutation = match MutationEnvelope::begin(
            &self.store,
            input.subject,
            input.operation_id,
            "create_work_context",
            &digest,
            None,
        )
        .await?
        {
            MutationStart::Replay(existing) => {
                return self
                    .work_context(input.subject, replay_context(existing)?)
                    .await;
            }
            MutationStart::Active(mutation) => mutation,
        };
        let tx = mutation.tx();
        let work_context_id = Uuid::now_v7();
        validate_refs_in_tx(&self.store, tx, input.subject, &input.cognition_anchors).await?;
        let typed = typed_anchors(&input.entity_anchors, &input.tag_anchors);
        validate_refs_in_tx(&self.store, tx, input.subject, &typed).await?;
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
        .execute(&mut **tx)
        .await
        .map_err(db)?;
        write_anchors(tx, work_context_id, &input.cognition_anchors, &typed).await?;
        self.store
            .ensure_identity_addresses_in(
                tx,
                input.subject,
                &[CognitiveRef::WorkContext(nous_core::WorkContextId(
                    work_context_id,
                ))],
                &input.purpose,
            )
            .await?;
        mutation
            .commit(
                "work_context",
                Some(&work_context_id.to_string()),
                None,
                None,
            )
            .await?;
        self.work_context(input.subject, work_context_id).await
    }
    pub async fn update_work_context(
        &self,
        input: UpdateWorkContextInput,
    ) -> Result<WorkContextView> {
        self.store.require_subject(input.subject).await?;
        let digest = canonical_request_digest(
            "update_work_context",
            input.subject,
            &serde_json::json!({
                "work_context_id": input.work_context_id,
                "expected_revision": input.expected_revision,
                "intent": input.intent,
            }),
        )?;
        let mut mutation = match MutationEnvelope::begin(
            &self.store,
            input.subject,
            input.operation_id,
            "update_work_context",
            &digest,
            None,
        )
        .await?
        {
            MutationStart::Replay(existing) => {
                return self
                    .work_context(input.subject, replay_context(existing)?)
                    .await;
            }
            MutationStart::Active(mutation) => mutation,
        };
        let tx = mutation.tx();
        let row = sqlx::query(
            "SELECT * FROM work_contexts WHERE subject_id=$1 AND work_context_id=$2 FOR UPDATE",
        )
        .bind(input.subject.0)
        .bind(input.work_context_id)
        .fetch_optional(&mut **tx)
        .await
        .map_err(db)?
        .ok_or_else(|| Error::NotFound("WorkContext not found".into()))?;
        let mut next = self.work_context_from_row(row, tx).await?;
        let revision = next.revision;
        if next.state == WorkContextState::Ended {
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
        let anchors_changed = !matches!(
            &input.intent,
            WorkContextIntent::Fields(_) | WorkContextIntent::Clear(WorkContextClearScope::Text)
        );
        next.apply_intent(input.intent)?;
        if anchors_changed {
            validate_refs_in_tx(&self.store, tx, input.subject, &next.cognition_anchors).await?;
            let typed = typed_anchors(&next.entity_anchors, &next.tag_anchors);
            validate_refs_in_tx(&self.store, tx, input.subject, &typed).await?;
            write_anchors(tx, input.work_context_id, &next.cognition_anchors, &typed).await?;
        }
        let now = self.now(input.subject);
        sqlx::query(
            "UPDATE work_contexts SET purpose=$3,unresolved_questions=$4,constraints=$5,resume_conditions=$6,budget_summary=$7,context_text=$9,revision=revision+1,updated_at=$8 WHERE subject_id=$1 AND work_context_id=$2",
        )
        .bind(input.subject.0)
        .bind(input.work_context_id)
        .bind(&next.purpose)
        .bind(&next.unresolved_questions)
        .bind(&next.constraints)
        .bind(&next.resume_conditions)
        .bind(&next.budget_summary)
        .bind(now)
        .bind(&next.context_text)
        .execute(&mut **tx)
        .await
        .map_err(db)?;
        mutation
            .commit(
                "work_context",
                Some(&input.work_context_id.to_string()),
                None,
                None,
            )
            .await?;
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
            "INSERT INTO work_context_refs(work_context_id,ordinal,ref_kind,ref_value) SELECT $1,(ordinality-1)::integer,kind,value FROM unnest($2::text[],$3::text[]) WITH ORDINALITY AS refs(kind,value,ordinality)",
            cognition,
        ),
        (
            "DELETE FROM work_context_anchors WHERE work_context_id=$1",
            "INSERT INTO work_context_anchors(work_context_id,ordinal,ref_kind,ref_value) SELECT $1,(ordinality-1)::integer,kind,value FROM unnest($2::text[],$3::text[]) WITH ORDINALITY AS refs(kind,value,ordinality)",
            typed,
        ),
    ] {
        sqlx::query(delete)
            .bind(id)
            .execute(&mut **tx)
            .await
            .map_err(db)?;
        let (kinds, values): (Vec<_>, Vec<_>) = references.iter().map(reference_parts).unzip();
        sqlx::query(insert)
            .bind(id)
            .bind(kinds)
            .bind(values)
            .execute(&mut **tx)
            .await
            .map_err(db)?;
    }
    Ok(())
}

fn validate_anchor_intent(anchors: &WorkContextAnchors) -> Result<()> {
    if anchors.cognition_anchors.is_empty()
        && anchors.entity_anchors.is_empty()
        && anchors.tag_anchors.is_empty()
    {
        return Err(Error::Invalid("WorkContext anchors are empty".into()));
    }
    validate_payload(
        "anchors",
        &[],
        &serde_json::Value::Null,
        &[],
        &serde_json::Value::Null,
        &anchors.cognition_anchors,
    )?;
    validate_anchors("", &anchors.entity_anchors, &anchors.tag_anchors)
}

impl WorkContextView {
    fn apply_intent(&mut self, intent: WorkContextIntent) -> Result<()> {
        match intent {
            WorkContextIntent::Fields(fields) => {
                if fields.purpose.is_none()
                    && fields.unresolved_questions.is_none()
                    && fields.constraints.is_none()
                    && fields.resume_conditions.is_none()
                    && fields.budget_summary.is_none()
                    && fields.context_text.is_none()
                {
                    return Err(Error::Invalid("WorkContext fields are empty".into()));
                }
                if let Some(value) = fields.purpose {
                    self.purpose = value;
                }
                if let Some(value) = fields.unresolved_questions {
                    self.unresolved_questions = value;
                }
                if let Some(value) = fields.constraints {
                    self.constraints = value;
                }
                if let Some(value) = fields.resume_conditions {
                    self.resume_conditions = value;
                }
                if let Some(value) = fields.budget_summary {
                    self.budget_summary = value;
                }
                if let Some(value) = fields.context_text {
                    self.context_text = value;
                }
            }
            WorkContextIntent::Pin(anchors) => {
                validate_anchor_intent(&anchors)?;
                for value in anchors.cognition_anchors {
                    if !self.cognition_anchors.contains(&value) {
                        self.cognition_anchors.push(value);
                    }
                }
                for value in anchors.entity_anchors {
                    if !self.entity_anchors.contains(&value) {
                        self.entity_anchors.push(value);
                    }
                }
                for value in anchors.tag_anchors {
                    if !self.tag_anchors.contains(&value) {
                        self.tag_anchors.push(value);
                    }
                }
            }
            WorkContextIntent::Unpin(anchors) => {
                validate_anchor_intent(&anchors)?;
                self.cognition_anchors
                    .retain(|value| !anchors.cognition_anchors.contains(value));
                self.entity_anchors
                    .retain(|value| !anchors.entity_anchors.contains(value));
                self.tag_anchors
                    .retain(|value| !anchors.tag_anchors.contains(value));
            }
            WorkContextIntent::Clear(scope) => {
                if matches!(
                    scope,
                    WorkContextClearScope::Text | WorkContextClearScope::All
                ) {
                    self.context_text.clear();
                }
                if matches!(
                    scope,
                    WorkContextClearScope::Anchors | WorkContextClearScope::All
                ) {
                    self.cognition_anchors.clear();
                    self.entity_anchors.clear();
                    self.tag_anchors.clear();
                }
            }
        }
        validate_payload(
            &self.purpose,
            &self.unresolved_questions,
            &self.constraints,
            &self.resume_conditions,
            &self.budget_summary,
            &self.cognition_anchors,
        )?;
        validate_anchors(&self.context_text, &self.entity_anchors, &self.tag_anchors)
    }
}
