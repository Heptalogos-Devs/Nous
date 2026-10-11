// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;
use crate::service::cognition::CognitionSelection;
use std::collections::HashMap;

impl MemoryService {
    pub async fn journal(
        &self,
        subject: SubjectId,
        journal: JournalId,
        revision: Option<JournalRevisionId>,
    ) -> Result<JournalView> {
        let reference = revision.map_or(
            CognitiveRef::Journal(journal),
            CognitiveRef::JournalRevision,
        );
        let context = self.current_read_context(subject).await?;
        match self
            .read_cognitions(&context, std::slice::from_ref(&reference))
            .await?
            .remove(&reference)
        {
            Some(CognitionView::Journal(view)) if view.header.object_id() == journal.0 => Ok(view),
            _ => Err(Error::new(
                ErrorOwner::Memory,
                ErrorCode::NotFound,
                "Journal not found",
            )),
        }
    }

    pub async fn journal_revision(
        &self,
        subject: SubjectId,
        revision: JournalRevisionId,
    ) -> Result<JournalView> {
        let reference = CognitiveRef::JournalRevision(revision);
        let context = self.current_read_context(subject).await?;
        match self
            .read_cognitions(&context, std::slice::from_ref(&reference))
            .await?
            .remove(&reference)
        {
            Some(CognitionView::Journal(view)) => Ok(view),
            _ => Err(Error::new(
                ErrorOwner::Memory,
                ErrorCode::NotFound,
                "Journal revision not found",
            )),
        }
    }

    pub async fn journal_page(
        &self,
        subject: SubjectId,
        journal: Option<JournalId>,
        limit: i64,
        last: Option<Uuid>,
        status: &str,
    ) -> Result<Vec<JournalView>> {
        if !(1..=201).contains(&limit) {
            return Err(Error::new(
                ErrorOwner::Memory,
                ErrorCode::InvalidRequest,
                "Journal page limit is out of bounds",
            ));
        }
        if !["", "accepted", "withdrawn"].contains(&status) {
            return Err(Error::new(
                ErrorOwner::Memory,
                ErrorCode::InvalidRequest,
                "invalid Journal status filter",
            ));
        }
        self.store.require_subject(subject).await?;
        let references = if let Some(journal) = journal {
            sqlx::query_scalar::<_, Uuid>("SELECT revision_id FROM cognition_revision_headers WHERE subject_id=$1 AND object_kind='journal' AND object_id=$2 AND ($3::uuid IS NULL OR revision_id>$3) ORDER BY revision_id LIMIT $4")
                .bind(subject.0).bind(journal.0).bind(last).bind(limit).fetch_all(self.store.pool()).await.map_err(db)?
                .into_iter().map(|id| CognitiveRef::JournalRevision(JournalRevisionId(id))).collect::<Vec<_>>()
        } else {
            sqlx::query_scalar::<_, Uuid>("SELECT object_id FROM cognition_objects WHERE object_kind='journal' AND subject_id=$1 AND ($2::uuid IS NULL OR object_id>$2) AND ($4='' OR acceptance_state=$4) ORDER BY object_id LIMIT $3")
                .bind(subject.0).bind(last).bind(limit).bind(status).fetch_all(self.store.pool()).await.map_err(db)?
                .into_iter().map(|id| CognitiveRef::Journal(JournalId(id))).collect::<Vec<_>>()
        };
        let context = self.current_read_context(subject).await?;
        let views = self.read_cognitions(&context, &references).await?;
        references
            .iter()
            .map(|reference| match views.get(reference) {
                Some(CognitionView::Journal(view)) => Ok(view.clone()),
                _ => Err(Error::new(
                    ErrorOwner::Memory,
                    ErrorCode::NotFound,
                    "Journal not found",
                )),
            })
            .collect()
    }

    pub async fn list_journals(&self, subject: SubjectId) -> Result<Vec<JournalView>> {
        self.store.require_subject(subject).await?;
        let references = sqlx::query_scalar::<_, Uuid>("SELECT object_id FROM cognition_objects WHERE object_kind='journal' AND subject_id=$1 ORDER BY created_at,object_id LIMIT 256")
            .bind(subject.0).fetch_all(self.store.pool()).await.map_err(db)?
            .into_iter().map(|id| CognitiveRef::Journal(JournalId(id))).collect::<Vec<_>>();
        let context = self.current_read_context(subject).await?;
        let views = self.read_cognitions(&context, &references).await?;
        references
            .iter()
            .map(|reference| match views.get(reference) {
                Some(CognitionView::Journal(view)) => Ok(view.clone()),
                _ => Err(Error::new(
                    ErrorOwner::Memory,
                    ErrorCode::NotFound,
                    "Journal not found",
                )),
            })
            .collect()
    }

    pub async fn journal_history(
        &self,
        subject: SubjectId,
        journal: JournalId,
    ) -> Result<Vec<JournalView>> {
        let references = sqlx::query_scalar::<_, Uuid>("SELECT revision_id FROM cognition_revision_headers WHERE subject_id=$1 AND object_kind='journal' AND object_id=$2 ORDER BY revision_no LIMIT 256")
            .bind(subject.0).bind(journal.0).fetch_all(self.store.pool()).await.map_err(db)?
            .into_iter().map(|id| CognitiveRef::JournalRevision(JournalRevisionId(id))).collect::<Vec<_>>();
        if references.is_empty() {
            return Ok(Vec::new());
        }
        let context = self.current_read_context(subject).await?;
        let views = self.read_cognitions(&context, &references).await?;
        references
            .iter()
            .map(|reference| match views.get(reference) {
                Some(CognitionView::Journal(view)) => Ok(view.clone()),
                _ => Err(Error::new(
                    ErrorOwner::Memory,
                    ErrorCode::NotFound,
                    "Journal revision not found",
                )),
            })
            .collect()
    }

    pub(in crate::service) async fn journal_views_for_selections_in_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        selections: &[CognitionSelection],
    ) -> Result<HashMap<CognitiveRef, JournalView>> {
        let revision_ids = selections
            .iter()
            .map(|selection| selection.revision.revision_id())
            .collect::<Vec<_>>();
        if revision_ids.is_empty() {
            return Ok(HashMap::new());
        }
        let rows = sqlx::query("SELECT journal_revision_id,revision_intent,title,temporal_scope_kind,temporal_scope_start,temporal_scope_end,narrative FROM journal_revisions WHERE journal_revision_id=ANY($1)")
            .bind(&revision_ids).fetch_all(&mut **tx).await.map_err(db)?;
        let mut points = HashMap::<Uuid, Vec<JournalPoint>>::new();
        for row in sqlx::query("SELECT p.journal_revision_id,p.ordinal,p.role,p.text,COALESCE(jsonb_agg(s.basis ORDER BY s.basis_no) FILTER(WHERE s.basis_no IS NOT NULL),'[]') AS basis FROM journal_revision_points p LEFT JOIN journal_point_basis s USING(journal_revision_id,ordinal) WHERE p.journal_revision_id=ANY($1) GROUP BY p.journal_revision_id,p.ordinal,p.role,p.text ORDER BY p.journal_revision_id,p.ordinal")
            .bind(&revision_ids).fetch_all(&mut **tx).await.map_err(db)? {
            let revision: Uuid = row.try_get("journal_revision_id").map_err(db)?;
            let role = parse_enum(ErrorOwner::Memory, row.try_get("role").map_err(db)?, "Journal point role")?;
            let basis = serde_json::from_value(row.try_get("basis").map_err(db)?).map_err(|error| Error::new(ErrorOwner::Memory, ErrorCode::InternalFailure, "Could not decode a persisted journal point basis").with_source(error))?;
            points.entry(revision).or_default().push(JournalPoint { role, text: row.try_get("text").map_err(db)?, basis });
        }
        let mut sources = HashMap::<Uuid, Vec<CognitiveRef>>::new();
        for row in sqlx::query("SELECT journal_revision_id,ref_kind,ref_value FROM journal_revision_sources WHERE journal_revision_id=ANY($1) ORDER BY journal_revision_id,ref_kind,ref_value")
            .bind(&revision_ids).fetch_all(&mut **tx).await.map_err(db)? {
            let revision: Uuid = row.try_get("journal_revision_id").map_err(db)?;
            sources.entry(revision).or_default().push(parse_reference(&row.try_get::<String, _>("ref_kind").map_err(db)?, &row.try_get::<String, _>("ref_value").map_err(db)?)?);
        }
        let selected = selections
            .iter()
            .map(|selection| (selection.revision.revision_id(), selection))
            .collect::<HashMap<_, _>>();
        let mut views = HashMap::with_capacity(rows.len());
        for row in rows {
            let revision_id: Uuid = row.try_get("journal_revision_id").map_err(db)?;
            let Some(selection) = selected.get(&revision_id) else {
                continue;
            };
            let revision = JournalRevision {
                header: selection.revision.clone(),
                revision_intent: row.try_get("revision_intent").map_err(db)?,
                title: row.try_get("title").map_err(db)?,
                temporal_scope: temporal_from_columns(
                    ErrorOwner::Memory,
                    row.try_get("temporal_scope_kind").map_err(db)?,
                    row.try_get("temporal_scope_start").map_err(db)?,
                    row.try_get("temporal_scope_end").map_err(db)?,
                )?,
                narrative: row.try_get("narrative").map_err(db)?,
            };
            let reference = selection.revision.revision_ref.clone();
            views.insert(
                reference,
                JournalView {
                    header: selection.header.clone(),
                    revision,
                    points: points.remove(&revision_id).unwrap_or_default(),
                    sources: sources.remove(&revision_id).unwrap_or_default(),
                },
            );
        }
        Ok(views)
    }
}
