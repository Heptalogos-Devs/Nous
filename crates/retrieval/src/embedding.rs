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

pub struct EmbeddingSubmission {
    pub reference: CognitiveRef,
    pub text: String,
    pub space: String,
    pub producer: String,
    pub vector: Vec<f32>,
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

    /// Admit one completed Host batch. Source validation and cache publication
    /// share one Authority fence; prior committed batches remain independent.
    pub async fn commit_embeddings(
        &self,
        subject: SubjectId,
        items: Vec<EmbeddingSubmission>,
        view: Option<&HistoricalAuthoritySnapshot>,
    ) -> Result<u32> {
        if items.is_empty() || items.len() > 256 || view.is_some_and(|view| view.subject != subject)
        {
            return Err(Error::Invalid(
                "embedding batch must be 1..256 for this Subject".into(),
            ));
        }
        let configured = self
            .embedding()
            .ok_or_else(|| Error::Unavailable("embedding not configured".into()))?;
        let space = configured.space();
        let producers: HashSet<_> = configured
            .producers()
            .into_iter()
            .map(|producer| producer.signature_hash)
            .collect();
        for item in &items {
            if item.space != space.space_hash
                || !producers.contains(&item.producer)
                || item.vector.len() != space.dimension as usize
                || item.vector.iter().any(|v| !v.is_finite())
            {
                return Err(Error::Invalid(
                    "embedding material disagrees with configured space/producer".into(),
                ));
            }
        }
        let revision = self.store.authority_seq(subject).await?;
        let references: Vec<_> = items.iter().map(|item| item.reference.clone()).collect();
        let documents: std::collections::HashMap<_, _> = self
            .documents_lookup(subject, &references, view)
            .await?
            .into_iter()
            .map(|document| {
                (
                    document.reference,
                    nous_material::text_content_identity(&document.representation_text),
                )
            })
            .collect();
        for item in &items {
            if documents.get(&item.reference)
                != Some(&nous_material::text_content_identity(&item.text))
            {
                return Err(nous_core::DomainError::new(
                    nous_core::DomainErrorCode::StaleRevision,
                    "embedding source changed",
                )
                .into());
            }
        }
        let mut tx = self.store.begin().await?;
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
        let accepted = items.len() as u32;
        let mut insert = sqlx::QueryBuilder::<sqlx::Postgres>::new(
            "INSERT INTO embedding_materials(subject_id,content_digest,space_hash,producer_hash,vector) ",
        );
        insert.push_values(items, |mut row, item| {
            row.push_bind(subject.0)
                .push_bind(nous_material::text_content_identity(&item.text))
                .push_bind(item.space)
                .push_bind(item.producer)
                .push_bind(item.vector);
        });
        insert.push(" ON CONFLICT DO NOTHING");
        if insert
            .build()
            .execute(&mut *tx)
            .await
            .map_err(db)?
            .rows_affected()
            > 0
        {
            AuthorityStore::invalidate_in(
                &mut tx,
                subject,
                ProjectionInvalidation {
                    dense: DenseInvalidation::Spaces(vec![space.space_hash]),
                    ..Default::default()
                },
            )
            .await?;
        }
        tx.commit().await.map_err(db)?;
        Ok(accepted)
    }
}
