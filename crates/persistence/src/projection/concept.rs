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
    ) -> Result<ConceptProjectionInput> {
        let mut tx = self.begin().await?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
            .execute(&mut *tx)
            .await
            .map_err(db)?;
        let watermark =
            crate::projection::text::watermark(&mut tx, subject, "concept", "*").await?;
        let rows = sqlx::query("SELECT t.tag_id,r.tag_revision_id,r.label,r.description,r.kind_hint FROM tags t JOIN tag_revisions r ON r.tag_revision_id=t.current_revision_id WHERE t.subject_id=$1 AND t.status='active' ORDER BY t.tag_id")
            .bind(subject.0).fetch_all(&mut *tx).await.map_err(db)?;
        let mut tags = Vec::new();
        for row in rows {
            tags.push(ConceptProjectionTag {
                tag: TagId(row.try_get("tag_id").map_err(db)?),
                revision: row.try_get("tag_revision_id").map_err(db)?,
                semantic: tag_semantic_representation(
                    &row.try_get::<String, _>("label").map_err(db)?,
                    row.try_get::<Option<String>, _>("description")
                        .map_err(db)?
                        .as_deref(),
                    row.try_get::<Option<String>, _>("kind_hint")
                        .map_err(db)?
                        .as_deref(),
                )?,
                attachments: Vec::new(),
            });
        }
        let rows = sqlx::query(r#"
WITH eligible AS (
 SELECT CASE o.object_kind
          WHEN 'memory' THEN 'memory_revision'
          WHEN 'cognitive_schema' THEN 'cognitive_schema_revision'
          WHEN 'episode' THEN 'episode_revision'
          WHEN 'journal' THEN 'journal_revision'
        END::text kind,
        o.current_revision_id::text value
 FROM cognition_objects o
 WHERE o.subject_id=$1 AND o.acceptance_state='accepted' AND o.integrity_state='valid'
   AND o.suppression_state='normal' AND o.purge_state='normal'
), attachments AS (
 SELECT canonical_tag($1,t.tag_id) tag_id,e.kind,e.value FROM memory_revision_tags t JOIN eligible e ON e.kind='memory_revision' AND e.value=t.memory_revision_id::text
 UNION ALL SELECT canonical_tag($1,tag_id),e.kind,e.value FROM cognitive_schema_revisions r JOIN eligible e ON e.kind='cognitive_schema_revision' AND e.value=r.schema_revision_id::text CROSS JOIN LATERAL unnest(r.tags) tag_id
 UNION ALL SELECT canonical_tag($1,a.to_ref::uuid),e.kind,e.value FROM association_evidence a JOIN eligible e ON e.kind=a.from_ref_kind AND e.value=a.from_ref WHERE a.subject_id=$1 AND a.relation_kind='tag_attachment' AND a.to_ref_kind='tag' AND a.polarity='positive' AND a.revoked_at IS NULL
)
SELECT DISTINCT tag_id,kind,value FROM attachments WHERE tag_id IS NOT NULL ORDER BY tag_id,kind,value
"#).bind(subject.0).fetch_all(&mut *tx).await.map_err(db)?;
        for row in rows {
            let id = TagId(row.try_get("tag_id").map_err(db)?);
            if let Ok(index) = tags.binary_search_by_key(&id.0, |tag| tag.tag.0) {
                tags[index].attachments.push(parse_reference(
                    &row.try_get::<String, _>("kind").map_err(db)?,
                    &row.try_get::<String, _>("value").map_err(db)?,
                )?);
            }
        }
        tx.commit().await.map_err(db)?;
        Ok(ConceptProjectionInput { watermark, tags })
    }
}
