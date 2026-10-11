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

impl ServingService {
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
            )
            .into());
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
            )
            .into());
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
        let missing: HashSet<String> = sqlx::query_scalar(r#"
SELECT digest FROM unnest($2::text[]) input(digest)
 WHERE NOT EXISTS(SELECT 1 FROM embedding_materials cached WHERE cached.subject_id=$1 AND cached.content_digest=input.digest AND cached.space_hash=$3 AND cached.producer_hash=ANY($4::text[]))
"#).bind(subject.0).bind(digests).bind(&space.space_hash).bind(producers.clone()).fetch_all(self.store.pool()).await.map_err(db)?.into_iter().collect();
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
            )
            .into());
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
            )
            .into());
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
            )
            .into());
        }
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
