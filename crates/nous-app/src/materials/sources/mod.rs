mod text;
use nous_domain::{media::*, *};
use nous_foundation::{
    content::{ObjectReader, ObjectStore, ReferenceLease},
    media::Media,
    models::{MediaReadFuture, ModelMediaReader},
};
use nous_storage::Database;
use serde::Serialize;
use std::collections::BTreeSet;
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
pub struct SourceReader {
    database: Database,
    objects: ObjectStore,
    media: Media,
}

pub struct OriginalStream {
    pub material: Material,
    pub details: MaterialDetails,
    pub reader: ObjectReader,
    pub watermark: i64,
}

#[derive(Serialize)]
pub struct OriginalMetadata {
    pub material: Material,
    pub details: MaterialDetails,
    pub watermark: i64,
    #[serde(skip)]
    _protection: ReferenceLease,
}

#[derive(Debug, Clone, Serialize)]
pub struct SourceSnapshot {
    #[serde(skip)]
    _protection: ReferenceLease,
    pub reference: MediaReference,
    pub roots: BTreeSet<MaterialId>,
    pub payload: RepresentationPayload,
    pub coordinate: SourceCoordinate,
    pub access: ChannelAccess,
    pub timeline: Option<MediaReference>,
    pub quality: DerivationQuality,
    pub producer: Option<DerivationProducer>,
    pub inputs: Vec<DerivationInput>,
    pub source_event_kind: Option<ObservationKind>,
    pub watermark: i64,
}

impl SourceReader {
    pub fn new(database: Database, objects: ObjectStore, media: Media) -> Self {
        Self {
            database,
            objects,
            media,
        }
    }

    pub async fn original_metadata(
        &self,
        ctx: &CallContext,
        id: &MaterialId,
        cancel: &CancellationToken,
    ) -> Result<OriginalMetadata> {
        let protection = self.objects.protect_references(cancel).await?;
        let mut view = self.database.read_view().await?;
        let material = view.material_by_id(id).await?;
        for scope in &ctx.source_read {
            view.require_source_read(
                ctx,
                &material.namespace_id,
                &scope.subject_id,
                &RetrievalTarget::Media(MediaReference::Original {
                    material_id: id.clone(),
                }),
            )
            .await?;
        }
        match &ctx.scope {
            AccessScope::Subject { subject_id, .. } => {
                ctx.require_subject(&material.namespace_id, subject_id)?;
                if !view
                    .subject_has_material_access(&material.namespace_id, subject_id, id)
                    .await?
                {
                    return Err(Error::new(
                        ErrorCode::Forbidden,
                        "material access has not been granted to this subject",
                    ));
                }
            }
            _ => ctx.require_namespace(&material.namespace_id)?,
        }
        let details = view.material_details(&material.namespace_id, id).await?;
        let watermark = view.watermark;
        view.finish().await?;
        Ok(OriginalMetadata {
            material,
            details,
            watermark,
            _protection: protection,
        })
    }
    pub async fn open_original(
        &self,
        ctx: &CallContext,
        id: &MaterialId,
        cancel: &CancellationToken,
    ) -> Result<OriginalStream> {
        let source = self.original_metadata(ctx, id, cancel).await?;
        let reader = self
            .media
            .open(&original_blob(&source.material)?, cancel)
            .await?;
        Ok(OriginalStream {
            material: source.material,
            details: source.details,
            reader,
            watermark: source.watermark,
        })
    }

    pub async fn original_text(
        &self,
        ctx: &CallContext,
        id: &MaterialId,
        cancel: &CancellationToken,
    ) -> Result<MaterialContent> {
        let source = self.original_metadata(ctx, id, cancel).await?;
        if !matches!(source.material.metadata, MediaMetadata::Text) {
            return Err(Error::new(
                ErrorCode::CapabilityUnavailable,
                "select a derived text representation for non-text material",
            ));
        }
        let text = self
            .media
            .read_text(&original_blob(&source.material)?, cancel)
            .await?;
        Ok(MaterialContent {
            material: source.material,
            details: source.details,
            text,
        })
    }

    pub async fn lineage(
        &self,
        ctx: &CallContext,
        namespace: &NamespaceId,
        subject: &SubjectId,
        reference: &MediaReference,
    ) -> Result<MediaSourceLineage> {
        self.lineage_selected(ctx, namespace, subject, reference, None)
            .await
    }
    pub async fn lineage_for_event(
        &self,
        ctx: &CallContext,
        namespace: &NamespaceId,
        subject: &SubjectId,
        reference: &MediaReference,
        observation: &ObservationId,
    ) -> Result<MediaSourceLineage> {
        self.lineage_selected(ctx, namespace, subject, reference, Some(observation))
            .await
    }
    async fn lineage_selected(
        &self,
        ctx: &CallContext,
        namespace: &NamespaceId,
        subject: &SubjectId,
        reference: &MediaReference,
        observation: Option<&ObservationId>,
    ) -> Result<MediaSourceLineage> {
        ctx.require_subject(namespace, subject)?;
        reference.validate()?;
        let _protection = self
            .objects
            .protect_references(&CancellationToken::new())
            .await?;
        let mut view = self.database.read_view().await?;
        view.subject(namespace, subject).await?;
        let mut lineage = view.media_lineage(namespace, subject, reference).await?;
        view.require_source_requirements(
            ctx,
            namespace,
            subject,
            &[SourceReadRef::Media {
                reference: reference.clone(),
                observation_id: observation.cloned(),
            }],
        )
        .await?;
        for representation in &lineage.representations {
            view.require_source_read(
                ctx,
                namespace,
                subject,
                &RetrievalTarget::Media(MediaReference::Representation {
                    reference: representation.reference(),
                }),
            )
            .await?;
        }
        for original in &mut lineage.materials {
            if !view
                .subject_has_material_access(namespace, subject, &original.material.id)
                .await?
            {
                return Err(Error::new(
                    ErrorCode::Forbidden,
                    "media source has not been made accessible to this subject",
                ));
            }
            if !ctx.source_read.is_empty() {
                let requirement = SourceReadRef::Media {
                    reference: MediaReference::Original {
                        material_id: original.material.id.clone(),
                    },
                    observation_id: observation.cloned(),
                };
                match view
                    .require_source_requirements(ctx, namespace, subject, &[requirement])
                    .await
                {
                    Ok(()) => {}
                    Err(error) if error.code == ErrorCode::Forbidden => {
                        original.details.metadata = serde_json::json!({});
                        original.details.structured = None;
                        original.details_redacted = true;
                    }
                    Err(error) => return Err(error),
                }
            }
        }
        for original in &lineage.materials {
            lineage.import_containers.extend(
                view.import_containers(namespace, subject, &original.material.id)
                    .await?,
            );
        }
        view.finish().await?;
        Ok(lineage)
    }

    pub async fn resolve(
        &self,
        ctx: &CallContext,
        namespace: &NamespaceId,
        subject: &SubjectId,
        reference: &MediaReference,
    ) -> Result<SourceSnapshot> {
        let mut view = self.database.read_view().await?;
        let snapshot = self
            .resolve_in(ctx, &mut view, namespace, subject, reference, None)
            .await?;
        view.finish().await?;
        Ok(snapshot)
    }
    pub async fn resolve_for_event(
        &self,
        ctx: &CallContext,
        namespace: &NamespaceId,
        subject: &SubjectId,
        reference: &MediaReference,
        observation: &ObservationId,
    ) -> Result<SourceSnapshot> {
        let mut view = self.database.read_view().await?;
        let snapshot = self
            .resolve_in(
                ctx,
                &mut view,
                namespace,
                subject,
                reference,
                Some(observation),
            )
            .await?;
        view.finish().await?;
        Ok(snapshot)
    }

    pub(crate) async fn resolve_in(
        &self,
        ctx: &CallContext,
        view: &mut nous_storage::ReadView,
        namespace: &NamespaceId,
        subject: &SubjectId,
        reference: &MediaReference,
        observation: Option<&ObservationId>,
    ) -> Result<SourceSnapshot> {
        ctx.require_subject(namespace, subject)?;
        reference.validate()?;
        let protection = self
            .objects
            .protect_references(&CancellationToken::new())
            .await?;
        view.subject(namespace, subject).await?;
        view.require_source_requirements(
            ctx,
            namespace,
            subject,
            &[SourceReadRef::Media {
                reference: reference.clone(),
                observation_id: observation.cloned(),
            }],
        )
        .await?;
        let roots = view.media_roots(namespace, subject, reference).await?;
        for material in &roots {
            if !view
                .subject_has_material_access(namespace, subject, material)
                .await?
            {
                return Err(Error::new(
                    ErrorCode::Forbidden,
                    "media source has not been made accessible to this subject",
                ));
            }
        }
        let watermark = view.watermark;
        let mut snapshot = match reference {
            MediaReference::Original { material_id } => {
                let material = view.material(namespace, material_id).await?;
                original_snapshot(
                    reference.clone(),
                    roots,
                    material,
                    SourceCoordinate::Whole,
                    watermark,
                    protection,
                )?
            }
            MediaReference::SourceRegion { id } => {
                let region = view.source_region(namespace, id).await?;
                let material = view.material(namespace, &region.material_id).await?;
                original_snapshot(
                    reference.clone(),
                    roots,
                    material,
                    region.coordinate,
                    watermark,
                    protection,
                )?
            }
            MediaReference::Representation { reference: exact } => {
                let value = view.representation(namespace, subject, exact).await?;
                representation_snapshot(reference.clone(), roots, value, watermark, protection)
            }
            MediaReference::DerivedRegion { id } => {
                let region = view.derived_region(namespace, subject, id).await?;
                let value = view
                    .representation(namespace, subject, &region.representation)
                    .await?;
                let selector = region.selector;
                let mut snapshot =
                    representation_snapshot(reference.clone(), roots, value, watermark, protection);
                text::select(&mut snapshot, selector)?;
                snapshot
            }
        };
        if let RepresentationPayload::Artifact { metadata, .. } = &snapshot.payload {
            snapshot.access = physical_access(metadata);
        }
        Ok(snapshot)
    }
    pub async fn text(
        &self,
        source: &SourceSnapshot,
        cancel: &CancellationToken,
    ) -> Result<String> {
        if let Some(text) = representation_text(&source.payload, None)? {
            return Ok(text);
        }
        match &source.payload {
            RepresentationPayload::Artifact {
                blob,
                metadata: MediaMetadata::Text,
            } => match &source.coordinate {
                SourceCoordinate::Whole => self.media.read_text(blob, cancel).await,
                SourceCoordinate::TextSpan { start, end }
                | SourceCoordinate::ByteRange { start, end } => {
                    self.media.read_text_range(blob, *start..*end, cancel).await
                }
                _ => Err(Error::invalid("selected source coordinate is not textual")),
            },
            _ => Err(Error::new(
                ErrorCode::CapabilityUnavailable,
                "selected source has no text representation",
            )),
        }
    }
}

fn original_snapshot(
    reference: MediaReference,
    roots: BTreeSet<MaterialId>,
    material: Material,
    coordinate: SourceCoordinate,
    watermark: i64,
    protection: ReferenceLease,
) -> Result<SourceSnapshot> {
    let duration = coordinate.duration_ms(&material.metadata);
    let timeline = duration.map(|_| reference.clone());
    let access = physical_access(&material.metadata);
    Ok(SourceSnapshot {
        _protection: protection,
        reference,
        roots,
        payload: RepresentationPayload::Artifact {
            blob: original_blob(&material)?,
            metadata: material.metadata,
        },
        quality: DerivationQuality {
            partial: !matches!(coordinate, SourceCoordinate::Whole),
            source_duration_ms: duration,
            sampling: None,
        },
        coordinate,
        access,
        timeline,
        producer: None,
        inputs: Vec::new(),
        source_event_kind: None,
        watermark,
    })
}
fn representation_snapshot(
    reference: MediaReference,
    roots: BTreeSet<MaterialId>,
    value: DerivedRepresentation,
    watermark: i64,
    protection: ReferenceLease,
) -> SourceSnapshot {
    SourceSnapshot {
        _protection: protection,
        reference,
        roots,
        payload: value.payload,
        coordinate: SourceCoordinate::Whole,
        access: value.access.represented(),
        timeline: value.timeline,
        quality: value.quality,
        producer: Some(value.producer),
        inputs: value.inputs,
        source_event_kind: value.source_event_kind,
        watermark,
    }
}
pub(super) fn original_blob(material: &Material) -> Result<BlobReference> {
    let blob = BlobReference {
        content_hash: material.content_hash.clone(),
        media_type: material.media_type.clone(),
        byte_len: u64::try_from(material.byte_len)
            .map_err(|_| Error::internal("material has an invalid byte length"))?,
    };
    blob.validate()?;
    Ok(blob)
}
pub fn physical_access(metadata: &MediaMetadata) -> ChannelAccess {
    let unavailable = ChannelMode::Unavailable;
    let original = ChannelMode::Original;
    match metadata {
        MediaMetadata::Text => ChannelAccess {
            visual: unavailable,
            audio: unavailable,
            source_text: original,
        },
        MediaMetadata::Image { .. } => ChannelAccess {
            visual: original,
            audio: unavailable,
            source_text: unavailable,
        },
        MediaMetadata::Audio { .. } => ChannelAccess {
            visual: unavailable,
            audio: original,
            source_text: unavailable,
        },
        MediaMetadata::Video { has_audio, .. } => ChannelAccess {
            visual: original,
            audio: if *has_audio { original } else { unavailable },
            source_text: unavailable,
        },
        _ => ChannelAccess {
            visual: unavailable,
            audio: unavailable,
            source_text: unavailable,
        },
    }
}
impl ModelMediaReader for SourceReader {
    fn read(
        &self,
        scope: ConfigScope,
        source: MediaReference,
        expected: BlobReference,
        max_bytes: usize,
        cancel: CancellationToken,
    ) -> MediaReadFuture {
        let owner = self.clone();
        Box::pin(async move {
            let ConfigScope::Subject {
                namespace_id,
                subject_id,
            } = scope
            else {
                return Err(Error::invalid(
                    "stored model media requires a subject-scoped invocation",
                ));
            };
            let mut context = CallContext::new(
                "model-input",
                AccessScope::Subject {
                    namespace_id: namespace_id.clone(),
                    subject_id: subject_id.clone(),
                },
            );
            if let Some(trace) = nous_foundation::observability::current_trace() {
                context.trace_id = trace;
            }
            let snapshot = owner
                .resolve(&context, &namespace_id, &subject_id, &source)
                .await?;
            let RepresentationPayload::Artifact { blob, .. } = snapshot.payload else {
                return Err(Error::invalid(
                    "model media locator does not name an artifact",
                ));
            };
            if !matches!(snapshot.coordinate, SourceCoordinate::Whole)
                || blob.content_hash != expected.content_hash
                || blob.byte_len != expected.byte_len
                || canonical_media_type(&blob.media_type)?
                    != canonical_media_type(&expected.media_type)?
            {
                return Err(Error::new(
                    ErrorCode::Conflict,
                    "model media locator differs from its admitted exact artifact",
                ));
            }
            owner
                .objects
                .read_bytes(
                    &blob.content_hash,
                    i64::try_from(blob.byte_len)
                        .map_err(|_| Error::invalid("media length is outside storage range"))?,
                    max_bytes as u64,
                    &cancel,
                )
                .await
        })
    }
}
