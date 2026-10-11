//! Bounded model-material discovery and fenced point commits.
// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
use crate::*;
use nous_persistence::{DenseInvalidation, ProjectionInvalidation, database_error as db};
use std::collections::HashSet;

#[derive(Debug, Clone)]
pub struct EmbeddingNeed {
    pub reference: CognitiveRef,
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
            return Err(Error::Invalid("invalid embedding candidate page".into()));
        }
        self.store.require_subject(subject).await?;
        let provider = self
            .embedding()
            .ok_or_else(|| Error::Unavailable("embedding space not configured".into()))?;
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
            return Err(nous_core::DomainError::new(
                nous_core::DomainErrorCode::StaleRevision,
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
            return Err(nous_core::DomainError::new(
                nous_core::DomainErrorCode::StaleRevision,
                "embedding discovery corpus changed",
            )
            .into());
        }
        let mut seen = HashSet::new();
        let candidates: Vec<_> = documents
            .into_iter()
            .filter_map(|document| {
                let digest = nous_material::text_content_identity(&document.representation_text);
                seen.insert(digest.clone()).then_some(EmbeddingNeed {
                    reference: document.reference,
                    text: document.representation_text,
                    digest,
                })
            })
            .collect();
        let digests: Vec<_> = candidates.iter().map(|need| need.digest.clone()).collect();
        // One anti-join for the entire bounded page; no per-document cache queries.
        let missing: HashSet<String> = sqlx::query_scalar(r#"
SELECT digest FROM unnest($2::text[]) input(digest)
 WHERE NOT EXISTS(SELECT 1 FROM embedding_materials cached WHERE cached.subject_id=$1 AND cached.content_digest=input.digest AND cached.space_hash=$3 AND cached.producer_hash=ANY($4::text[]))
"#).bind(subject.0).bind(digests).bind(&space.space_hash).bind(producers).fetch_all(self.store.pool()).await.map_err(db)?.into_iter().collect();
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
        reference: CognitiveRef,
        text: String,
        space: &str,
        producer: &str,
        vector: Vec<f32>,
    ) -> Result<()> {
        self.commit_embedding_in_view(subject, reference, text, space, producer, vector, None)
            .await
    }

    pub async fn commit_embedding_in_view(
        &self,
        subject: SubjectId,
        reference: CognitiveRef,
        text: String,
        space: &str,
        producer: &str,
        vector: Vec<f32>,
        view: Option<&HistoricalAuthoritySnapshot>,
    ) -> Result<()> {
        self.store.validate_reference(subject, &reference).await?;
        let configured = self
            .embedding()
            .ok_or_else(|| Error::Unavailable("embedding not configured".into()))?;
        if configured.space().space_hash != space
            || !configured
                .producers()
                .iter()
                .any(|p| p.signature_hash == producer)
            || vector.len() != configured.space().dimension as usize
            || vector.iter().any(|v| !v.is_finite())
        {
            return Err(Error::Invalid(
                "embedding material disagrees with configured space/producer".into(),
            ));
        }
        let revision = self.store.authority_seq(subject).await?;
        let digest = nous_material::text_content_identity(&text);
        let document = self
            .documents_lookup(subject, std::slice::from_ref(&reference), view)
            .await?
            .pop();
        if document.is_none_or(|document| {
            document.reference != reference
                || nous_material::text_content_identity(&document.representation_text) != digest
        }) {
            return Err(nous_core::DomainError::new(
                nous_core::DomainErrorCode::StaleRevision,
                "embedding source changed",
            )
            .into());
        }
        let mut tx = self.store.begin().await?;
        // Authority writers advance this row in their transaction. Hold it through cache publication.
        let current: i64 =
            sqlx::query_scalar("SELECT authority_seq FROM subjects WHERE subject_id=$1 FOR UPDATE")
                .bind(subject.0)
                .fetch_one(&mut *tx)
                .await
                .map_err(db)?;
        if current != revision {
            return Err(nous_core::DomainError::new(
                nous_core::DomainErrorCode::StaleRevision,
                "Authority changed during embedding validation",
            )
            .with_context("expected_authority_seq", revision)
            .with_context("actual_authority_seq", current)
            .into());
        }
        let inserted = sqlx::query("INSERT INTO embedding_materials(subject_id,content_digest,space_hash,producer_hash,vector) VALUES($1,$2,$3,$4,$5) ON CONFLICT DO NOTHING")
            .bind(subject.0).bind(digest).bind(space).bind(producer).bind(vector).execute(&mut *tx).await.map_err(db)?;
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
