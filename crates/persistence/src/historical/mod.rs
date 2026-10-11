//! SQL mechanics for owner-selected historical state and immutable revision identities.
// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
pub(crate) mod binding;
pub(crate) mod projection;
pub(crate) mod text;

use crate::{AuthorityStore, database_error as db};
use nous_core::*;
use sqlx::Row;
#[derive(Debug)]
pub struct HistoricalCognitionRows {
    pub object_kind: String,
    pub object_id: uuid::Uuid,
    pub state: serde_json::Value,
    pub revisions: Vec<uuid::Uuid>,
}
#[derive(Debug)]
pub struct HistoricalTagRows {
    pub tag_id: TagId,
    pub revision_id: uuid::Uuid,
    pub state: serde_json::Value,
    pub label: String,
    pub description: Option<String>,
    pub kind_hint: Option<String>,
}
#[derive(Debug)]
pub struct HistoricalOwnerRows {
    pub cognition: Vec<HistoricalCognitionRows>,
    pub tags: Vec<HistoricalTagRows>,
    pub associations: Vec<AssociationEvidenceId>,
    pub schema_evidence_links: Vec<uuid::Uuid>,
    pub entity_bindings: Vec<uuid::Uuid>,
    pub lexical: Vec<HistoricalLexicalVisibility>,
    pub material: Vec<CognitiveRef>,
}
impl AuthorityStore {
    pub async fn historical_owner_rows(
        &self,
        subject: SubjectId,
        as_of: chrono::DateTime<chrono::Utc>,
    ) -> Result<HistoricalOwnerRows> {
        let mut tx = self.begin().await?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
            .execute(&mut *tx)
            .await
            .map_err(db)?;
        let mut cognition = Vec::new();
        for (kind, objects, revisions, object_id, revision_id) in [
            (
                "memory",
                "memory_objects",
                "memory_revisions",
                "memory_id",
                "memory_revision_id",
            ),
            (
                "cognitive_schema",
                "cognitive_schemas",
                "cognitive_schema_revisions",
                "schema_id",
                "schema_revision_id",
            ),
            (
                "episode",
                "episode_objects",
                "episode_revisions",
                "episode_id",
                "episode_revision_id",
            ),
            (
                "journal",
                "journal_objects",
                "journal_revisions",
                "journal_id",
                "journal_revision_id",
            ),
        ] {
            let query = format!(
                "SELECT o.{object_id} object_id,h.state,array_agg(r.{revision_id} ORDER BY r.recorded_at DESC,r.revision_no DESC) revisions FROM {objects} o JOIN LATERAL (SELECT state FROM authority_object_states WHERE subject_id=o.subject_id AND object_kind=$2 AND object_ref=o.{object_id}::text AND recorded_at<=$3 ORDER BY recorded_at DESC,event_id DESC LIMIT 1) h ON true JOIN {revisions} r ON r.{object_id}=o.{object_id} AND r.recorded_at<=$3 WHERE o.subject_id=$1 AND o.created_at<=$3 AND o.purge_state='normal' GROUP BY o.{object_id},h.state ORDER BY o.{object_id}"
            );
            // Identifiers come only from the fixed four-owner table catalog above; values are bound.
            for row in sqlx::query(sqlx::AssertSqlSafe(query.as_str()))
                .bind(subject.0)
                .bind(kind)
                .bind(as_of)
                .fetch_all(&mut *tx)
                .await
                .map_err(db)?
            {
                cognition.push(HistoricalCognitionRows {
                    object_kind: kind.into(),
                    object_id: row.try_get("object_id").map_err(db)?,
                    state: row.try_get("state").map_err(db)?,
                    revisions: row.try_get("revisions").map_err(db)?,
                });
            }
        }
        let rows=sqlx::query("SELECT t.tag_id,h.state,r.tag_revision_id,r.label,r.description,r.kind_hint FROM tags t JOIN LATERAL (SELECT state FROM authority_object_states WHERE subject_id=t.subject_id AND object_kind='tag' AND object_ref=t.tag_id::text AND recorded_at<=$2 ORDER BY recorded_at DESC,event_id DESC LIMIT 1) h ON true JOIN LATERAL (SELECT * FROM tag_revisions WHERE tag_id=t.tag_id AND created_at<=$2 ORDER BY created_at DESC,revision_no DESC LIMIT 1) r ON true WHERE t.subject_id=$1 AND t.created_at<=$2 ORDER BY t.tag_id").bind(subject.0).bind(as_of).fetch_all(&mut *tx).await.map_err(db)?;
        let tags = rows
            .into_iter()
            .map(|row| {
                Ok(HistoricalTagRows {
                    tag_id: TagId(row.try_get("tag_id").map_err(db)?),
                    revision_id: row.try_get("tag_revision_id").map_err(db)?,
                    state: row.try_get("state").map_err(db)?,
                    label: row.try_get("label").map_err(db)?,
                    description: row.try_get("description").map_err(db)?,
                    kind_hint: row.try_get("kind_hint").map_err(db)?,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let associations=sqlx::query_scalar::<_,uuid::Uuid>("SELECT association_evidence_id FROM association_evidence WHERE subject_id=$1 AND created_at<=$2 AND (revoked_at IS NULL OR revoked_at>$2) ORDER BY association_evidence_id").bind(subject.0).bind(as_of).fetch_all(&mut *tx).await.map_err(db)?.into_iter().map(AssociationEvidenceId).collect();
        let schema_evidence_links = sqlx::query_scalar("SELECT link_id FROM cognitive_schema_evidence_links WHERE subject_id=$1 AND created_at<=$2 AND (revoked_at IS NULL OR revoked_at>$2) ORDER BY link_id").bind(subject.0).bind(as_of).fetch_all(&mut *tx).await.map_err(db)?;
        let entity_bindings = sqlx::query_scalar("SELECT binding_revision_id FROM (SELECT DISTINCT ON (m.mention_id) r.binding_revision_id,m.mention_id FROM entity_mentions m JOIN entity_binding_revisions r USING(mention_id) WHERE m.subject_id=$1 AND m.created_at<=$2 AND r.created_at<=$2 ORDER BY m.mention_id,r.created_at DESC,r.revision_no DESC) effective ORDER BY binding_revision_id").bind(subject.0).bind(as_of).fetch_all(&mut *tx).await.map_err(db)?;
        let rows=sqlx::query("SELECT b.lexical_ref,b.object_kind,b.canonical_ref,h.state FROM lexical_bindings b JOIN LATERAL (SELECT state FROM authority_object_states WHERE subject_id=$1 AND object_kind='lexical_visibility' AND object_ref=b.lexical_ref AND recorded_at<=$2 ORDER BY recorded_at DESC,event_id DESC LIMIT 1) h ON true WHERE b.created_at<=$2 AND (b.tombstoned_at IS NULL OR b.tombstoned_at>$2) ORDER BY b.lexical_ref").bind(subject.0).bind(as_of).fetch_all(&mut *tx).await.map_err(db)?;
        let lexical = rows
            .into_iter()
            .map(|row| {
                let state: serde_json::Value = row.try_get("state").map_err(db)?;
                Ok(HistoricalLexicalVisibility {
                    lexical_ref: row.try_get("lexical_ref").map_err(db)?,
                    object_kind: row.try_get("object_kind").map_err(db)?,
                    canonical_ref: row.try_get("canonical_ref").map_err(db)?,
                    display_name: serde_json::from_value(state["display_name"].clone())
                        .map_err(|error| Error::Infrastructure(error.to_string()))?,
                    aliases: serde_json::from_value(state["aliases"].clone())
                        .map_err(|error| Error::Infrastructure(error.to_string()))?,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let rows=sqlx::query("SELECT 'occurrence' kind,occurrence_id::text value FROM observation_occurrences WHERE subject_id=$1 AND created_at<=$2 UNION ALL SELECT 'artifact',artifact_id::text FROM observation_occurrences WHERE subject_id=$1 AND created_at<=$2 AND artifact_id IS NOT NULL UNION ALL SELECT 'source_region',source_region_id::text FROM source_regions WHERE subject_id=$1 AND created_at<=$2 UNION ALL SELECT 'derived_representation',derived_representation_id::text FROM derived_representations WHERE subject_id=$1 AND created_at<=$2 UNION ALL SELECT 'derived_region',derived_region_id::text FROM derived_regions WHERE subject_id=$1 AND created_at<=$2 ORDER BY kind,value").bind(subject.0).bind(as_of).fetch_all(&mut *tx).await.map_err(db)?;
        let mut material = rows
            .into_iter()
            .map(|row| {
                parse_reference(
                    &row.try_get::<String, _>("kind").map_err(db)?,
                    &row.try_get::<String, _>("value").map_err(db)?,
                )
            })
            .collect::<Result<Vec<_>>>()?;
        material.sort_by_key(ToString::to_string);
        material.dedup();
        tx.commit().await.map_err(db)?;
        Ok(HistoricalOwnerRows {
            cognition,
            tags,
            associations,
            schema_evidence_links,
            entity_bindings,
            lexical,
            material,
        })
    }
}
