use super::digest;
use crate::materials::sources::{SourceSnapshot, physical_access};
use nous_domain::{media::*, *};
use nous_foundation::media::PreparedItem;
use nous_storage::WriteTransaction;
use serde_json::json;

pub(super) struct PreparationScope<'a> {
    pub namespace: &'a NamespaceId,
    pub subject: &'a SubjectId,
    pub source: &'a MediaReference,
    pub quality: &'a DerivationQuality,
}
pub(super) async fn persist_item(
    tx: &mut WriteTransaction,
    scope: &PreparationScope<'_>,
    snapshot: &SourceSnapshot,
    item: PreparedItem,
    text: Option<String>,
    preprocessing: &str,
    config_digest: &str,
) -> Result<MediaPreparedInput> {
    let role = if item.at_ms.is_some() {
        DerivationInputRole::SampledFrame
    } else {
        DerivationInputRole::Source
    };
    let at_ms = item.at_ms;
    let reused = item.object.is_none()
        && matches!(snapshot.coordinate, SourceCoordinate::Whole)
        && matches!(item.coordinate, SourceCoordinate::Whole);
    let reference = if reused {
        scope.source.clone()
    } else {
        let input = anchor(tx, scope, &item.coordinate).await?;
        if let Some(object) = &item.object {
            if object.digest != item.blob.content_hash
                || u64::try_from(object.byte_len).ok() != Some(item.blob.byte_len)
            {
                return Err(Error::internal(
                    "prepared blob differs from protected publication",
                ));
            }
            object.protect_in(tx);
        }
        let producer = mechanical_producer(
            &input,
            &json!({"blob":item.blob,"metadata":item.metadata}),
            preprocessing,
            config_digest,
        )?;
        let value = representation(
            tx,
            scope,
            input,
            RepresentationPayload::Artifact {
                blob: item.blob,
                metadata: item.metadata.clone(),
            },
            physical_access(&item.metadata),
            RepresentationKind::Other,
            producer,
        );
        tx.save_representation(&value, 0).await?;
        MediaReference::Representation {
            reference: value.reference(),
        }
    };
    let text_source = if let Some(text) = text {
        if reused && matches!(reference, MediaReference::Original { .. }) {
            None
        } else {
            let producer = mechanical_producer(
                &reference,
                &json!({"text":text}),
                preprocessing,
                config_digest,
            )?;
            let value = representation(
                tx,
                scope,
                reference.clone(),
                RepresentationPayload::Text { text },
                physical_access(&MediaMetadata::Text),
                RepresentationKind::ExtractedText,
                producer,
            );
            tx.save_representation(&value, 0).await?;
            Some(value.reference())
        }
    } else {
        None
    };
    Ok(MediaPreparedInput {
        source: reference,
        role,
        at_ms,
        text_source,
    })
}

fn mechanical_producer(
    input: &MediaReference,
    payload: &serde_json::Value,
    preprocessing: &str,
    config_digest: &str,
) -> Result<DerivationProducer> {
    Ok(DerivationProducer {
        signature: digest(&json!({"preprocessing":preprocessing,"input":input,"payload":payload}))?,
        implementation: "media.prepare".into(),
        model_call_id: None,
        prompt_digest: None,
        schema_digest: None,
        config_digest: config_digest.into(),
        preprocessing_digest: Some(preprocessing.into()),
    })
}
fn representation(
    tx: &WriteTransaction,
    scope: &PreparationScope<'_>,
    input: MediaReference,
    payload: RepresentationPayload,
    access: ChannelAccess,
    kind: RepresentationKind,
    producer: DerivationProducer,
) -> DerivedRepresentation {
    DerivedRepresentation {
        id: DerivedRepresentationId::new(),
        namespace_id: scope.namespace.clone(),
        subject_id: scope.subject.clone(),
        revision: 1,
        inputs: vec![DerivationInput {
            ordinal: 0,
            source: input,
            role: DerivationInputRole::Source,
        }],
        strategy: DerivationStrategy::DescriptionOnly,
        kind,
        payload,
        producer,
        access,
        observation_id: None,
        source_event_kind: None,
        timeline: Some(scope.source.clone()),
        formed_at: tx.time().point(),
        recorded_at: tx.time().recorded(),
        commit_seq: tx.commit_seq(),
        previous: None,
        quality: scope.quality.clone(),
    }
}

async fn anchor(
    tx: &mut WriteTransaction,
    scope: &PreparationScope<'_>,
    coordinate: &SourceCoordinate,
) -> Result<MediaReference> {
    let (namespace, subject, source) = (scope.namespace, scope.subject, scope.source);
    let (material_id, source_parent, representation, derived_parent, existing_coordinate) =
        match source {
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
        return Ok(source.clone());
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
