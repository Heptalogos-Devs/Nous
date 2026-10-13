use super::*;

impl CognitiveRuntimeService {
    pub async fn work_context(
        &self,
        subject: SubjectId,
        work_context_id: Uuid,
    ) -> Result<WorkContextView> {
        let mut tx = self.store.read_view().await?;
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

    pub async fn list_work_contexts(
        &self,
        subject: SubjectId,
        limit: i64,
        after: Option<Uuid>,
    ) -> Result<(Vec<WorkContextView>, bool)> {
        if !(1..=200).contains(&limit) {
            return Err(Error::Invalid(
                "WorkContext page size is out of bounds".into(),
            ));
        }
        self.store.require_subject(subject).await?;
        let mut tx = self.store.read_view().await?;
        let mut rows = sqlx::query("SELECT * FROM work_contexts WHERE subject_id=$1 AND ($2::uuid IS NULL OR work_context_id>$2) ORDER BY work_context_id LIMIT $3")
            .bind(subject.0).bind(after).bind(limit+1).fetch_all(&mut *tx).await.map_err(db)?;
        let more = rows.len() > limit as usize;
        rows.truncate(limit as usize);
        let ids = rows
            .iter()
            .map(|row| row.try_get::<Uuid, _>("work_context_id").map_err(db))
            .collect::<Result<Vec<_>>>()?;
        let mut refs = std::collections::HashMap::<Uuid, Vec<sqlx::postgres::PgRow>>::new();
        let mut anchors = std::collections::HashMap::<Uuid, Vec<sqlx::postgres::PgRow>>::new();
        for row in sqlx::query("SELECT * FROM work_context_refs WHERE work_context_id=ANY($1) ORDER BY work_context_id,ordinal")
            .bind(&ids).fetch_all(&mut *tx).await.map_err(db)? {
            refs.entry(row.try_get("work_context_id").map_err(db)?).or_default().push(row);
        }
        for row in sqlx::query("SELECT * FROM work_context_anchors WHERE work_context_id=ANY($1) ORDER BY work_context_id,ordinal")
            .bind(&ids).fetch_all(&mut *tx).await.map_err(db)? {
            anchors.entry(row.try_get("work_context_id").map_err(db)?).or_default().push(row);
        }
        let result = rows
            .into_iter()
            .map(|row| {
                let id: Uuid = row.try_get("work_context_id").map_err(db)?;
                decode_work_context(
                    row,
                    refs.remove(&id).unwrap_or_default(),
                    anchors.remove(&id).unwrap_or_default(),
                )
            })
            .collect::<Result<Vec<_>>>()?;
        tx.commit().await.map_err(db)?;
        Ok((result, more))
    }

    pub(super) async fn work_context_from_row(
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
        let anchors = sqlx::query("SELECT ref_kind,ref_value FROM work_context_anchors WHERE work_context_id=$1 ORDER BY ordinal").bind(id).fetch_all(&mut *executor).await.map_err(db)?;
        decode_work_context(row, refs, anchors)
    }
}

fn decode_work_context(
    row: sqlx::postgres::PgRow,
    refs: Vec<sqlx::postgres::PgRow>,
    anchors: Vec<sqlx::postgres::PgRow>,
) -> Result<WorkContextView> {
    let id: Uuid = row.try_get("work_context_id").map_err(db)?;
    let cognition_anchors = refs
        .into_iter()
        .map(|value| {
            parse_reference(
                &value.try_get::<String, _>("ref_kind").map_err(db)?,
                &value.try_get::<String, _>("ref_value").map_err(db)?,
            )
        })
        .collect::<Result<Vec<_>>>()?;
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
