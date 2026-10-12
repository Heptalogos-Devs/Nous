//! Serving readiness is interpreted by its owner, independently of wire types.
// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use crate::*;
use nous_persistence::database_error as db;
use nous_runtime::LaneStatus;
use sqlx::Row;

pub struct ProjectionFamilyStatus {
    pub name: String,
    pub state: LaneStatus,
    pub space_signature: String,
    pub desired_authority_seq: i64,
    pub authority_watermark: Option<i64>,
    pub generation_id: Option<ServingGenerationId>,
    pub config_digest: Option<String>,
    pub desired_config_digest: String,
}

impl ServingService {
    pub async fn family_status(&self, subject: SubjectId) -> Result<Vec<ProjectionFamilyStatus>> {
        self.store.require_subject(subject).await?;
        let config = self.configuration.snapshot_for_subject(subject)?;
        let rows = sqlx::query("SELECT COALESCE(w.family,c.family) AS family,COALESCE(w.space_signature,c.space_signature) AS space_signature,g.authority_watermark,g.generation_id,g.config_digest FROM projection_watermarks w FULL JOIN serving_current c ON c.subject_id=w.subject_id AND c.family=w.family AND c.space_signature=w.space_signature LEFT JOIN serving_generations g ON g.generation_id=c.generation_id WHERE COALESCE(w.subject_id,c.subject_id)=$1 ORDER BY family,space_signature")
            .bind(subject.0).fetch_all(self.store.pool()).await.map_err(db)?;
        let watermarks = self.store.projection_watermarks(subject).await?;
        let mut families = Vec::with_capacity(rows.len());
        for row in rows {
            let name: String = row.try_get("family").map_err(db)?;
            let space_signature: String = row.try_get("space_signature").map_err(db)?;
            let desired_authority_seq = watermarks.desired(&name, &space_signature);
            let authority_watermark: Option<i64> =
                row.try_get("authority_watermark").map_err(db)?;
            let config_digest: Option<String> = row.try_get("config_digest").map_err(db)?;
            let desired_config_digest = self
                .config_digest(subject, &name, &space_signature, &config)
                .await?;
            let state = match authority_watermark {
                Some(watermark)
                    if watermark >= desired_authority_seq
                        && config_digest.as_deref() == Some(desired_config_digest.as_str()) =>
                {
                    LaneStatus::Ready
                }
                Some(_) => LaneStatus::Stale,
                None => LaneStatus::Unavailable,
            };
            families.push(ProjectionFamilyStatus {
                name,
                state,
                space_signature,
                desired_authority_seq,
                authority_watermark,
                generation_id: row
                    .try_get::<Option<uuid::Uuid>, _>("generation_id")
                    .map_err(db)?
                    .map(ServingGenerationId),
                config_digest,
                desired_config_digest,
            });
        }
        Ok(families)
    }
}
