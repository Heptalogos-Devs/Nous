//! Coherent raw Tag revisions and direct attachment rows for concept Serving.
// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
use crate::{AuthorityStore, database_error as db};
use nous_core::*;
use sqlx::Row;
#[derive(Debug, Clone)]
pub struct ConceptProjectionTag {
    pub tag: TagId,
    pub revision: uuid::Uuid,
    pub semantic: TagSemanticRepresentation,
    pub attachments: Vec<CognitiveRef>,
}
pub struct ConceptProjectionInput {
    pub watermark: i64,
    pub tags: Vec<ConceptProjectionTag>,
}
impl AuthorityStore {
    pub async fn concept_projection_input(
        &self,
        subject: SubjectId,
        memory_enabled: bool,
        view: Option<&HistoricalAuthoritySnapshot>,
    ) -> Result<ConceptProjectionInput> {
        let mut tx = self.begin().await?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
            .execute(&mut *tx)
            .await
            .map_err(db)?;
        if view.is_some_and(|view| view.subject != subject) {
            return Err(Error::Invalid(
                "concept projection view Subject mismatch".into(),
            ));
        }
        let watermark = if view.is_some() {
            0
        } else {
            crate::projection::watermark(&mut tx, subject, "concept", "*").await?
        };
        let mut tags = Vec::new();
        if !memory_enabled {
            tx.commit().await.map_err(db)?;
            return Ok(ConceptProjectionInput { watermark, tags });
        }
        let rows = sqlx::query("SELECT t.tag_id,r.tag_revision_id,r.label,r.description,r.kind_hint FROM tags t JOIN tag_revisions r ON r.tag_revision_id=t.current_revision_id WHERE t.subject_id=$1 AND (($2::uuid[] IS NULL AND t.status='active') OR t.tag_id=ANY($2)) ORDER BY t.tag_id")
                .bind(subject.0).bind(view.map(|view| view.tags.iter().filter(|tag| tag.status == "active").map(|tag|tag.tag.0).collect::<Vec<_>>())).fetch_all(&mut *tx).await.map_err(db)?;
        for row in rows {
            let tag = TagId(row.try_get("tag_id").map_err(db)?);
            let (revision, semantic) = if let Some(view) = view {
                let state = view.tag_state(tag).ok_or_else(|| {
                    Error::Infrastructure("selected historical Tag missing".into())
                })?;
                (state.revision_id, state.semantic.clone())
            } else {
                (
                    row.try_get("tag_revision_id").map_err(db)?,
                    tag_semantic_representation(
                        &row.try_get::<String, _>("label").map_err(db)?,
                        row.try_get::<Option<String>, _>("description")
                            .map_err(db)?
                            .as_deref(),
                        row.try_get::<Option<String>, _>("kind_hint")
                            .map_err(db)?
                            .as_deref(),
                    )?,
                )
            };
            tags.push(ConceptProjectionTag {
                tag,
                revision,
                semantic,
                attachments: vec![],
            });
        }
        let selected = super::longitudinal::selected_cognition_in(&mut tx, subject, view).await?;
        let (kinds, values): (Vec<_>, Vec<_>) = selected.iter().map(reference_parts).unzip();
        let rows = sqlx::query(r#"
WITH selected AS (SELECT * FROM unnest($2::text[],$3::text[]) eligible(kind,value)), attachments AS (
 SELECT t.tag_id,e.kind,e.value FROM memory_revision_tags t JOIN selected e ON e.kind='memory_revision' AND e.value=t.memory_revision_id::text
 UNION ALL SELECT tag_id,e.kind,e.value FROM cognitive_schema_revisions r JOIN selected e ON e.kind='cognitive_schema_revision' AND e.value=r.schema_revision_id::text CROSS JOIN LATERAL unnest(r.tags) tag_id
 UNION ALL SELECT a.to_ref::uuid,e.kind,e.value FROM association_evidence a JOIN selected e ON e.kind=a.from_ref_kind AND e.value=a.from_ref WHERE a.subject_id=$1 AND a.relation_kind='tag_attachment' AND a.to_ref_kind='tag' AND a.polarity='positive' AND (($4::uuid[] IS NULL AND a.revoked_at IS NULL) OR a.association_evidence_id=ANY($4))
) SELECT DISTINCT CASE WHEN $4::uuid[] IS NULL THEN canonical_tag($1,tag_id) ELSE tag_id END tag_id,kind,value FROM attachments ORDER BY tag_id,kind,value
"#).bind(subject.0).bind(kinds).bind(values).bind(view.map(|view| view.associations.iter().map(|id|id.0).collect::<Vec<_>>())).fetch_all(&mut *tx).await.map_err(db)?;
        for row in rows {
            let Some(raw) = row.try_get::<Option<uuid::Uuid>, _>("tag_id").map_err(db)? else {
                continue;
            };
            let tag = if let Some(view) = view {
                let Some(tag) = view.canonical_tag(TagId(raw)) else {
                    continue;
                };
                tag
            } else {
                TagId(raw)
            };
            if let Ok(index) = tags.binary_search_by_key(&tag.0, |tag| tag.tag.0) {
                tags[index].attachments.push(parse_reference(
                    &row.try_get::<String, _>("kind").map_err(db)?,
                    &row.try_get::<String, _>("value").map_err(db)?,
                )?);
            }
        }
        for tag in &mut tags {
            tag.attachments.sort_by_key(ToString::to_string);
            tag.attachments.dedup();
        }
        tx.commit().await.map_err(db)?;
        Ok(ConceptProjectionInput { watermark, tags })
    }
}
