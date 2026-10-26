use crate::{
    App,
    materials::sources::{SourceSnapshot, physical_access},
};
use nous_domain::{media::*, *};
use nous_foundation::media::{PreparationRequest, PreparedItem};
use nous_storage::WriteTransaction;
use serde_json::json;
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;

impl App {
    pub(super) async fn prepare_media_run(
        &self,
        run: MediaDerivationRun,
        cancel: &CancellationToken,
    ) -> Result<MediaDerivationRun> {
        if run.prepared.is_some() {
            return Ok(run);
        }
        let (namespace, subject) = run.job.subject_scope()?;
        let ctx = run.job.call_context("nous-media")?;
        let source = self
            .sources
            .resolve(&ctx, namespace, subject, &run.request.source)
            .await?;
        let prepared = if let RepresentationPayload::Artifact { blob, metadata } = &source.payload {
            Some(
                self.media
                    .prepare(
                        &ctx,
                        PreparationRequest {
                            blob: blob.clone(),
                            metadata: metadata.clone(),
                            coordinate: source.coordinate.clone(),
                            delivery: run.request.delivery,
                        },
                        cancel,
                    )
                    .await?,
            )
        } else {
            if run.request.delivery != MediaDelivery::Native {
                return Err(Error::invalid(
                    "sampling requires a temporal media artifact",
                ));
            }
            None
        };
        if cancel.is_cancelled() {
            return Err(Error::new(
                ErrorCode::Cancelled,
                "media preparation cancelled",
            ));
        }
        let Some(mut tx) = self
            .worker_transaction(
                &run.job,
                "media.prepare",
                &run.job.id,
                &run.job.id,
                &json!({"job_id":run.job.id}),
            )
            .await?
        else {
            return self.database.media_derivation(&run.job.id).await;
        };
        let current = tx.media_derivation(&run.job.id).await?;
        if current.job.state == JobState::Cancelled || current.prepared.is_some() {
            tx.rollback().await?;
            return Ok(current);
        }
        let graph = if let Some(prepared) = prepared {
            let maximum = if run.request.strategy == DerivationStrategy::DescribeThenStructure {
                63
            } else {
                64
            };
            if prepared.items.is_empty() || prepared.items.len() > maximum {
                return Err(Error::invalid(
                    "media preparation exceeds the invocation's input catalog capacity",
                ));
            }
            let mut inputs = Vec::new();
            for item in prepared.items {
                if !source.coordinate.contains(&item.coordinate) {
                    return Err(Error::invalid(
                        "prepared output lies outside its exact selected source",
                    ));
                }
                let role = if item.at_ms.is_some() {
                    DerivationInputRole::SampledFrame
                } else {
                    DerivationInputRole::Source
                };
                let at_ms = item.at_ms;
                let reference = persist_item(
                    &mut tx,
                    &run,
                    &source,
                    item,
                    &prepared.preprocessing_digest,
                    &prepared.quality,
                )
                .await?;
                inputs.push(MediaPreparedInput {
                    source: reference,
                    role,
                    at_ms,
                });
            }
            MediaPreparedGraph {
                inputs,
                quality: prepared.quality,
                preprocessing_digest: prepared.preprocessing_digest,
            }
        } else {
            MediaPreparedGraph {
                inputs: vec![MediaPreparedInput {
                    source: run.request.source.clone(),
                    role: DerivationInputRole::Source,
                    at_ms: None,
                }],
                quality: source.quality.clone(),
                preprocessing_digest: digest(
                    &json!({"source":run.request.source,"payload":source.payload,"producer":source.producer}),
                )?,
            }
        };
        tx.set_media_prepared(&run.job.id, &graph).await?;
        tx.commit(json!({"prepared":graph})).await?;
        self.database.media_derivation(&run.job.id).await
    }
}

async fn persist_item(
    tx: &mut WriteTransaction,
    run: &MediaDerivationRun,
    source: &SourceSnapshot,
    item: PreparedItem,
    preprocessing: &str,
    quality: &DerivationQuality,
) -> Result<MediaReference> {
    if item.object.is_none()
        && matches!(source.coordinate, SourceCoordinate::Whole)
        && matches!(item.coordinate, SourceCoordinate::Whole)
    {
        return Ok(run.request.source.clone());
    }
    let input = anchor(tx, run, &item.coordinate).await?;
    let (namespace, subject) = run.job.subject_scope()?;
    if let Some(object) = &item.object {
        if object.digest != item.blob.content_hash
            || u64::try_from(object.byte_len).ok() != Some(item.blob.byte_len)
        {
            return Err(Error::internal(
                "prepared blob differs from its protected publication",
            ));
        }
        object.protect_in(tx);
    }
    let producer = DerivationProducer {
        signature: digest(
            &json!({"preprocessing":preprocessing,"input":input,"blob":item.blob,"metadata":item.metadata}),
        )?,
        implementation: "media.prepare".into(),
        model_call_id: None,
        prompt_digest: None,
        schema_digest: None,
        config_digest: preprocessing.into(),
        preprocessing_digest: Some(preprocessing.into()),
    };
    let value = DerivedRepresentation {
        id: DerivedRepresentationId::new(),
        namespace_id: namespace.clone(),
        subject_id: subject.clone(),
        revision: 1,
        inputs: vec![DerivationInput {
            ordinal: 0,
            source: input,
            role: DerivationInputRole::Source,
        }],
        strategy: DerivationStrategy::DescriptionOnly,
        kind: RepresentationKind::Other,
        access: physical_access(&item.metadata),
        payload: RepresentationPayload::Artifact {
            blob: item.blob,
            metadata: item.metadata,
        },
        producer,
        observation_id: Some(run.request.observation_id.clone()),
        source_event_kind: Some(run.source_event_kind),
        timeline: Some(run.request.source.clone()),
        formed_at: tx.time().point(),
        recorded_at: tx.time().recorded(),
        commit_seq: tx.commit_seq(),
        previous: None,
        quality: quality.clone(),
    };
    tx.save_representation(&value, 0).await?;
    Ok(MediaReference::Representation {
        reference: value.reference(),
    })
}

async fn anchor(
    tx: &mut WriteTransaction,
    run: &MediaDerivationRun,
    coordinate: &SourceCoordinate,
) -> Result<MediaReference> {
    let (namespace, subject) = run.job.subject_scope()?;
    let request = &run.request.source;
    let (material_id, source_parent, representation, derived_parent, existing_coordinate) =
        match request {
            MediaReference::Original { material_id } => (
                Some(material_id.clone()),
                None,
                None,
                None,
                SourceCoordinate::Whole,
            ),
            MediaReference::SourceRegion { id } => {
                let region = tx.source_region(namespace, id).await?;
                (
                    Some(region.material_id),
                    Some(id.clone()),
                    None,
                    None,
                    region.coordinate,
                )
            }
            MediaReference::Representation { reference } => (
                None,
                None,
                Some(reference.clone()),
                None,
                SourceCoordinate::Whole,
            ),
            MediaReference::DerivedRegion { id } => {
                let region = tx.derived_region(namespace, subject, id).await?;
                let DerivedSelector::Artifact { coordinate } = region.selector else {
                    return Err(Error::invalid(
                        "prepared bytes require an artifact source region",
                    ));
                };
                (
                    None,
                    None,
                    Some(region.representation),
                    Some(id.clone()),
                    coordinate,
                )
            }
        };
    if existing_coordinate == *coordinate {
        return Ok(request.clone());
    }
    if !existing_coordinate.contains(coordinate) {
        return Err(Error::invalid(
            "prepared coordinate is outside admitted media",
        ));
    }
    if let Some(material_id) = material_id {
        Ok(tx
            .put_source_region(SourceRegion {
                id: SourceRegionId::new(),
                namespace_id: namespace.clone(),
                material_id,
                parent: source_parent,
                coordinate: coordinate.clone(),
                coordinate_hash: String::new(),
                recorded_at: tx.time().recorded(),
                commit_seq: tx.commit_seq(),
            })
            .await?
            .reference())
    } else {
        Ok(tx
            .put_derived_region(DerivedRegion {
                id: DerivedRegionId::new(),
                namespace_id: namespace.clone(),
                subject_id: subject.clone(),
                representation: representation
                    .ok_or_else(|| Error::internal("artifact anchor is missing"))?,
                parent: derived_parent,
                selector: DerivedSelector::Artifact {
                    coordinate: coordinate.clone(),
                },
                recorded_at: tx.time().recorded(),
                commit_seq: tx.commit_seq(),
            })
            .await?
            .reference())
    }
}
pub(super) fn digest(value: &serde_json::Value) -> Result<String> {
    Ok(format!("{:x}", Sha256::digest(serde_json::to_vec(value)?)))
}
