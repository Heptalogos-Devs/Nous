use super::*;

impl CognitiveRuntimeService {
    pub async fn work_context(
        &self,
        subject: SubjectId,
        work_context_id: Uuid,
    ) -> Result<WorkContextView> {
        let mut tx = self.store.begin().await?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
            .execute(&mut *tx)
            .await
            .map_err(db)?;
        let context = self
            .work_context_in(subject, work_context_id, &mut tx)
            .await?;
        tx.commit().await.map_err(db)?;
        Ok(context)
    }

    pub(crate) async fn work_context_in(
        &self,
        subject: SubjectId,
        work_context_id: Uuid,
        connection: &mut sqlx::PgConnection,
    ) -> Result<WorkContextView> {
        let row =
            sqlx::query("SELECT * FROM work_contexts WHERE subject_id=$1 AND work_context_id=$2")
                .bind(subject.0)
                .bind(work_context_id)
                .fetch_optional(&mut *connection)
                .await
                .map_err(db)?
                .ok_or_else(|| Error::NotFound("WorkContext not found".into()))?;
        self.work_context_from_row(row, connection).await
    }

    pub async fn list_work_contexts(&self, subject: SubjectId) -> Result<Vec<WorkContextView>> {
        self.store.require_subject(subject).await?;
        let ids = sqlx::query_scalar::<_, Uuid>(
            "SELECT work_context_id FROM work_contexts WHERE subject_id=$1 ORDER BY created_at,work_context_id",
        )
        .bind(subject.0)
        .fetch_all(self.store.pool())
        .await
        .map_err(db)?;
        let mut result = Vec::with_capacity(ids.len());
        for id in ids {
            result.push(self.work_context(subject, id).await?);
        }
        Ok(result)
    }
    pub async fn active_work_context_refs(
        &self,
        subject: SubjectId,
        session: SessionId,
    ) -> Result<Vec<CognitiveRef>> {
        let rows = sqlx::query(
            "SELECT r.ref_kind,r.ref_value FROM cognitive_sessions s JOIN work_context_refs r ON r.work_context_id=s.active_work_context_id JOIN work_contexts w ON w.work_context_id=r.work_context_id WHERE s.subject_id=$1 AND s.session_id=$2 AND s.closed_at IS NULL AND w.state='open' ORDER BY r.ordinal",
        )
        .bind(subject.0)
        .bind(session.0)
        .fetch_all(self.store.pool())
        .await
        .map_err(db)?;
        rows.into_iter()
            .map(|row| {
                parse_reference(
                    &row.try_get::<String, _>("ref_kind").map_err(db)?,
                    &row.try_get::<String, _>("ref_value").map_err(db)?,
                )
            })
            .collect()
    }

    async fn work_context_from_row(
        &self,
        row: sqlx::postgres::PgRow,
        executor: &mut sqlx::PgConnection,
    ) -> Result<WorkContextView> {
        let id: Uuid = row.try_get("work_context_id").map_err(db)?;
        let refs = sqlx::query(
            "SELECT ref_kind,ref_value FROM work_context_refs WHERE work_context_id=$1 ORDER BY ordinal",
        )
        .bind(id)
        .fetch_all(&mut *executor)
        .await
        .map_err(db)?;
        let cognition_anchors = refs
            .into_iter()
            .map(|value| {
                parse_reference(
                    &value.try_get::<String, _>("ref_kind").map_err(db)?,
                    &value.try_get::<String, _>("ref_value").map_err(db)?,
                )
            })
            .collect::<Result<Vec<_>>>()?;
        let anchors = sqlx::query("SELECT ref_kind,ref_value FROM work_context_anchors WHERE work_context_id=$1 ORDER BY ordinal").bind(id).fetch_all(&mut *executor).await.map_err(db)?;
        let mut entity_anchors = Vec::new();
        let mut tag_anchors = Vec::new();
        for anchor in anchors {
            match parse_reference(
                &anchor.try_get::<String, _>("ref_kind").map_err(db)?,
                &anchor.try_get::<String, _>("ref_value").map_err(db)?,
            )? {
                CognitiveRef::Entity(entity) => entity_anchors.push(entity),
                CognitiveRef::Tag(tag) => tag_anchors.push(tag),
                _ => {
                    return Err(Error::Infrastructure(
                        "invalid typed WorkContext anchor".into(),
                    ));
                }
            }
        }
        Ok(WorkContextView {
            work_context_id: id,
            subject_id: SubjectId(row.try_get("subject_id").map_err(db)?),
            state: parse_state(row.try_get("state").map_err(db)?)?,
            purpose: row.try_get("purpose").map_err(db)?,
            unresolved_questions: row.try_get("unresolved_questions").map_err(db)?,
            constraints: row.try_get("constraints").map_err(db)?,
            resume_conditions: row.try_get("resume_conditions").map_err(db)?,
            budget_summary: row.try_get("budget_summary").map_err(db)?,
            revision: row.try_get("revision").map_err(db)?,
            created_at: row.try_get("created_at").map_err(db)?,
            updated_at: row.try_get("updated_at").map_err(db)?,
            ended_at: row.try_get("ended_at").map_err(db)?,
            cognition_anchors,
            context_text: row.try_get("context_text").map_err(db)?,
            entity_anchors,
            tag_anchors,
        })
    }

    pub async fn runtime_references(
        &self,
        subject: SubjectId,
        session: SessionId,
    ) -> Result<Vec<(CognitiveRef, &'static str)>> {
        let session_view = self.session(subject, session).await?;
        let mut refs = session_view
            .resident
            .into_iter()
            .map(|value| (value.reference, "runtime_resident"))
            .collect::<Vec<_>>();
        refs.extend(
            self.active_work_context_refs(subject, session)
                .await?
                .into_iter()
                .map(|value| (value, "runtime_work_context")),
        );
        let mut seen = BTreeSet::new();
        refs.retain(|(reference, _)| seen.insert(reference.to_string()));
        Ok(refs)
    }
}
