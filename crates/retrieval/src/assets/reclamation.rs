// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use crate::{assets::files::io, *};
use sqlx::Row;

#[derive(Debug, Default, Serialize)]
pub struct ReclamationReport {
    pub reclaimed: Vec<ServingGenerationId>,
    pub bytes_reclaimed: u64,
    pub readers_active: bool,
    pub orphan_directories: usize,
}

impl ServingService {
    /// Purge is epoch-scoped and deliberately bypasses the ordinary grace and
    /// research-pin protections. Active immutable query leases remain the only
    /// reason an old artifact survives a completed checkpoint.
    pub async fn reclaim_purged(
        &self,
        subject: SubjectId,
        current_epoch: i64,
    ) -> Result<ReclamationReport> {
        let Ok(_exclusive) = self.read_gate.clone().try_write_owned() else {
            return Ok(ReclamationReport {
                readers_active: true,
                ..Default::default()
            });
        };
        let readers = self
            .query_readers
            .lock()
            .map_err(|_| {
                Error::new(
                    nous_core::ErrorOwner::Resource,
                    nous_core::ErrorCode::InternalFailure,
                    "serving reader registry unavailable",
                )
            })?
            .iter()
            .filter_map(std::sync::Weak::upgrade)
            .collect::<Vec<_>>();
        let mut tx = self.store.begin().await?;
        let actual_epoch: i64 = sqlx::query_scalar(
            "SELECT serving_purge_epoch FROM subjects WHERE subject_id=$1 FOR SHARE",
        )
        .bind(subject.0)
        .fetch_one(&mut *tx)
        .await
        .map_err(nous_persistence::database_error)?;
        if actual_epoch < current_epoch {
            return Err(Error::new(
                nous_core::ErrorOwner::Resource,
                nous_core::ErrorCode::FailedPrecondition,
                "Serving purge checkpoint is not visible",
            ));
        }
        let current_epoch = actual_epoch;
        let old_ids: Vec<uuid::Uuid> = sqlx::query_scalar(
            "SELECT generation_id FROM serving_generations WHERE subject_id=$1 AND purge_epoch<$2 AND artifact_location<>''",
        )
        .bind(subject.0)
        .bind(current_epoch)
        .fetch_all(&mut *tx)
        .await
        .map_err(nous_persistence::database_error)?;

        sqlx::query("UPDATE serving_generations SET state='retired',published_at=COALESCE(published_at,$3),metadata=metadata-'research_pinned' WHERE subject_id=$1 AND purge_epoch<$2 AND state IN ('ready','retired','failed')")
            .bind(subject.0).bind(current_epoch).bind(chrono::Utc::now()).execute(&mut *tx).await.map_err(nous_persistence::database_error)?;

        // Remove process-owned references before touching bytes. Active readers
        // retain their own Arc snapshot and are checked below.
        let retained: Vec<uuid::Uuid> = sqlx::query_scalar("SELECT generation_id FROM serving_generations WHERE subject_id=$1 AND purge_epoch>=$2 AND state='ready' AND view_descriptor->>'kind'='current'")
            .bind(subject.0).bind(current_epoch).fetch_all(&mut *tx).await.map_err(nous_persistence::database_error)?;
        self.publisher.update_for(subject, |snapshot| {
            snapshot.retain_generations(retained.iter().copied().map(ServingGenerationId));
        });

        let mut report = ReclamationReport::default();
        for id in old_ids {
            let id = ServingGenerationId(id);
            if readers
                .iter()
                .any(|reader| reader.snapshot.contains_generation(id))
            {
                report.readers_active = true;
                continue;
            }
            let row = sqlx::query("SELECT artifact_location FROM serving_generations WHERE generation_id=$1 AND subject_id=$2 AND purge_epoch<$3 FOR UPDATE")
                .bind(id.0).bind(subject.0).bind(current_epoch).fetch_optional(&mut *tx).await.map_err(nous_persistence::database_error)?;
            let Some(row) = row else { continue };
            let location: String = row
                .try_get("artifact_location")
                .map_err(nous_persistence::database_error)?;
            if location.is_empty() {
                continue;
            }
            let path = PathBuf::from(&location);
            if path != self.options.root.join(id.0.to_string()) {
                return Err(Error::new(
                    nous_core::ErrorOwner::Resource,
                    nous_core::ErrorCode::FailedPrecondition,
                    "Serving purge artifact is outside its owned root",
                ));
            }
            if path.exists() {
                report.bytes_reclaimed += directory_bytes(&path)?;
                std::fs::remove_dir_all(&path).map_err(io)?;
            }
            sqlx::query("UPDATE serving_generations SET state='failed',artifact_location='',metadata=jsonb_build_object('reclaimed_at',$2::text,'artifact_hash',artifact_hash) WHERE generation_id=$1")
                .bind(id.0).bind(chrono::Utc::now().to_rfc3339()).execute(&mut *tx).await.map_err(nous_persistence::database_error)?;
            report.reclaimed.push(id);
        }
        tx.commit()
            .await
            .map_err(nous_persistence::database_error)?;
        Ok(report)
    }

    /// A reader includes the complete query and any retained validation ticket.
    /// A busy reader defers collection instead of waiting on an external call.
    pub async fn reclaim_retired(
        &self,
        subject: SubjectId,
        grace: std::time::Duration,
    ) -> Result<ReclamationReport> {
        let Ok(_exclusive) = self.read_gate.clone().try_write_owned() else {
            return Ok(ReclamationReport {
                readers_active: true,
                ..Default::default()
            });
        };
        let readers = self
            .query_readers
            .lock()
            .map_err(|_| {
                Error::new(
                    nous_core::ErrorOwner::Resource,
                    nous_core::ErrorCode::InternalFailure,
                    "serving reader registry unavailable",
                )
            })?
            .iter()
            .filter_map(std::sync::Weak::upgrade)
            .collect::<Vec<_>>();
        let cutoff = chrono::Utc::now()
            - chrono::Duration::from_std(grace).map_err(|error| {
                Error::new(
                    nous_core::ErrorOwner::Resource,
                    nous_core::ErrorCode::InvalidRequest,
                    "Invalid request",
                )
                .with_source(error)
            })?;
        let mut tx = self.store.begin().await?;
        // Superseded physical families leave the active catalog once. Their
        // artifacts retain the same reader, research pin and grace protection.
        let supported = super::family::AssetFamily::ALL.map(super::family::AssetFamily::as_str);
        sqlx::query("UPDATE serving_generations SET state='retired',published_at=$3 WHERE subject_id=$1 AND family<>ALL($2::text[]) AND state='ready'")
            .bind(subject.0).bind(supported).bind(chrono::Utc::now()).execute(&mut *tx).await.map_err(nous_persistence::database_error)?;
        let rows = sqlx::query("SELECT generation_id,artifact_location FROM serving_generations WHERE subject_id=$1 AND state IN ('retired','failed') AND artifact_location<>'' AND published_at<$2 AND COALESCE(metadata->>'research_pinned','false')<>'true' FOR UPDATE")
            .bind(subject.0).bind(cutoff).fetch_all(&mut *tx).await.map_err(nous_persistence::database_error)?;
        let mut report = ReclamationReport {
            readers_active: !readers.is_empty(),
            ..Default::default()
        };
        for row in rows {
            let id = ServingGenerationId(
                row.try_get("generation_id")
                    .map_err(nous_persistence::database_error)?,
            );
            if readers
                .iter()
                .any(|reader| reader.snapshot.contains_generation(id))
            {
                report.readers_active = true;
                continue;
            }
            let location: String = row
                .try_get("artifact_location")
                .map_err(nous_persistence::database_error)?;
            let path = PathBuf::from(location);
            // Only generation directories owned by this Serving root may be collected.
            if path != self.options.root.join(id.0.to_string()) {
                continue;
            }
            if path.exists() {
                report.bytes_reclaimed += directory_bytes(&path)?;
                std::fs::remove_dir_all(&path).map_err(io)?;
            }
            // Keep a compact audit row; remove checksums and algorithm metadata.
            sqlx::query("UPDATE serving_generations SET state='failed',artifact_location='',metadata=jsonb_build_object('reclaimed_at',$2::text,'artifact_hash',artifact_hash) WHERE generation_id=$1")
                .bind(id.0).bind(chrono::Utc::now().to_rfc3339()).execute(&mut *tx).await.map_err(nous_persistence::database_error)?;
            report.reclaimed.push(id);
        }
        let known: Vec<String> = sqlx::query_scalar(
            "SELECT artifact_location FROM serving_generations WHERE artifact_location<>''",
        )
        .fetch_all(&mut *tx)
        .await
        .map_err(nous_persistence::database_error)?;
        for entry in std::fs::read_dir(&self.options.root).map_err(io)? {
            let entry = entry.map_err(io)?;
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            let owned = name.starts_with(".staging-") || name.parse::<uuid::Uuid>().is_ok();
            if !owned
                || !entry.file_type().map_err(io)?.is_dir()
                || known
                    .iter()
                    .any(|location| std::path::Path::new(location) == path)
                || entry
                    .metadata()
                    .map_err(io)?
                    .modified()
                    .map_err(io)?
                    .elapsed()
                    .unwrap_or_default()
                    < grace
            {
                continue;
            }
            report.bytes_reclaimed += directory_bytes(&path)?;
            std::fs::remove_dir_all(path).map_err(io)?;
            report.orphan_directories += 1;
        }
        tx.commit()
            .await
            .map_err(nous_persistence::database_error)?;
        Ok(report)
    }

    pub async fn pin_research_generation(
        &self,
        id: ServingGenerationId,
        pinned: bool,
    ) -> Result<()> {
        let _reader = self.read_gate.clone().read_owned().await;
        let changed = sqlx::query("UPDATE serving_generations g SET metadata=jsonb_set(g.metadata,'{research_pinned}',$2) FROM subjects s WHERE g.generation_id=$1 AND g.subject_id=s.subject_id AND g.purge_epoch=s.serving_purge_epoch AND g.state IN ('ready','retired')")
            .bind(id.0).bind(serde_json::json!(pinned)).execute(self.store.pool()).await.map_err(nous_persistence::database_error)?;
        if changed.rows_affected() == 0 {
            return Err(Error::new(
                nous_core::ErrorOwner::Resource,
                nous_core::ErrorCode::NotFound,
                "reusable generation not found",
            ));
        }
        Ok(())
    }
}

fn directory_bytes(path: &std::path::Path) -> Result<u64> {
    let mut bytes = 0;
    for entry in std::fs::read_dir(path).map_err(io)? {
        let entry = entry.map_err(io)?;
        let kind = entry.file_type().map_err(io)?;
        if kind.is_symlink() {
            return Err(Error::new(
                nous_core::ErrorOwner::Resource,
                nous_core::ErrorCode::InternalFailure,
                "Serving artifacts cannot contain symbolic links",
            ));
        }
        bytes += if kind.is_dir() {
            directory_bytes(&entry.path())?
        } else {
            entry.metadata().map_err(io)?.len()
        };
    }
    Ok(bytes)
}
