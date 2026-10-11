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
    pub purge_epoch: i64,
    pub implementation_id: String,
    pub implementation_revision: String,
    pub config_digest: String,
    pub artifact_location: String,
    pub artifact_hash: String,
    pub built_at: DateTime<Utc>,
    pub metadata: serde_json::Value,
}

fn decode(row: sqlx::postgres::PgRow) -> Result<ServingRecord> {
    Ok(ServingRecord {
        generation_id: ServingGenerationId(row.try_get("generation_id").map_err(db)?),
        subject: SubjectId(row.try_get("subject_id").map_err(db)?),
        family: row.try_get("family").map_err(db)?,
        view: serde_json::from_value(row.try_get("view_descriptor").map_err(db)?).map_err(|e| {
            Error::new(
                nous_core::ErrorOwner::Persistence,
                nous_core::ErrorCode::InternalFailure,
                "Could not decode the persisted Serving view descriptor",
            )
            .with_source(e)
        })?,
        space: row
            .try_get::<Option<String>, _>("space_signature")
            .map_err(db)?
            .unwrap_or_default(),
        authority_watermark: row.try_get("authority_watermark").map_err(db)?,
        purge_epoch: row.try_get("purge_epoch").map_err(db)?,
        implementation_id: row.try_get("implementation_id").map_err(db)?,
        implementation_revision: row.try_get("implementation_revision").map_err(db)?,
        config_digest: row.try_get("config_digest").map_err(db)?,
        artifact_location: row.try_get("artifact_location").map_err(db)?,
        artifact_hash: row.try_get("artifact_hash").map_err(db)?,
        built_at: row.try_get("built_at").map_err(db)?,
        metadata: row.try_get("metadata").map_err(db)?,
    })
}

impl AuthorityStore {
    /// Detects whether a prior-epoch artifact is still owned by the live Serving
    /// catalog. Semantic owners call this before final purge deletion so direct
    /// completion cannot skip Kernel-coordinated artifact reclamation.
    pub async fn has_unreclaimed_serving_artifact_in(
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        subject: SubjectId,
    ) -> Result<bool> {
        sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM serving_generations g JOIN subjects s USING(subject_id) WHERE g.subject_id=$1 AND g.purge_epoch<s.serving_purge_epoch AND g.artifact_location<>'')",
        )
        .bind(subject.0)
        .fetch_one(&mut **tx)
        .await
        .map_err(db)
    }

    /// The Serving owner has rejected this artifact; retain its audit row as failed.
    pub async fn fail_serving_generation(&self, record: &ServingRecord) -> Result<()> {
        let mut tx = self.begin().await?;
        sqlx::query(
            "UPDATE serving_generations SET state='failed',published_at=$2 WHERE generation_id=$1",
        )
        .bind(record.generation_id.0)
        .bind(Utc::now())
        .execute(&mut *tx)
        .await
        .map_err(db)?;
        tx.commit().await.map_err(db)
    }

    pub async fn serving_current(&self, subject: SubjectId) -> Result<Vec<ServingRecord>> {
        sqlx::query("SELECT g.* FROM serving_generations g JOIN subjects s USING(subject_id) WHERE g.subject_id=$1 AND g.state='ready' AND g.view_descriptor->>'kind'='current' AND g.purge_epoch=s.serving_purge_epoch ORDER BY g.family,COALESCE(g.space_signature,'')")
            .bind(subject.0).fetch_all(self.pool()).await.map_err(db)?.into_iter().map(decode).collect()
    }

    /// Immutable artifacts remain reusable across readout/profile changes.
    pub async fn serving_reusable(&self, subject: SubjectId) -> Result<Vec<ServingRecord>> {
        sqlx::query("SELECT g.* FROM serving_generations g JOIN subjects s USING(subject_id) WHERE g.subject_id=$1 AND g.view_descriptor->>'kind'='current' AND g.state IN ('ready','retired') AND g.purge_epoch=s.serving_purge_epoch ORDER BY g.authority_watermark DESC,g.built_at DESC,generation_id")
            .bind(subject.0).fetch_all(self.pool()).await.map_err(db)?.into_iter().map(decode).collect()
    }

    pub async fn promote_generation(&self, id: ServingGenerationId) -> Result<bool> {
        let mut tx = self.begin().await?;
        // Read the immutable key first, then serialize on that key before
        // locking generation rows. Locking the target row before the advisory
        // lock lets concurrent promotions hold opposite generation rows while
        // waiting to retire one another.
        let record = sqlx::query("SELECT g.* FROM serving_generations g JOIN subjects s USING(subject_id) WHERE g.generation_id=$1 AND g.state IN ('ready','retired') AND g.purge_epoch=s.serving_purge_epoch")
            .bind(id.0).fetch_optional(&mut *tx).await.map_err(db)?.map(decode).transpose()?;
        let Some(record) = record else {
            return Ok(false);
        };
        if record.view != ServingView::Current {
            return Err(Error::new(
                nous_core::ErrorOwner::Resource,
                nous_core::ErrorCode::InvalidRequest,
                "Invalid request",
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
        let purge_epoch: i64 = sqlx::query_scalar(
            "SELECT serving_purge_epoch FROM subjects WHERE subject_id=$1 FOR SHARE",
        )
        .bind(record.subject.0)
        .fetch_one(&mut *tx)
        .await
        .map_err(db)?;
        let record = sqlx::query("SELECT * FROM serving_generations WHERE generation_id=$1 AND state IN ('ready','retired') AND purge_epoch=$2 FOR UPDATE")
            .bind(id.0).bind(purge_epoch).fetch_optional(&mut *tx).await.map_err(db)?.map(decode).transpose()?;
        let Some(record) = record else {
            return Ok(false);
        };
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
        sqlx::query("UPDATE serving_generations SET state='retired',published_at=$4 WHERE subject_id=$1 AND family=$2 AND COALESCE(space_signature,'')=$3 AND view_descriptor->>'kind'='current' AND state='ready' AND generation_id<>$5")
            .bind(record.subject.0).bind(&record.family).bind(&record.space).bind(Utc::now()).bind(id.0).execute(&mut *tx).await.map_err(db)?;
        sqlx::query(
            "UPDATE serving_generations SET state='ready',published_at=$2 WHERE generation_id=$1",
        )
        .bind(id.0)
        .bind(Utc::now())
        .execute(&mut *tx)
        .await
        .map_err(db)?;
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
        let purge_epoch: i64 = sqlx::query_scalar(
            "SELECT serving_purge_epoch FROM subjects WHERE subject_id=$1 FOR SHARE",
        )
        .bind(record.subject.0)
        .fetch_one(&mut *tx)
        .await
        .map_err(db)?;
        if record.purge_epoch != purge_epoch {
            return Err(Error::new(
                nous_core::ErrorOwner::Persistence,
                nous_core::ErrorCode::FailedPrecondition,
                "Serving build crossed a purge checkpoint",
            ));
        }
        let current=sqlx::query("SELECT g.* FROM serving_generations g WHERE g.subject_id=$1 AND g.family=$2 AND COALESCE(g.space_signature,'')=$3 AND g.view_descriptor->>'kind'='current' AND g.state='ready' AND g.purge_epoch=$4").bind(record.subject.0).bind(&record.family).bind(&record.space).bind(record.purge_epoch).fetch_optional(&mut *tx).await.map_err(db)?.map(decode).transpose()?;
        if current.as_ref().is_some_and(|value| {
            value.authority_watermark > record.authority_watermark
                || (value.authority_watermark == record.authority_watermark
                    && value.config_digest == record.config_digest
                    && value.implementation_id == record.implementation_id
                    && value.implementation_revision == record.implementation_revision)
        }) {
            let current = current.ok_or_else(|| {
                Error::new(
                    nous_core::ErrorOwner::Persistence,
                    nous_core::ErrorCode::InternalFailure,
                    "serving current row disappeared",
                )
            })?;
            tx.commit().await.map_err(db)?;
            return Ok(current);
        }
        let desired: i64 = sqlx::query_scalar("SELECT COALESCE(max(desired_authority_seq),0) FROM projection_watermarks WHERE subject_id=$1 AND family=$2 AND (space_signature=$3 OR space_signature='*')")
            .bind(record.subject.0).bind(&record.family).bind(&record.space).fetch_one(&mut *tx).await.map_err(db)?;
        // Keep the replacement non-current until the old ready row is retired.
        // The partial unique index makes the single current state explicit.
        sqlx::query("INSERT INTO serving_generations(generation_id,subject_id,family,space_signature,authority_watermark,purge_epoch,implementation_id,implementation_revision,config_digest,artifact_location,artifact_hash,state,built_at,published_at,metadata) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,'retired',$12,$13,$14)")
            .bind(record.generation_id.0).bind(record.subject.0).bind(&record.family).bind(&record.space).bind(record.authority_watermark).bind(record.purge_epoch).bind(&record.implementation_id).bind(&record.implementation_revision).bind(&record.config_digest).bind(&record.artifact_location).bind(&record.artifact_hash).bind(record.built_at).bind(Utc::now()).bind(&record.metadata).execute(&mut *tx).await.map_err(db)?;
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
            sqlx::query("UPDATE serving_generations SET state='ready',published_at=$2 WHERE generation_id=$1")
                .bind(record.generation_id.0).bind(Utc::now()).execute(&mut *tx).await.map_err(db)?;
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
        sqlx::query("SELECT g.* FROM serving_generations g JOIN subjects s USING(subject_id) WHERE g.subject_id=$1 AND g.view_descriptor->>'kind'='historical' AND g.view_descriptor->>'snapshot_digest'=$2 AND g.state IN ('ready','retired') AND g.purge_epoch=s.serving_purge_epoch ORDER BY g.built_at DESC,generation_id")
            .bind(subject.0).bind(digest).fetch_all(self.pool()).await.map_err(db)?.into_iter().map(decode).collect()
    }
    async fn publish_historical_generation(&self, record: ServingRecord) -> Result<ServingRecord> {
        let mut tx = self.begin().await?;
        let view = serde_json::to_value(&record.view).map_err(|e| {
            Error::new(
                nous_core::ErrorOwner::Persistence,
                nous_core::ErrorCode::InternalFailure,
                "Could not encode the Serving view descriptor for persistence",
            )
            .with_source(e)
        })?;
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
        let purge_epoch: i64 = sqlx::query_scalar(
            "SELECT serving_purge_epoch FROM subjects WHERE subject_id=$1 FOR SHARE",
        )
        .bind(record.subject.0)
        .fetch_one(&mut *tx)
        .await
        .map_err(db)?;
        if record.purge_epoch != purge_epoch {
            return Err(Error::new(
                nous_core::ErrorOwner::Persistence,
                nous_core::ErrorCode::FailedPrecondition,
                "Serving build crossed a purge checkpoint",
            ));
        }
        sqlx::query("INSERT INTO serving_generations(generation_id,subject_id,family,space_signature,authority_watermark,purge_epoch,implementation_id,implementation_revision,config_digest,artifact_location,artifact_hash,state,built_at,published_at,metadata,view_descriptor) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,'retired',$12,$12,$13,$14)")
            .bind(record.generation_id.0).bind(record.subject.0).bind(&record.family).bind(&record.space).bind(record.authority_watermark).bind(record.purge_epoch).bind(&record.implementation_id).bind(&record.implementation_revision).bind(&record.config_digest).bind(&record.artifact_location).bind(&record.artifact_hash).bind(record.built_at).bind(&record.metadata).bind(view).execute(&mut *tx).await.map_err(db)?;
        tx.commit().await.map_err(db)?;
        Ok(record)
    }
}
