//! Bounded source enumeration and backup mechanics for legacy embedding rows.
// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use crate::{AuthorityStore, database_error as db};
use chrono::{DateTime, Utc};
use nous_core::{Result, SubjectId};
use sqlx::Row;

/// A page of persisted event instants at which readable document text can
/// change. Callers use keyset pagination so upgrades do not load all events.
pub struct EmbeddingUpgradeInstants {
    pub instants: Vec<DateTime<Utc>>,
}

impl AuthorityStore {
    /// Find Subjects with legacy active cache rows that do not have any known
    /// canonical cognitive source binding.
    pub async fn legacy_embedding_subject_page(
        &self,
        after: Option<SubjectId>,
        limit: usize,
    ) -> Result<Vec<SubjectId>> {
        if limit == 0 || limit > 256 {
            return Err(nous_core::Error::new(
                nous_core::ErrorOwner::Persistence,
                nous_core::ErrorCode::InvalidRequest,
                "legacy embedding subject page limit must be 1..256",
            ));
        }
        Ok(sqlx::query_scalar::<_, uuid::Uuid>(
            r#"
SELECT DISTINCT m.subject_id
FROM embedding_materials m
WHERE NOT EXISTS (
    SELECT 1 FROM embedding_material_uses u
    WHERE u.subject_id=m.subject_id AND u.content_digest=m.content_digest
      AND u.space_hash=m.space_hash AND u.producer_hash=m.producer_hash
)
  AND ($1::uuid IS NULL OR m.subject_id>$1)
ORDER BY m.subject_id
LIMIT $2
"#,
        )
        .bind(after.map(|subject| subject.0))
        .bind(limit as i64)
        .fetch_all(self.pool())
        .await
        .map_err(db)?
        .into_iter()
        .map(SubjectId)
        .collect())
    }

    /// Return one bounded page of timestamps that may change historical
    /// document membership or representation. Duplicate timestamps are
    /// collapsed by SQL because one authority transaction can write many rows.
    pub async fn embedding_document_change_page(
        &self,
        subject: SubjectId,
        after: Option<DateTime<Utc>>,
        limit: usize,
    ) -> Result<EmbeddingUpgradeInstants> {
        if limit == 0 || limit > 256 {
            return Err(nous_core::Error::new(
                nous_core::ErrorOwner::Persistence,
                nous_core::ErrorCode::InvalidRequest,
                "legacy embedding event page limit must be 1..256",
            ));
        }
        let rows = sqlx::query(
            r#"
WITH changes(event_at) AS (
    SELECT created_at FROM observation_occurrences WHERE subject_id=$1
    UNION SELECT created_at FROM source_regions WHERE subject_id=$1
    UNION SELECT created_at FROM derived_representations WHERE subject_id=$1
    UNION SELECT created_at FROM derived_regions WHERE subject_id=$1
    UNION SELECT created_at FROM memory_objects WHERE subject_id=$1
    UNION SELECT recorded_at FROM memory_revisions WHERE subject_id=$1
    UNION SELECT created_at FROM cognitive_schemas WHERE subject_id=$1
    UNION SELECT r.recorded_at FROM cognitive_schema_revisions r JOIN cognitive_schemas o USING(schema_id) WHERE o.subject_id=$1
    UNION SELECT created_at FROM episode_objects WHERE subject_id=$1
    UNION SELECT recorded_at FROM episode_revisions WHERE subject_id=$1
    UNION SELECT created_at FROM journal_objects WHERE subject_id=$1
    UNION SELECT recorded_at FROM journal_revisions WHERE subject_id=$1
    UNION SELECT created_at FROM tags WHERE subject_id=$1
    UNION SELECT r.created_at FROM tag_revisions r JOIN tags t USING(tag_id) WHERE t.subject_id=$1
    UNION SELECT recorded_at FROM authority_object_states WHERE subject_id=$1
)
SELECT DISTINCT event_at FROM changes
WHERE event_at IS NOT NULL AND ($2::timestamptz IS NULL OR event_at>$2)
ORDER BY event_at
LIMIT $3
"#,
        )
        .bind(subject.0)
        .bind(after)
        .bind(limit as i64)
        .fetch_all(self.pool())
        .await
        .map_err(db)?;
        let instants = rows
            .into_iter()
            .map(|row| row.try_get("event_at").map_err(db))
            .collect::<Result<Vec<DateTime<Utc>>>>()?;
        Ok(EmbeddingUpgradeInstants { instants })
    }

    pub async fn unbound_embedding_material_count(&self, subject: SubjectId) -> Result<i64> {
        sqlx::query_scalar(
            "SELECT count(*) FROM embedding_materials m WHERE m.subject_id=$1 AND NOT EXISTS (SELECT 1 FROM embedding_material_uses u WHERE u.subject_id=m.subject_id AND u.content_digest=m.content_digest AND u.space_hash=m.space_hash AND u.producer_hash=m.producer_hash)",
        )
        .bind(subject.0)
        .fetch_one(self.pool())
        .await
        .map_err(db)
    }

    /// Move unbound rows out of the queryable cache only after first preserving
    /// every byte in the independent legacy backup. The caller has finished
    /// all current and historical source discovery for this Subject.
    pub async fn back_up_unbound_embedding_materials_in(
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        subject: SubjectId,
    ) -> Result<i64> {
        sqlx::query(
            r#"
INSERT INTO legacy_embedding_materials_backup(
    subject_id,content_digest,space_hash,producer_hash,vector,source_created_at
)
SELECT m.subject_id,m.content_digest,m.space_hash,m.producer_hash,m.vector,m.created_at
FROM embedding_materials m
WHERE m.subject_id=$1
  AND NOT EXISTS (
      SELECT 1 FROM embedding_material_uses u
      WHERE u.subject_id=m.subject_id AND u.content_digest=m.content_digest
        AND u.space_hash=m.space_hash AND u.producer_hash=m.producer_hash
  )
ON CONFLICT DO NOTHING
"#,
        )
        .bind(subject.0)
        .execute(&mut **tx)
        .await
        .map_err(db)?;
        let removed = sqlx::query(
            r#"
DELETE FROM embedding_materials m
WHERE m.subject_id=$1
  AND NOT EXISTS (
      SELECT 1 FROM embedding_material_uses u
      WHERE u.subject_id=m.subject_id AND u.content_digest=m.content_digest
        AND u.space_hash=m.space_hash AND u.producer_hash=m.producer_hash
  )
"#,
        )
        .bind(subject.0)
        .execute(&mut **tx)
        .await
        .map_err(db)?
        .rows_affected() as i64;
        Ok(removed)
    }
}
