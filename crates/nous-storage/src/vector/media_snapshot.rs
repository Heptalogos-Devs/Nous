//! Authorized media candidates retain their exact lineage in one Authority view.
use crate::{ReadView, not_found, storage_error};
use nous_domain::{media::*, *};
use serde::{Deserialize, Serialize};
use sqlx::Row;
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaRetrievalEntry {
    pub source: MediaSourceLineage,
    pub observations: Vec<Observation>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaRetrievalSnapshot {
    pub watermark: i64,
    pub entries: Vec<MediaRetrievalEntry>,
    pub complete: bool,
}
impl ReadView {
    pub async fn media_retrieval_snapshot(
        &mut self,
        namespace: &NamespaceId,
        subject: &SubjectId,
        options: &super::QuerySnapshotOptions<'_>,
    ) -> Result<MediaRetrievalSnapshot> {
        let limit = options.limit;
        let as_of_commit = options.as_of_commit;
        let source_read = options.source_read;
        let _request_limit = i64::try_from(limit)
            .ok()
            .filter(|limit| *limit > 0)
            .and_then(|limit| limit.checked_add(1))
            .ok_or_else(|| {
                Error::invalid("media snapshot limit must be positive and fit the authority range")
            })?;
        self.subject(namespace, subject).await?;
        let watermark = cutoff(self.watermark, as_of_commit)?;
        let rows=sqlx::query("SELECT 'original' AS kind,m.id,0 AS revision FROM materials m WHERE m.namespace_id=?1 AND m.commit_seq<=?3 AND (EXISTS(SELECT 1 FROM observations o WHERE o.namespace_id=m.namespace_id AND o.material_id=m.id AND o.subject_id=?2) OR EXISTS(SELECT 1 FROM material_outputs o WHERE o.namespace_id=m.namespace_id AND o.material_id=m.id AND o.subject_id=?2)) UNION ALL SELECT 'source_region',r.id,0 FROM source_regions r WHERE r.namespace_id=?1 AND r.commit_seq<=?3 AND (EXISTS(SELECT 1 FROM observations o WHERE o.namespace_id=r.namespace_id AND o.material_id=r.material_id AND o.subject_id=?2) OR EXISTS(SELECT 1 FROM material_outputs o WHERE o.namespace_id=r.namespace_id AND o.material_id=r.material_id AND o.subject_id=?2)) UNION ALL SELECT 'representation',r.id,r.revision FROM media_representations r WHERE r.namespace_id=?1 AND r.subject_id=?2 AND r.commit_seq<=?3 AND (?4 OR r.revision=(SELECT max(h.revision) FROM media_representations h WHERE h.id=r.id AND h.commit_seq<=?3)) UNION ALL SELECT 'derived_region',r.id,0 FROM derived_regions r WHERE r.namespace_id=?1 AND r.subject_id=?2 AND r.commit_seq<=?3 ORDER BY kind,id")
            .bind(namespace.as_str()).bind(subject.as_str()).bind(watermark).bind(options.history).fetch_all(&mut *self.tx).await.map_err(storage_error)?;
        let mut complete = true;
        let mut entries = Vec::new();
        for row in rows {
            let id: String = row.get("id");
            let reference = match row.get::<&str, _>("kind") {
                "original" => MediaReference::Original {
                    material_id: id.parse()?,
                },
                "source_region" => MediaReference::SourceRegion { id: id.parse()? },
                "representation" => MediaReference::Representation {
                    reference: RepresentationRef {
                        id: id.parse()?,
                        revision: row.get("revision"),
                    },
                },
                "derived_region" => MediaReference::DerivedRegion { id: id.parse()? },
                _ => return Err(Error::internal("invalid media candidate kind")),
            };
            if let Some(view) = source_read
                && !self
                    .source_read_allowed(view, &RetrievalTarget::Media(reference.clone()))
                    .await?
            {
                continue;
            }
            if !crate::source::query_filter::target_matches(
                &mut self.tx,
                namespace,
                subject,
                &RetrievalTarget::Media(reference.clone()),
                options.filters,
                as_of_commit,
            )
            .await?
            {
                continue;
            }
            if entries.len() == limit {
                complete = false;
                break;
            }
            if let Some(entry) = self
                .media_entry(namespace, subject, &reference, watermark)
                .await?
            {
                entries.push(entry);
            }
        }
        Ok(MediaRetrievalSnapshot {
            watermark,
            entries,
            complete,
        })
    }
    pub async fn media_reference_snapshot(
        &mut self,
        namespace: &NamespaceId,
        subject: &SubjectId,
        options: &super::QuerySnapshotOptions<'_>,
        reference: &MediaReference,
    ) -> Result<MediaRetrievalSnapshot> {
        let as_of_commit = options.as_of_commit;
        let source_read = options.source_read;
        self.subject(namespace, subject).await?;
        let watermark = cutoff(self.watermark, as_of_commit)?;
        if let Some(view) = source_read
            && !self
                .source_read_allowed(view, &RetrievalTarget::Media(reference.clone()))
                .await?
        {
            return Err(Error::new(
                ErrorCode::Forbidden,
                "exact media reference is outside the source-read scope",
            ));
        }
        let entry = self
            .media_entry(namespace, subject, reference, watermark)
            .await?
            .ok_or_else(not_found)?;
        Ok(MediaRetrievalSnapshot {
            watermark,
            entries: if crate::source::query_filter::target_matches(
                &mut self.tx,
                namespace,
                subject,
                &RetrievalTarget::Media(reference.clone()),
                options.filters,
                as_of_commit,
            )
            .await?
            {
                vec![entry]
            } else {
                Vec::new()
            },
            complete: true,
        })
    }
    async fn media_entry(
        &mut self,
        namespace: &NamespaceId,
        subject: &SubjectId,
        reference: &MediaReference,
        watermark: i64,
    ) -> Result<Option<MediaRetrievalEntry>> {
        let source =
            crate::media::lineage_in(&mut self.tx, namespace, subject, reference, watermark)
                .await?;
        let sequence = match reference {
            MediaReference::Original { material_id } => source
                .materials
                .iter()
                .find(|row| row.material.id == *material_id)
                .map(|row| row.material.commit_seq),
            MediaReference::SourceRegion { id } => source
                .source_regions
                .iter()
                .find(|row| row.id == *id)
                .map(|row| row.commit_seq),
            MediaReference::Representation { reference } => source
                .representations
                .iter()
                .find(|row| row.reference() == *reference)
                .map(|row| row.commit_seq),
            MediaReference::DerivedRegion { id } => source
                .derived_regions
                .iter()
                .find(|row| row.id == *id)
                .map(|row| row.commit_seq),
        }
        .ok_or_else(not_found)?;
        if sequence > watermark {
            return Ok(None);
        }
        for root in &source.materials {
            if !self
                .subject_has_material_access(namespace, subject, &root.material.id)
                .await?
            {
                return Ok(None);
            }
        }
        let ids = source
            .materials
            .iter()
            .map(|row| row.material.id.as_str())
            .collect::<Vec<_>>();
        let observations=sqlx::query("SELECT * FROM observations WHERE namespace_id=? AND subject_id=? AND material_id IN(SELECT value FROM json_each(?)) AND commit_seq<=? ORDER BY commit_seq,id")
            .bind(namespace.as_str()).bind(subject.as_str()).bind(serde_json::to_string(&ids)?).bind(watermark).fetch_all(&mut *self.tx).await.map_err(storage_error)?.into_iter().map(crate::authority::records::observation_row).collect::<Result<Vec<_>>>()?;
        Ok(Some(MediaRetrievalEntry {
            source,
            observations,
        }))
    }
}
fn cutoff(upper: i64, requested: Option<i64>) -> Result<i64> {
    let cutoff = requested.unwrap_or(upper);
    if cutoff < 0 || cutoff > upper {
        return Err(Error::invalid(
            "media cutoff refers to uncommitted authority",
        ));
    }
    Ok(cutoff)
}
