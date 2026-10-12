// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use crate::{AuthorityStore, database_error as db};
use chrono::{DateTime, Utc};
use nous_core::*;
use serde::{Deserialize, Serialize};
use sqlx::Row;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ServingView {
    Current,
    Historical {
        snapshot_digest: String,
        as_of: DateTime<Utc>,
        revision_view: RevisionView,
    },
}
impl ServingView {
    pub fn digest(&self) -> Option<&str> {
        match self {
            Self::Current => None,
            Self::Historical {
                snapshot_digest, ..
            } => Some(snapshot_digest),
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServingRecord {
    pub generation_id: ServingGenerationId,
    pub subject: SubjectId,
    pub family: String,
    pub view: ServingView,
    pub space: String,
    pub authority_watermark: i64,
    pub config_digest: String,
    pub artifact_hash: String,
    pub built_at: DateTime<Utc>,
    pub metadata: serde_json::Value,
}

fn decode(row: sqlx::postgres::PgRow) -> Result<ServingRecord> {
    Ok(ServingRecord {
        generation_id: ServingGenerationId(row.try_get("generation_id").map_err(db)?),
        subject: SubjectId(row.try_get("subject_id").map_err(db)?),
        family: row.try_get("family").map_err(db)?,
        view: serde_json::from_value(row.try_get("view_descriptor").map_err(db)?)
            .map_err(|e| Error::Infrastructure(e.to_string()))?,
        space: row
            .try_get::<Option<String>, _>("space_signature")
            .map_err(db)?
            .unwrap_or_default(),
        authority_watermark: row.try_get("authority_watermark").map_err(db)?,
        config_digest: row.try_get("config_digest").map_err(db)?,
        artifact_hash: row.try_get("artifact_hash").map_err(db)?,
        built_at: row.try_get("built_at").map_err(db)?,
        metadata: row.try_get("metadata").map_err(db)?,
    })
}

impl AuthorityStore {
    /// The Serving owner has rejected this artifact. Remove only that exact current pointer.
    pub async fn fail_serving_generation(&self, record: &ServingRecord) -> Result<()> {
        let mut tx = self.begin().await?;
        let key = format!(
            "serving:{}:{}:{}",
            record.subject.0, record.family, record.space
        );
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
            .bind(key)
            .execute(&mut *tx)
            .await
            .map_err(db)?;
        sqlx::query(
            "UPDATE serving_generations SET state='failed',published_at=$2 WHERE generation_id=$1",
        )
        .bind(record.generation_id.0)
        .bind(Utc::now())
        .execute(&mut *tx)
        .await
        .map_err(db)?;
        sqlx::query("DELETE FROM serving_current WHERE generation_id=$1")
            .bind(record.generation_id.0)
            .execute(&mut *tx)
            .await
            .map_err(db)?;
        tx.commit().await.map_err(db)
    }

    pub async fn serving_current(&self, subject: SubjectId) -> Result<Vec<ServingRecord>> {
        sqlx::query("SELECT g.* FROM serving_current c JOIN serving_generations g USING(generation_id) WHERE c.subject_id=$1 AND g.state='ready' ORDER BY c.family,c.space_signature")
            .bind(subject.0).fetch_all(self.pool()).await.map_err(db)?.into_iter().map(decode).collect()
    }

    /// Immutable artifacts remain reusable across readout/profile changes.
    pub async fn serving_reusable(&self, subject: SubjectId) -> Result<Vec<ServingRecord>> {
        sqlx::query("SELECT * FROM serving_generations WHERE subject_id=$1 AND view_descriptor->>'kind'='current' AND state IN ('ready','retired') ORDER BY authority_watermark DESC,built_at DESC,generation_id")
            .bind(subject.0).fetch_all(self.pool()).await.map_err(db)?.into_iter().map(decode).collect()
    }

    pub async fn promote_generation(&self, id: ServingGenerationId) -> Result<bool> {
        let mut tx = self.begin().await?;
        let record = sqlx::query("SELECT * FROM serving_generations WHERE generation_id=$1 AND state IN ('ready','retired')")
            .bind(id.0).fetch_optional(&mut *tx).await.map_err(db)?.map(decode).transpose()?;
        let Some(record) = record else {
            return Ok(false);
        };
        if record.view != ServingView::Current {
            return Err(Error::Invalid(
                "historical generation cannot be promoted as current".into(),
            ));
        }
        let key = format!(
            "serving:{}:{}:{}",
            record.subject.0, record.family, record.space
        );
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
            .bind(key)
            .execute(&mut *tx)
            .await
            .map_err(db)?;
        let desired: i64 = sqlx::query_scalar("SELECT COALESCE(max(desired_authority_seq),0) FROM projection_watermarks WHERE subject_id=$1 AND family=$2 AND (space_signature=$3 OR space_signature='*')")
            .bind(record.subject.0).bind(&record.family).bind(&record.space).fetch_one(&mut *tx).await.map_err(db)?;
        let desired = if record.family == "topology" {
            sqlx::query_scalar::<_, i64>("SELECT authority_seq FROM subjects WHERE subject_id=$1")
                .bind(record.subject.0)
                .fetch_one(&mut *tx)
                .await
                .map_err(db)?
                .max(desired)
        } else {
            desired
        };
        if record.authority_watermark < desired {
            return Ok(false);
        }
        sqlx::query("UPDATE serving_generations SET state='retired',published_at=$4 WHERE generation_id IN (SELECT generation_id FROM serving_current WHERE subject_id=$1 AND family=$2 AND space_signature=$3) AND generation_id<>$5")
            .bind(record.subject.0).bind(&record.family).bind(&record.space).bind(Utc::now()).bind(id.0).execute(&mut *tx).await.map_err(db)?;
        sqlx::query(
            "UPDATE serving_generations SET state='ready',published_at=$2 WHERE generation_id=$1",
        )
        .bind(id.0)
        .bind(Utc::now())
        .execute(&mut *tx)
        .await
        .map_err(db)?;
        sqlx::query("INSERT INTO serving_current(subject_id,family,space_signature,generation_id) VALUES($1,$2,$3,$4) ON CONFLICT(subject_id,family,space_signature) DO UPDATE SET generation_id=excluded.generation_id")
            .bind(record.subject.0).bind(&record.family).bind(&record.space).bind(id.0).execute(&mut *tx).await.map_err(db)?;
        tx.commit().await.map_err(db)?;
        Ok(true)
    }

    pub async fn publish_generation(&self, record: ServingRecord) -> Result<ServingRecord> {
        if record.view != ServingView::Current {
            return self.publish_historical_generation(record).await;
        }
        let mut tx = self.begin().await?;
        let key = format!(
            "serving:{}:{}:{}",
            record.subject.0, record.family, record.space
        );
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
            .bind(key)
            .execute(&mut *tx)
            .await
            .map_err(db)?;
        let current=sqlx::query("SELECT g.* FROM serving_current c JOIN serving_generations g USING(generation_id) WHERE c.subject_id=$1 AND c.family=$2 AND c.space_signature=$3").bind(record.subject.0).bind(&record.family).bind(&record.space).fetch_optional(&mut *tx).await.map_err(db)?.map(decode).transpose()?;
        if current.as_ref().is_some_and(|value| {
            value.authority_watermark > record.authority_watermark
                || (value.authority_watermark == record.authority_watermark
                    && value.config_digest == record.config_digest)
        }) {
            let current = current
                .ok_or_else(|| Error::Infrastructure("serving current row disappeared".into()))?;
            tx.commit().await.map_err(db)?;
            return Ok(current);
        }
        let desired: i64 = sqlx::query_scalar("SELECT COALESCE(max(desired_authority_seq),0) FROM projection_watermarks WHERE subject_id=$1 AND family=$2 AND (space_signature=$3 OR space_signature='*')")
            .bind(record.subject.0).bind(&record.family).bind(&record.space).fetch_one(&mut *tx).await.map_err(db)?;
        sqlx::query("INSERT INTO serving_generations(generation_id,subject_id,family,space_signature,authority_watermark,config_digest,artifact_hash,state,built_at,published_at,metadata) VALUES($1,$2,$3,$4,$5,$6,$7,'ready',$8,$9,$10)")
            .bind(record.generation_id.0).bind(record.subject.0).bind(&record.family).bind(&record.space).bind(record.authority_watermark).bind(&record.config_digest).bind(&record.artifact_hash).bind(record.built_at).bind(Utc::now()).bind(&record.metadata).execute(&mut *tx).await.map_err(db)?;
        let desired = if record.family == "topology" {
            sqlx::query_scalar::<_, i64>("SELECT authority_seq FROM subjects WHERE subject_id=$1")
                .bind(record.subject.0)
                .fetch_one(&mut *tx)
                .await
                .map_err(db)?
                .max(desired)
        } else {
            desired
        };
        if record.authority_watermark >= desired {
            sqlx::query("INSERT INTO serving_current(subject_id,family,space_signature,generation_id) VALUES($1,$2,$3,$4) ON CONFLICT(subject_id,family,space_signature) DO UPDATE SET generation_id=excluded.generation_id").bind(record.subject.0).bind(&record.family).bind(&record.space).bind(record.generation_id.0).execute(&mut *tx).await.map_err(db)?;
            if let Some(current) = current {
                sqlx::query(
                    "UPDATE serving_generations SET state='retired',published_at=$2 WHERE generation_id=$1",
                )
                .bind(current.generation_id.0)
                .bind(Utc::now())
                .execute(&mut *tx)
                .await
                .map_err(db)?;
            }
        } else {
            sqlx::query("UPDATE serving_generations SET state='retired' WHERE generation_id=$1")
                .bind(record.generation_id.0)
                .execute(&mut *tx)
                .await
                .map_err(db)?;
        }
        tx.commit().await.map_err(db)?;
        Ok(record)
    }
    pub async fn historical_serving_reusable(
        &self,
        subject: SubjectId,
        digest: &str,
    ) -> Result<Vec<ServingRecord>> {
        sqlx::query("SELECT * FROM serving_generations WHERE subject_id=$1 AND view_descriptor->>'kind'='historical' AND view_descriptor->>'snapshot_digest'=$2 AND state IN ('ready','retired') ORDER BY built_at DESC,generation_id")
            .bind(subject.0).bind(digest).fetch_all(self.pool()).await.map_err(db)?.into_iter().map(decode).collect()
    }
    async fn publish_historical_generation(&self, record: ServingRecord) -> Result<ServingRecord> {
        let mut tx = self.begin().await?;
        let view =
            serde_json::to_value(&record.view).map_err(|e| Error::Infrastructure(e.to_string()))?;
        let key = format!(
            "historical:{}:{}:{}:{}:{}",
            record.subject.0,
            record.view.digest().unwrap_or_default(),
            record.family,
            record.space,
            record.config_digest
        );
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
            .bind(key)
            .execute(&mut *tx)
            .await
            .map_err(db)?;
        sqlx::query("INSERT INTO serving_generations(generation_id,subject_id,family,space_signature,authority_watermark,config_digest,artifact_hash,state,built_at,published_at,metadata,view_descriptor) VALUES($1,$2,$3,$4,$5,$6,$7,'retired',$8,$8,$9,$10)")
            .bind(record.generation_id.0).bind(record.subject.0).bind(&record.family).bind(&record.space).bind(record.authority_watermark).bind(&record.config_digest).bind(&record.artifact_hash).bind(record.built_at).bind(&record.metadata).bind(view).execute(&mut *tx).await.map_err(db)?;
        tx.commit().await.map_err(db)?;
        Ok(record)
    }
}
