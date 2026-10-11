//! Bounded reference discovery and document rows. Bodies are fetched only for selected refs.
// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
use crate::{AuthorityStore, EpisodeTextBudget, TextProjectionSource, database_error as db};
use nous_core::*;
use sqlx::Row;

impl AuthorityStore {
    pub async fn text_projection_lookup(
        &self,
        subject: SubjectId,
        references: &[CognitiveRef],
        memory_enabled: bool,
        budget: EpisodeTextBudget,
        view: Option<&HistoricalAuthoritySnapshot>,
    ) -> Result<Vec<TextProjectionSource>> {
        if references.len() > 256 || view.is_some_and(|view| view.subject != subject) {
            return Err(Error::Invalid("invalid bounded document lookup".into()));
        }
        let mut tx = self.begin().await?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
            .execute(&mut *tx)
            .await
            .map_err(db)?;
        let mut sources = if let Some(view) = view {
            crate::historical::text::historical_sources_in(
                &mut tx,
                view,
                budget,
                memory_enabled,
                Some(references),
            )
            .await?
            .0
        } else {
            let mut sources =
                super::text::material_sources(&mut tx, subject, Some(references)).await?;
            if memory_enabled {
                sources
                    .extend(super::text::memory_sources(&mut tx, subject, Some(references)).await?);
                sources.extend(
                    super::text::longitudinal_sources(&mut tx, subject, budget, Some(references))
                        .await?,
                );
            }
            sources
        };
        sources.sort_by_key(|source| source.reference.to_string());
        tx.commit().await.map_err(db)?;
        Ok(sources)
    }

    /// Keyset page of identities; this query never reads representation or Artifact bodies.
    pub async fn text_projection_reference_page(
        &self,
        subject: SubjectId,
        memory_enabled: bool,
        after: Option<&CognitiveRef>,
        limit: usize,
        view: Option<&HistoricalAuthoritySnapshot>,
    ) -> Result<Vec<CognitiveRef>> {
        if limit == 0 || limit > 257 {
            return Err(Error::Invalid(
                "document reference page must be 1..257".into(),
            ));
        }
        if let Some(view) = view {
            if view.subject != subject {
                return Err(Error::Invalid("document page Subject mismatch".into()));
            }
            let mut references = crate::historical::text::document_references(view, memory_enabled);
            references.sort_by_key(ToString::to_string);
            references.dedup();
            return Ok(references
                .into_iter()
                .filter(|reference| {
                    after.is_none_or(|after| reference.to_string() > after.to_string())
                })
                .take(limit)
                .collect());
        }
        let rows = sqlx::query(r#"
WITH catalog AS (
 SELECT 'occurrence' kind,occurrence_id id FROM observation_occurrences WHERE subject_id=$1
 UNION ALL SELECT 'source_region',source_region_id FROM source_regions WHERE subject_id=$1
 UNION ALL SELECT 'derived_representation',derived_representation_id FROM derived_representations WHERE subject_id=$1
 UNION ALL SELECT 'derived_region',derived_region_id FROM derived_regions WHERE subject_id=$1
 UNION ALL SELECT 'memory_revision',current_revision_id FROM memory_objects WHERE $2 AND subject_id=$1 AND acceptance_state='accepted' AND integrity_state='valid' AND suppression_state='normal' AND purge_state='normal'
 UNION ALL SELECT 'cognitive_schema_revision',current_revision_id FROM cognitive_schemas WHERE $2 AND subject_id=$1 AND acceptance_state='accepted' AND integrity_state='valid' AND suppression_state='normal' AND purge_state='normal'
 UNION ALL SELECT 'episode_revision',current_revision_id FROM episode_objects WHERE $2 AND subject_id=$1 AND acceptance_state='accepted' AND integrity_state='valid' AND suppression_state='normal' AND purge_state='normal'
 UNION ALL SELECT 'journal_revision',current_revision_id FROM journal_objects WHERE $2 AND subject_id=$1 AND acceptance_state='accepted' AND integrity_state='valid' AND suppression_state='normal' AND purge_state='normal'
 UNION ALL SELECT 'tag',tag_id FROM tags WHERE $2 AND subject_id=$1 AND status='active'
) SELECT kind,id FROM catalog WHERE id IS NOT NULL AND ($3::text IS NULL OR kind||':'||id::text > $3 COLLATE "C") ORDER BY kind||':'||id::text COLLATE "C" LIMIT $4
"#).bind(subject.0).bind(memory_enabled).bind(after.map(ToString::to_string))
            .bind(limit as i64).fetch_all(self.pool()).await.map_err(db)?;
        rows.into_iter()
            .map(|row| {
                parse_reference(
                    &row.get::<String, _>("kind"),
                    &row.get::<uuid::Uuid, _>("id").to_string(),
                )
            })
            .collect()
    }
}
