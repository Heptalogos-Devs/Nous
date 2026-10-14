// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;
use nous_persistence::database_error as db;
use sqlx::{Postgres, Transaction};

/// Material-owned immutable content admission, composable with another owner's transaction.
#[derive(Clone)]
pub struct ArtifactRegistration {
    store: AuthorityStore,
    objects: ObjectStore,
}

impl ArtifactRegistration {
    pub fn new(store: AuthorityStore, objects: ObjectStore) -> Self {
        Self { store, objects }
    }

    /// The caller holds an object reference guard until its Authority transaction finishes.
    /// Failed transactions leave unreferenced immutable CAS bytes for normal reclamation.
    pub async fn write_in<S>(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        subject: SubjectId,
        metadata: UploadMetadata,
        chunks: S,
        max_bytes: u64,
        now: DateTime<Utc>,
    ) -> Result<Artifact>
    where
        S: Stream<Item = Result<Vec<u8>>> + Send,
    {
        if metadata.media_type.trim().is_empty() {
            return Err(Error::Invalid("artifact media_type is required".into()));
        }
        let (hash, size) = self.objects.put_chunks(chunks, max_bytes).await?;
        let byte_length = i64::try_from(size)
            .map_err(|_| Error::Invalid("artifact byte length exceeds database bounds".into()))?;
        let artifact = Artifact {
            artifact_id: ArtifactId::new(),
            subject_id: subject,
            content_hash: hash,
            byte_length: size,
            media_type: metadata.media_type,
            created_at: now,
            metadata: metadata.metadata,
        };
        sqlx::query("INSERT INTO artifacts(artifact_id,subject_id,content_hash,byte_length,media_type,created_at,metadata) VALUES($1,$2,$3,$4,$5,$6,$7) ON CONFLICT(subject_id,content_hash) DO NOTHING")
            .bind(artifact.artifact_id.0).bind(subject.0).bind(&artifact.content_hash)
            .bind(byte_length).bind(&artifact.media_type)
            .bind(now).bind(&artifact.metadata).execute(&mut **tx).await.map_err(db)?;
        // A separate statement sees the winning row even when a concurrent insertion waited.
        let row = sqlx::query("SELECT artifact_id,subject_id,content_hash,byte_length,media_type,created_at,metadata FROM artifacts WHERE subject_id=$1 AND content_hash=$2")
            .bind(subject.0).bind(&artifact.content_hash).fetch_one(&mut **tx).await.map_err(db)?;
        let artifact = decode_artifact(row)?;
        self.store
            .ensure_identity_addresses_in(
                tx,
                subject,
                &[CognitiveRef::Artifact(artifact.artifact_id)],
                "",
            )
            .await?;
        Ok(artifact)
    }
}

impl MaterialService {
    /// Explicit instance-wide reclamation; Artifact Authority in every Subject protects shared CAS bytes.
    pub async fn clean(&self) -> Result<(u64, u64)> {
        let guard = self.objects.reference_guard(true).await?;
        let retained: Vec<String> =
            sqlx::query_scalar("SELECT DISTINCT content_hash FROM artifacts")
                .fetch_all(self.store.pool())
                .await
                .map_err(db)?;
        let result = self
            .objects
            .reclaim(&retained.into_iter().collect())
            .await?;
        drop(guard);
        Ok(result)
    }
}

pub(super) fn decode_artifact(row: sqlx::postgres::PgRow) -> Result<Artifact> {
    Ok(Artifact {
        artifact_id: ArtifactId(row.try_get("artifact_id").map_err(db)?),
        subject_id: SubjectId(row.try_get("subject_id").map_err(db)?),
        content_hash: row.try_get("content_hash").map_err(db)?,
        byte_length: row.try_get::<i64, _>("byte_length").map_err(db)? as u64,
        media_type: row.try_get("media_type").map_err(db)?,
        created_at: row.try_get("created_at").map_err(db)?,
        metadata: row.try_get("metadata").map_err(db)?,
    })
}
