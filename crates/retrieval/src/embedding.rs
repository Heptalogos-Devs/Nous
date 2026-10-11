//! Bounded model-material discovery and fenced point commits.
// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
use crate::*;
use nous_persistence::{DenseInvalidation, ProjectionInvalidation, database_error as db};
use sqlx::Row;
use std::collections::HashSet;

#[derive(Debug, Clone)]
pub struct EmbeddingNeed {
    pub references: Vec<CognitiveRef>,
    pub text: String,
    pub digest: String,
}
#[derive(Debug, Clone)]
pub struct EmbeddingCursor {
    pub subject: SubjectId,
    pub content_revision: i64,
    pub view_digest: String,
    pub after: CognitiveRef,
}
pub struct EmbeddingPage {
    pub needs: Vec<EmbeddingNeed>,
    pub next_cursor: Option<EmbeddingCursor>,
}

#[derive(Debug, Clone, Copy, Default, Serialize)]
pub struct LegacyEmbeddingUpgradeReport {
    pub legacy_rows_examined: u64,
    pub references_examined: u64,
    pub recovered_rows: u64,
    pub backed_up_rows: u64,
}

impl ServingService {
    /// Reconstruct exact source ownership for pre-binding vectors without
    /// invoking an embedding provider. Rows with no readable current or
    /// historical source are preserved in the isolated legacy backup table
    /// and removed from the active cache.
    pub async fn migrate_legacy_embedding_uses(&self) -> Result<LegacyEmbeddingUpgradeReport> {
        let mut report = LegacyEmbeddingUpgradeReport::default();
        let mut after_subject = None;
        loop {
            let subjects = self
                .store
                .legacy_embedding_subject_page(after_subject, 256)
                .await?;
            let Some(last_subject) = subjects.last().copied() else {
                break;
            };
            for subject in &subjects {
                self.migrate_legacy_embedding_subject(*subject, &mut report)
                    .await?;
            }
            after_subject = Some(last_subject);
        }
        Ok(report)
    }

    async fn migrate_legacy_embedding_subject(
        &self,
        subject: SubjectId,
        report: &mut LegacyEmbeddingUpgradeReport,
    ) -> Result<()> {
        let pending = self.store.unbound_embedding_material_count(subject).await?;
        if pending == 0 {
            return Ok(());
        }
        report.legacy_rows_examined += pending as u64;
        let mut tx = self.store.begin().await?;
        let status: String =
            sqlx::query_scalar("SELECT status FROM subjects WHERE subject_id=$1 FOR UPDATE")
                .bind(subject.0)
                .fetch_optional(&mut *tx)
                .await
                .map_err(db)?
                .ok_or_else(|| {
                    Error::new(
                        nous_core::ErrorOwner::Subject,
                        nous_core::ErrorCode::NotFound,
                        "subject not found",
                    )
                })?;
        // The lock makes the complete source scan, ownership binding and
        // unknown-row backup atomic per Subject. All page buffers remain
        // bounded; interruption cannot leave an unresumable partial binding.
        if status == "active" {
            let memory_enabled = self.projection_capabilities(subject).await?.memory;
            self.bind_legacy_historical_documents(subject, memory_enabled, &mut tx, report)
                .await?;

            // The current projection can differ from an as-of view if
            // persisted event timestamps are in the future.
            report.references_examined += self
                .bind_legacy_document_pages(subject, memory_enabled, None, &mut tx)
                .await? as u64;
        }

        let backed_up =
            AuthorityStore::back_up_unbound_embedding_materials_in(&mut tx, subject).await?;
        report.backed_up_rows += backed_up as u64;
        report.recovered_rows += (pending - backed_up).max(0) as u64;
        tx.commit().await.map_err(db)
    }

    async fn bind_legacy_historical_documents(
        &self,
        subject: SubjectId,
        memory_enabled: bool,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        report: &mut LegacyEmbeddingUpgradeReport,
    ) -> Result<()> {
        let mut after_instant = None;
        loop {
            let page = self
                .store
                .embedding_document_change_page(subject, after_instant, 256)
                .await?;
            let Some(last_instant) = page.instants.last().copied() else {
                break;
            };
            for instant in &page.instants {
                let view = self
                    .legacy_historical_view(subject, *instant, memory_enabled)
                    .await?;
                report.references_examined += self
                    .bind_legacy_document_pages(subject, memory_enabled, Some(&view), tx)
                    .await? as u64;
            }
            after_instant = Some(last_instant);
        }
        Ok(())
    }

    async fn legacy_historical_view(
        &self,
        subject: SubjectId,
        at: chrono::DateTime<chrono::Utc>,
        memory_enabled: bool,
    ) -> Result<HistoricalAuthoritySnapshot> {
        let mut view = if memory_enabled {
            if let Some(memory) = &self.memory {
                memory
                    .project_as_of(subject, at, RevisionView::History)
                    .await?
            } else {
                HistoricalAuthoritySnapshot::empty(subject, at, RevisionView::History)
            }
        } else {
            HistoricalAuthoritySnapshot::empty(subject, at, RevisionView::History)
        };
        let material = self.material.project_as_of(subject, at).await?;
        view.material_visibility = material.known_references;
        view.material_documents = material.document_references;
        view.entity_bindings = material.entity_bindings;
        view.lexical_visibility.extend(material.lexical_visibility);
        view.lexical_visibility
            .sort_by(|left, right| left.lexical_ref.cmp(&right.lexical_ref));
        view.lexical_visibility
            .dedup_by(|left, right| left.lexical_ref == right.lexical_ref);
        view.refresh_digest()?;
        Ok(view)
    }

    async fn bind_legacy_document_pages(
        &self,
        subject: SubjectId,
        memory_enabled: bool,
        view: Option<&HistoricalAuthoritySnapshot>,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    ) -> Result<usize> {
        let mut after = None;
        let mut examined = 0;
        loop {
            let mut references = self
                .store
                .text_projection_reference_page(subject, memory_enabled, after.as_ref(), 257, view)
                .await?;
            let has_more = references.len() > 256;
            references.truncate(256);
            if references.is_empty() {
                break;
            }
            let documents = self.documents_lookup(subject, &references, view).await?;
            examined += documents.len();
            if !documents.is_empty() {
                let requested = documents
                    .iter()
                    .map(|document| {
                        let (kind, value) = reference_parts(&document.reference);
                        serde_json::json!({
                            "digest": nous_material::text_content_identity(&document.representation_text),
                            "ref_kind": kind,
                            "ref_value": value,
                        })
                    })
                    .collect::<Vec<_>>();
                let source_references = documents
                    .iter()
                    .map(|document| document.reference.clone())
                    .collect::<Vec<_>>();
                AuthorityStore::validate_not_purging_refs_in(tx, subject, &source_references)
                    .await?;
                sqlx::query(
                    r#"
WITH requested AS (
    SELECT r.digest,r.ref_kind,r.ref_value
    FROM jsonb_array_elements($2) AS item(value)
    CROSS JOIN LATERAL jsonb_to_record(item.value) AS r(digest text,ref_kind text,ref_value text)
)
INSERT INTO embedding_material_uses(subject_id,content_digest,space_hash,producer_hash,ref_kind,ref_value)
SELECT m.subject_id,m.content_digest,m.space_hash,m.producer_hash,r.ref_kind,r.ref_value
FROM requested r
JOIN embedding_materials m ON m.subject_id=$1 AND m.content_digest=r.digest
ON CONFLICT DO NOTHING
"#,
                )
                .bind(subject.0)
                .bind(serde_json::json!(requested))
                .execute(&mut **tx)
                .await
                .map_err(db)?;
            }
            if !has_more {
                break;
            }
            after = references.last().cloned();
        }
        Ok(examined)
    }

    pub async fn embedding_needs_page(
        &self,
        subject: SubjectId,
        limit: usize,
        cursor: Option<&EmbeddingCursor>,
        view: Option<&HistoricalAuthoritySnapshot>,
    ) -> Result<EmbeddingPage> {
        if limit == 0 || limit > 256 || view.is_some_and(|view| view.subject != subject) {
            return Err(Error::new(
                nous_core::ErrorOwner::Resource,
                nous_core::ErrorCode::InvalidRequest,
                "invalid embedding candidate page",
            ));
        }
        self.store.require_subject(subject).await?;
        let binding_stamp = self.store.read_context_stamp(subject).await?;
        let provider = self.embedding().ok_or_else(|| {
            Error::new(
                nous_core::ErrorOwner::Resource,
                nous_core::ErrorCode::Unavailable,
                "embedding space not configured",
            )
        })?;
        let space = provider.space();
        let producers: Vec<_> = provider
            .producers()
            .into_iter()
            .map(|producer| producer.signature_hash)
            .collect();
        let memory_enabled = self.projection_capabilities(subject).await?.memory;
        let content_revision = self.document_revision(subject, view).await?;
        let view_digest = view
            .map(|view| view.snapshot_digest.clone())
            .unwrap_or_default();
        if cursor.is_some_and(|cursor| {
            cursor.subject != subject
                || cursor.content_revision != content_revision
                || cursor.view_digest != view_digest
        }) {
            return Err(nous_core::Error::new(
                nous_core::ErrorOwner::Resource,
                nous_core::ErrorCode::StaleRevision,
                "embedding discovery corpus changed",
            ));
        }
        let mut references = self
            .store
            .text_projection_reference_page(
                subject,
                memory_enabled,
                cursor.map(|cursor| &cursor.after),
                limit + 1,
                view,
            )
            .await?;
        let has_more = references.len() > limit;
        references.truncate(limit);
        let next_cursor = if has_more {
            references.last().cloned().map(|after| EmbeddingCursor {
                subject,
                content_revision,
                view_digest,
                after,
            })
        } else {
            None
        };
        let documents = self.documents_lookup(subject, &references, view).await?;
        if self.document_revision(subject, view).await? != content_revision {
            return Err(nous_core::Error::new(
                nous_core::ErrorOwner::Resource,
                nous_core::ErrorCode::StaleRevision,
                "embedding discovery corpus changed",
            ));
        }
        let mut by_digest = std::collections::BTreeMap::<String, EmbeddingNeed>::new();
        for document in documents {
            let digest = nous_material::text_content_identity(&document.representation_text);
            by_digest
                .entry(digest.clone())
                .and_modify(|need| need.references.push(document.reference.clone()))
                .or_insert_with(|| EmbeddingNeed {
                    references: vec![document.reference],
                    text: document.representation_text,
                    digest,
                });
        }
        let mut candidates: Vec<_> = by_digest.into_values().collect();
        for need in &mut candidates {
            need.references.sort_by_key(reference_parts);
            need.references.dedup();
        }
        let digests: Vec<_> = candidates.iter().map(|need| need.digest.clone()).collect();
        // One anti-join for the entire bounded page; no per-document cache queries.
        let missing: HashSet<String> = sqlx::query_scalar(
            r#"
SELECT digest FROM unnest($2::text[]) input(digest)
 WHERE NOT EXISTS(
     SELECT 1 FROM embedding_materials cached
     WHERE cached.subject_id=$1 AND cached.content_digest=input.digest
       AND cached.space_hash=$3 AND cached.producer_hash=ANY($4::text[])
       AND EXISTS (
           SELECT 1 FROM embedding_material_uses existing
           WHERE existing.subject_id=cached.subject_id
             AND existing.content_digest=cached.content_digest
             AND existing.space_hash=cached.space_hash
             AND existing.producer_hash=cached.producer_hash
       )
 )
"#,
        )
        .bind(subject.0)
        .bind(digests)
        .bind(&space.space_hash)
        .bind(producers.clone())
        .fetch_all(self.store.pool())
        .await
        .map_err(db)?
        .into_iter()
        .collect();
        let mut tx = self.store.begin().await?;
        let current = sqlx::query(
            "SELECT authority_seq,serving_purge_epoch FROM subjects WHERE subject_id=$1 FOR UPDATE",
        )
        .bind(subject.0)
        .fetch_one(&mut *tx)
        .await
        .map_err(db)?;
        if current.try_get::<i64, _>("authority_seq").map_err(db)? != binding_stamp.authority_seq
            || current
                .try_get::<i64, _>("serving_purge_epoch")
                .map_err(db)?
                != binding_stamp.serving_purge_epoch
        {
            return Err(nous_core::Error::new(
                nous_core::ErrorOwner::Runtime,
                nous_core::ErrorCode::StaleRevision,
                "embedding source changed during discovery",
            ));
        }
        let requested = candidates
            .iter()
            .flat_map(|need| {
                need.references.iter().map(|reference| {
                    let (kind, value) = reference_parts(reference);
                    serde_json::json!({
                        "digest": need.digest,
                        "ref_kind": kind,
                        "ref_value": value,
                    })
                })
            })
            .collect::<Vec<_>>();
        let source_references = candidates
            .iter()
            .flat_map(|need| need.references.iter().cloned())
            .collect::<Vec<_>>();
        AuthorityStore::validate_not_purging_refs_in(&mut tx, subject, &source_references).await?;
        sqlx::query(r#"
WITH requested AS (
    SELECT r.digest,r.ref_kind,r.ref_value
    FROM jsonb_array_elements($2) AS item(value)
    CROSS JOIN LATERAL jsonb_to_record(item.value) AS r(digest text,ref_kind text,ref_value text)
)
INSERT INTO embedding_material_uses(subject_id,content_digest,space_hash,producer_hash,ref_kind,ref_value)
SELECT $1,m.content_digest,m.space_hash,m.producer_hash,r.ref_kind,r.ref_value
FROM requested r
JOIN embedding_materials m ON m.subject_id=$1 AND m.content_digest=r.digest
    AND m.space_hash=$3 AND m.producer_hash=ANY($4::text[])
WHERE EXISTS (
    SELECT 1 FROM embedding_material_uses existing
    WHERE existing.subject_id=m.subject_id
      AND existing.content_digest=m.content_digest
      AND existing.space_hash=m.space_hash
      AND existing.producer_hash=m.producer_hash
)
ON CONFLICT DO NOTHING
"#)
        .bind(subject.0)
        .bind(serde_json::json!(requested))
        .bind(&space.space_hash)
        .bind(&producers)
        .execute(&mut *tx)
        .await
        .map_err(db)?;
        tx.commit().await.map_err(db)?;
        Ok(EmbeddingPage {
            needs: candidates
                .into_iter()
                .filter(|need| missing.contains(&need.digest))
                .collect(),
            next_cursor,
        })
    }

    async fn document_revision(
        &self,
        subject: SubjectId,
        view: Option<&HistoricalAuthoritySnapshot>,
    ) -> Result<i64> {
        if view.is_some() {
            Ok(0)
        } else {
            self.store
                .projection_watermark(subject, "lexical", "")
                .await
        }
    }

    pub async fn commit_embedding(
        &self,
        subject: SubjectId,
        references: Vec<CognitiveRef>,
        text: String,
        space: &str,
        producer: &str,
        vector: Vec<f32>,
    ) -> Result<()> {
        self.commit_embedding_in_view(subject, references, text, space, producer, vector, None)
            .await
    }

    pub async fn commit_embedding_in_view(
        &self,
        subject: SubjectId,
        mut references: Vec<CognitiveRef>,
        text: String,
        space: &str,
        producer: &str,
        vector: Vec<f32>,
        view: Option<&HistoricalAuthoritySnapshot>,
    ) -> Result<()> {
        if references.is_empty() || references.len() > 512 {
            return Err(Error::new(
                nous_core::ErrorOwner::Runtime,
                nous_core::ErrorCode::InvalidRequest,
                "embedding commit requires 1 to 512 source refs",
            ));
        }
        references.sort_by_key(reference_parts);
        references.dedup();
        for reference in &references {
            self.store.validate_reference(subject, reference).await?;
        }
        let stamp = self.store.read_context_stamp(subject).await?;
        let configured = self.embedding().ok_or_else(|| {
            Error::new(
                nous_core::ErrorOwner::Resource,
                nous_core::ErrorCode::Unavailable,
                "embedding not configured",
            )
        })?;
        if configured.space().space_hash != space
            || !configured
                .producers()
                .iter()
                .any(|p| p.signature_hash == producer)
            || vector.len() != configured.space().dimension as usize
            || vector.iter().any(|v| !v.is_finite())
        {
            return Err(Error::new(
                nous_core::ErrorOwner::Resource,
                nous_core::ErrorCode::InvalidRequest,
                "Invalid request",
            ));
        }
        let digest = nous_material::text_content_identity(&text);
        let document = self
            .documents_lookup(subject, &references, view)
            .await?
            .into_iter()
            .filter(|document| {
                nous_material::text_content_identity(&document.representation_text) == digest
            })
            .map(|document| document.reference)
            .collect::<HashSet<_>>();
        if references
            .iter()
            .any(|reference| !document.contains(reference))
        {
            return Err(nous_core::Error::new(
                nous_core::ErrorOwner::Runtime,
                nous_core::ErrorCode::StaleRevision,
                "embedding source changed",
            ));
        }
        let mut tx = self.store.begin().await?;
        // Authority writers advance this row in their transaction. Hold it through cache publication.
        let current = sqlx::query(
            "SELECT authority_seq,serving_purge_epoch FROM subjects WHERE subject_id=$1 FOR UPDATE",
        )
        .bind(subject.0)
        .fetch_one(&mut *tx)
        .await
        .map_err(db)?;
        if current.try_get::<i64, _>("authority_seq").map_err(db)? != stamp.authority_seq
            || current
                .try_get::<i64, _>("serving_purge_epoch")
                .map_err(db)?
                != stamp.serving_purge_epoch
        {
            return Err(nous_core::Error::new(
                nous_core::ErrorOwner::Runtime,
                nous_core::ErrorCode::StaleRevision,
                "Authority or Serving purge epoch changed during embedding validation",
            ));
        }
        let unbound_legacy: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM embedding_materials m WHERE m.subject_id=$1 AND m.content_digest=$2 AND m.space_hash=$3 AND m.producer_hash=$4 AND NOT EXISTS(SELECT 1 FROM embedding_material_uses u WHERE u.subject_id=m.subject_id AND u.content_digest=m.content_digest AND u.space_hash=m.space_hash AND u.producer_hash=m.producer_hash))",
        )
        .bind(subject.0)
        .bind(&digest)
        .bind(space)
        .bind(producer)
        .fetch_one(&mut *tx)
        .await
        .map_err(db)?;
        if unbound_legacy {
            return Err(Error::new(
                nous_core::ErrorOwner::Persistence,
                nous_core::ErrorCode::FailedPrecondition,
                "Legacy embedding ownership requires the explicit copied-database upgrade",
            ));
        }
        AuthorityStore::validate_not_purging_refs_in(&mut tx, subject, &references).await?;
        let inserted = sqlx::query("INSERT INTO embedding_materials(subject_id,content_digest,space_hash,producer_hash,vector) VALUES($1,$2,$3,$4,$5) ON CONFLICT DO NOTHING")
            .bind(subject.0).bind(&digest).bind(space).bind(producer).bind(vector).execute(&mut *tx).await.map_err(db)?;
        let (kinds, values): (Vec<_>, Vec<_>) = references.iter().map(reference_parts).unzip();
        sqlx::query("INSERT INTO embedding_material_uses(subject_id,content_digest,space_hash,producer_hash,ref_kind,ref_value) SELECT $1,$2,$3,$4,refs.ref_kind,refs.ref_value FROM unnest($5::text[],$6::text[]) AS refs(ref_kind,ref_value) ON CONFLICT DO NOTHING")
            .bind(subject.0).bind(&digest).bind(space).bind(producer).bind(kinds).bind(values)
            .execute(&mut *tx).await.map_err(db)?;
        if inserted.rows_affected() > 0 {
            AuthorityStore::invalidate_in(
                &mut tx,
                subject,
                ProjectionInvalidation {
                    dense: DenseInvalidation::Spaces(vec![space.into()]),
                    ..Default::default()
                },
            )
            .await?;
        }
        tx.commit().await.map_err(db)
    }
}
