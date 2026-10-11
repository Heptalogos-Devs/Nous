//! Semantic hints from existing embedding materials; never invokes a provider.
// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use crate::*;
use sqlx::Row;
impl AuthorityStore {
    /// Remove only source bindings owned by the purged refs. Shared content
    /// vectors survive while any other active ref still binds them.
    pub async fn purge_embedding_material_uses_in(
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        subject: nous_core::SubjectId,
        references: &[nous_core::CognitiveRef],
    ) -> Result<()> {
        if references.is_empty() {
            return Ok(());
        }
        let parts = references
            .iter()
            .map(nous_core::reference_parts)
            .collect::<Vec<_>>();
        let kinds = parts
            .iter()
            .map(|(kind, _)| kind.clone())
            .collect::<Vec<_>>();
        let values = parts
            .iter()
            .map(|(_, value)| value.clone())
            .collect::<Vec<_>>();
        // Keep the two mutations as separate statements in the same owner
        // transaction. PostgreSQL data-modifying CTEs share one statement
        // snapshot, so a sibling anti-join would still see the bindings that
        // the CTE just deleted and could leave the final orphan vector behind.
        let removed = sqlx::query_as::<_, (String, String, String)>(
            r#"
DELETE FROM embedding_material_uses
WHERE subject_id=$1
  AND (ref_kind,ref_value) IN (
      SELECT ref_kind,ref_value
      FROM unnest($2::text[],$3::text[]) AS refs(ref_kind,ref_value)
  )
RETURNING content_digest,space_hash,producer_hash
"#,
        )
        .bind(subject.0)
        .bind(kinds)
        .bind(values)
        .fetch_all(&mut **tx)
        .await
        .map_err(database_error)?;
        let (digests, spaces, producers): (Vec<_>, Vec<_>, Vec<_>) =
            removed
                .into_iter()
                .fold((Vec::new(), Vec::new(), Vec::new()), |mut rows, row| {
                    rows.0.push(row.0);
                    rows.1.push(row.1);
                    rows.2.push(row.2);
                    rows
                });
        sqlx::query(
            r#"
DELETE FROM embedding_materials m
WHERE m.subject_id=$1
  AND EXISTS (
      SELECT 1
      FROM unnest($2::text[],$3::text[],$4::text[]) AS candidates(content_digest,space_hash,producer_hash)
      WHERE candidates.content_digest=m.content_digest
        AND candidates.space_hash=m.space_hash
        AND candidates.producer_hash=m.producer_hash
  )
  AND NOT EXISTS (
      SELECT 1 FROM embedding_material_uses u
      WHERE u.subject_id=m.subject_id AND u.content_digest=m.content_digest
        AND u.space_hash=m.space_hash AND u.producer_hash=m.producer_hash
  )
"#,
        )
        .bind(subject.0)
        .bind(digests)
        .bind(spaces)
        .bind(producers)
        .execute(&mut **tx)
        .await
        .map_err(database_error)?;
        Ok(())
    }

    pub async fn cached_semantic_scores(
        &self,
        subject: SubjectId,
        focus: &str,
        texts: &[String],
    ) -> Result<Vec<Option<f64>>> {
        if texts.len() > 32 {
            return Err(Error::new(
                nous_core::ErrorOwner::Concept,
                nous_core::ErrorCode::InvalidRequest,
                "semantic catalog exceeded",
            ));
        }
        let digest = blake3::hash(focus.as_bytes()).to_hex().to_string();
        let Some(source) = sqlx::query(
            r#"
SELECT m.space_hash,m.producer_hash,m.vector
FROM embedding_materials m
WHERE m.subject_id=$1 AND m.content_digest=$2
  AND EXISTS (
      SELECT 1 FROM embedding_material_uses u
      WHERE u.subject_id=m.subject_id AND u.content_digest=m.content_digest
        AND u.space_hash=m.space_hash AND u.producer_hash=m.producer_hash
  )
ORDER BY m.created_at DESC,m.space_hash,m.producer_hash
LIMIT 1
"#,
        )
        .bind(subject.0)
        .bind(digest)
        .fetch_optional(self.pool())
        .await
        .map_err(database_error)?
        else {
            return Ok(vec![None; texts.len()]);
        };
        let vector: Vec<f32> = source.try_get("vector").map_err(database_error)?;
        let hashes = texts
            .iter()
            .map(|text| blake3::hash(text.as_bytes()).to_hex().to_string())
            .collect::<Vec<_>>();
        let rows = sqlx::query(
            r#"
SELECT m.content_digest,m.vector
FROM embedding_materials m
WHERE m.subject_id=$1 AND m.content_digest=ANY($2::text[])
  AND m.space_hash=$3 AND m.producer_hash=$4
  AND EXISTS (
      SELECT 1 FROM embedding_material_uses u
      WHERE u.subject_id=m.subject_id AND u.content_digest=m.content_digest
        AND u.space_hash=m.space_hash AND u.producer_hash=m.producer_hash
  )
"#,
        )
        .bind(subject.0)
        .bind(&hashes)
        .bind(
            source
                .try_get::<String, _>("space_hash")
                .map_err(database_error)?,
        )
        .bind(
            source
                .try_get::<String, _>("producer_hash")
                .map_err(database_error)?,
        )
        .fetch_all(self.pool())
        .await
        .map_err(database_error)?;
        let mut scores = std::collections::HashMap::new();
        for row in rows {
            let candidate: Vec<f32> = row.try_get("vector").map_err(database_error)?;
            if candidate.len() != vector.len() || vector.is_empty() {
                continue;
            }
            let (mut dot, mut left, mut right) = (0.0f64, 0.0f64, 0.0f64);
            for (a, b) in vector.iter().zip(&candidate) {
                let (a, b) = (f64::from(*a), f64::from(*b));
                dot += a * b;
                left += a * a;
                right += b * b;
            }
            let score = dot / (left * right).sqrt();
            if score.is_finite() {
                scores.insert(
                    row.try_get::<String, _>("content_digest")
                        .map_err(database_error)?,
                    score.clamp(-1.0, 1.0),
                );
            }
        }
        Ok(hashes
            .iter()
            .map(|hash| scores.get(hash).copied())
            .collect())
    }
}
