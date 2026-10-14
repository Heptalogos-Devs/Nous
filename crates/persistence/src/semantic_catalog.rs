//! Semantic hints from existing embedding materials; never invokes a provider.
// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use crate::*;
use sqlx::Row;
impl AuthorityStore {
    pub async fn cached_semantic_scores(
        &self,
        subject: SubjectId,
        focus: &str,
        texts: &[String],
    ) -> Result<Vec<Option<f64>>> {
        if texts.len() > 256 {
            return Err(Error::Invalid("semantic catalog exceeded".into()));
        }
        let digest = blake3::hash(focus.as_bytes()).to_hex().to_string();
        let Some(source)=sqlx::query("SELECT space_hash,producer_hash,vector FROM embedding_materials WHERE subject_id=$1 AND content_digest=$2 ORDER BY created_at DESC,space_hash,producer_hash LIMIT 1")
            .bind(subject.0).bind(digest).fetch_optional(self.pool()).await.map_err(database_error)? else { return Ok(vec![None;texts.len()]); };
        let vector: Vec<f32> = source.try_get("vector").map_err(database_error)?;
        let hashes = texts
            .iter()
            .map(|text| blake3::hash(text.as_bytes()).to_hex().to_string())
            .collect::<Vec<_>>();
        let rows=sqlx::query("SELECT content_digest,vector FROM embedding_materials WHERE subject_id=$1 AND content_digest=ANY($2::text[]) AND space_hash=$3 AND producer_hash=$4")
            .bind(subject.0).bind(&hashes).bind(source.try_get::<String,_>("space_hash").map_err(database_error)?).bind(source.try_get::<String,_>("producer_hash").map_err(database_error)?).fetch_all(self.pool()).await.map_err(database_error)?;
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
