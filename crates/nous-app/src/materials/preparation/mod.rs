mod adoption;
use crate::App;
use nous_domain::{media::*, *};
use nous_foundation::media::PreparationRequest;
use nous_storage::{BeginOperation, operation_key};
use serde_json::json;
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;

impl App {
    pub async fn embedding_media_parts(
        &self,
        ctx: &CallContext,
        namespace: &NamespaceId,
        subject: &SubjectId,
        source: &MediaReference,
        cancel: &CancellationToken,
    ) -> Result<Vec<EmbeddingPart>> {
        let snapshot = self
            .sources
            .resolve(ctx, namespace, subject, source)
            .await?;
        match &snapshot.payload {
            RepresentationPayload::Text { .. } | RepresentationPayload::Interpretation { .. } => {
                return Ok(vec![EmbeddingPart::Text {
                    text: self.sources.text(&snapshot, cancel).await?,
                    source: Some(source.clone()),
                }]);
            }
            RepresentationPayload::Artifact { blob, metadata }
                if matches!(snapshot.coordinate, SourceCoordinate::Whole)
                    && !matches!(metadata, MediaMetadata::Image { frames: 2.., .. }) =>
            {
                if matches!(metadata, MediaMetadata::Text) {
                    if matches!(source, MediaReference::Original { .. }) {
                        return Ok(vec![EmbeddingPart::Text {
                            text: self.sources.text(&snapshot, cancel).await?,
                            source: Some(source.clone()),
                        }]);
                    }
                } else {
                    return Ok(vec![EmbeddingPart::Media {
                        source: source.clone(),
                        blob: blob.clone(),
                        channels: snapshot.access,
                    }]);
                }
            }
            _ => {}
        }
        let graph = self
            .prepare_material_source(
                ctx,
                namespace,
                subject,
                source,
                MediaDelivery::Native,
                cancel,
            )
            .await?;
        let mut parts = Vec::new();
        for item in graph.inputs {
            if let Some(reference) = item.text_source {
                let reference = MediaReference::Representation { reference };
                let snapshot = self
                    .sources
                    .resolve(ctx, namespace, subject, &reference)
                    .await?;
                parts.push(EmbeddingPart::Text {
                    text: self.sources.text(&snapshot, cancel).await?,
                    source: Some(reference),
                });
                continue;
            }
            let snapshot = self
                .sources
                .resolve(ctx, namespace, subject, &item.source)
                .await?;
            match &snapshot.payload {
                RepresentationPayload::Artifact { blob, metadata }
                    if !matches!(metadata, MediaMetadata::Text) =>
                {
                    if !matches!(snapshot.coordinate, SourceCoordinate::Whole) {
                        return Err(Error::internal(
                            "prepared delivery is not a whole immutable artifact",
                        ));
                    }
                    parts.push(EmbeddingPart::Media {
                        source: item.source,
                        blob: blob.clone(),
                        channels: snapshot.access,
                    });
                }
                _ => parts.push(EmbeddingPart::Text {
                    text: self.sources.text(&snapshot, cancel).await?,
                    source: Some(item.source),
                }),
            }
        }
        Ok(parts)
    }

    pub(crate) async fn prepare_material_source(
        &self,
        ctx: &CallContext,
        namespace: &NamespaceId,
        subject: &SubjectId,
        source: &MediaReference,
        delivery: MediaDelivery,
        cancel: &CancellationToken,
    ) -> Result<MediaPreparedGraph> {
        let snapshot = self
            .sources
            .resolve(ctx, namespace, subject, source)
            .await?;
        let RepresentationPayload::Artifact { blob, metadata } = &snapshot.payload else {
            if delivery != MediaDelivery::Native {
                return Err(Error::invalid(
                    "sampling requires a temporal media artifact",
                ));
            }
            return Ok(MediaPreparedGraph {
                inputs: vec![MediaPreparedInput {
                    source: source.clone(),
                    role: DerivationInputRole::Source,
                    at_ms: None,
                    text_source: None,
                }],
                quality: snapshot.quality.clone(),
                preprocessing_digest: digest(
                    &json!({"source":source,"payload":snapshot.payload,"producer":snapshot.producer}),
                )?,
            });
        };
        let plan = self
            .media
            .freeze_preparation(PreparationRequest {
                blob: blob.clone(),
                metadata: metadata.clone(),
                coordinate: snapshot.coordinate.clone(),
                delivery,
            })
            .await?;
        if let Some(existing) = self
            .database
            .media_preparation(namespace, subject, source, plan.fingerprint())
            .await?
        {
            return Ok(existing.graph);
        }
        let prepared = self.media.prepare_frozen(ctx, plan.clone(), cancel).await?;
        let mut texts = Vec::new();
        for item in &prepared.items {
            texts.push(if matches!(item.metadata, MediaMetadata::Text) {
                Some(self.media.read_text(&item.blob, cancel).await?)
            } else {
                None
            });
        }
        if cancel.is_cancelled() {
            return Err(Error::new(
                ErrorCode::Cancelled,
                "media preparation cancelled",
            ));
        }
        let key = operation_key(
            ctx,
            "media.prepare",
            &format!("namespace:{namespace}/subject:{subject}"),
            &digest(&json!({"source":source,"plan":plan.fingerprint()}))?,
            &json!({"source":source,"plan":plan.fingerprint()}),
        )?;
        let mut tx = match self.database.begin_operation(key).await? {
            BeginOperation::Replay(receipt) => {
                return serde_json::from_value(receipt.result["prepared"].clone())
                    .map_err(Error::from);
            }
            BeginOperation::New(tx) => tx,
        };
        if let Some(existing) = tx
            .media_preparation(namespace, subject, source, plan.fingerprint())
            .await?
        {
            tx.rollback().await?;
            return Ok(existing.graph);
        }
        let context = adoption::PreparationScope {
            namespace,
            subject,
            source,
            quality: &prepared.quality,
        };
        let mut inputs = Vec::new();
        for (item, text) in prepared.items.into_iter().zip(texts) {
            if !snapshot.coordinate.contains(&item.coordinate) {
                return Err(Error::invalid(
                    "prepared output lies outside the exact selected source",
                ));
            }
            inputs.push(
                adoption::persist_item(
                    &mut tx,
                    &context,
                    &snapshot,
                    item,
                    text,
                    &prepared.preprocessing_digest,
                    plan.policy_digest(),
                )
                .await?,
            );
        }
        let graph = MediaPreparedGraph {
            inputs,
            quality: prepared.quality,
            preprocessing_digest: prepared.preprocessing_digest,
        };
        let record = MediaPreparationRecord {
            namespace_id: namespace.clone(),
            subject_id: subject.clone(),
            source: source.clone(),
            plan_digest: plan.fingerprint().into(),
            graph: graph.clone(),
            recorded_at: tx.time().recorded(),
            commit_seq: tx.commit_seq(),
        };
        tx.put_media_preparation(&record).await?;
        tx.commit(json!({"prepared":graph,"plan":plan})).await?;
        Ok(graph)
    }
}
pub(crate) fn digest(value: &serde_json::Value) -> Result<String> {
    Ok(format!("{:x}", Sha256::digest(serde_json::to_vec(value)?)))
}
