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
            .map_err(|_| Error::Infrastructure("serving reader registry unavailable".into()))?
            .iter()
            .filter_map(std::sync::Weak::upgrade)
            .collect::<Vec<_>>();
        let cutoff = chrono::Utc::now()
            - chrono::Duration::from_std(grace)
                .map_err(|error| Error::Invalid(error.to_string()))?;
        let mut tx = self.store.begin().await?;
        // Superseded physical families leave the active catalog once. Their
        // artifacts retain the same reader, research pin and grace protection.
        let supported = super::family::AssetFamily::ALL.map(super::family::AssetFamily::as_str);
        sqlx::query("UPDATE serving_generations SET state='retired',published_at=$3 WHERE subject_id=$1 AND family<>ALL($2::text[]) AND state='ready'")
            .bind(subject.0).bind(supported).bind(chrono::Utc::now()).execute(&mut *tx).await.map_err(nous_persistence::database_error)?;
        sqlx::query("DELETE FROM serving_current WHERE subject_id=$1 AND family<>ALL($2::text[])")
            .bind(subject.0)
            .bind(supported)
            .execute(&mut *tx)
            .await
            .map_err(nous_persistence::database_error)?;
        let rows = sqlx::query("SELECT generation_id,artifact_location FROM serving_generations g WHERE subject_id=$1 AND state IN ('retired','failed') AND artifact_location<>'' AND published_at<$2 AND COALESCE(metadata->>'research_pinned','false')<>'true' AND NOT EXISTS (SELECT 1 FROM serving_current c WHERE c.generation_id=g.generation_id) FOR UPDATE")
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
        let changed = sqlx::query("UPDATE serving_generations SET metadata=jsonb_set(metadata,'{research_pinned}',$2) WHERE generation_id=$1 AND state IN ('ready','retired')")
            .bind(id.0).bind(serde_json::json!(pinned)).execute(self.store.pool()).await.map_err(nous_persistence::database_error)?;
        if changed.rows_affected() == 0 {
            return Err(Error::NotFound("reusable generation not found".into()));
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
            return Err(Error::Infrastructure(
                "serving artifact contains a symbolic link".into(),
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
