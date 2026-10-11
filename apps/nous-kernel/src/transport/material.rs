// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;
use futures::{Stream, StreamExt};
use nous_core::{ArtifactId, EntityRef, ObjectRef, ResourceRef, Result, SessionId, SubjectId};
use nous_material::{ByteRange, MaterializeRequest, UploadMetadata};
use nous_material::{
    ObservationInput, ObservationMaterial, OccurrenceDescriptor, ResolvedEntityMention,
    RuntimeDirective,
};
use nous_persistence::database_error as db;
use std::pin::Pin;
use uuid::Uuid;

const MAX_UNARY_MATERIAL_BYTES: u64 = 1_048_576;
const MAX_ARTIFACT_STREAM_CHUNK_BYTES: u64 = 262_144;

fn artifact(value: nous_material::Artifact) -> p::Artifact {
    p::Artifact {
        artifact_id: value.artifact_id.0.to_string(),
        subject_id: value.subject_id.0.to_string(),
        content_hash: value.content_hash,
        byte_length: value.byte_length,
        media_type: value.media_type,
        created_at: Some(timestamp(value.created_at)),
        metadata: to_object(value.metadata),
    }
}

impl KernelService {
    pub(super) async fn record_observation(
        &self,
        input: p::ObservationInput,
    ) -> Result<p::AcceptedObservation> {
        use p::observation_input::Material;
        let material = match required(input.material, "material")? {
            Material::InlineText(text) => ObservationMaterial::InlineText {
                text: text.text,
                media_type: text.media_type,
            },
            Material::ArtifactId(value) => ObservationMaterial::ArtifactRef {
                artifact_id: ArtifactId(id(&value)?),
            },
            Material::ObjectRef(value) => ObservationMaterial::ExternalObjectRef {
                object_ref: ObjectRef::new(value)?,
            },
            Material::Structured(value) => {
                ObservationMaterial::StructuredJson { value: json(value) }
            }
            Material::ResourceRef(value) => ObservationMaterial::ResourceAvailability {
                resource: ResourceRef::new(value)?,
            },
        };
        let accepted = self
            .0
            .material
            .record_observation_once(
                ObservationInput {
                    subject: SubjectId(id(&input.subject_id)?),
                    session: input
                        .session_id
                        .as_deref()
                        .map(id)
                        .transpose()?
                        .map(SessionId),
                    material,
                    runtime: RuntimeDirective {
                        admit: input.admit,
                        hold_until: time(input.hold_until)?,
                    },
                    entities: input
                        .entities
                        .into_iter()
                        .map(|e| {
                            Ok(ResolvedEntityMention {
                                surface: e.surface,
                                entity_ref: e.entity_ref.map(EntityRef::new).transpose()?,
                                semantic_role: e.semantic_role,
                            })
                        })
                        .collect::<Result<_>>()?,
                    occurrence: OccurrenceDescriptor {
                        source_class: input.source_class.into(),
                        external_object_ref: input
                            .external_object_ref
                            .map(ObjectRef::new)
                            .transpose()?,
                        occurred_time: temporal(input.occurred_time)?,
                        observed_at: time(input.observed_at)?,
                        conversation_ref: input.conversation_ref,
                        actor_entity_ref: input.actor_entity_ref.map(EntityRef::new).transpose()?,
                        context: object(input.context),
                    },
                },
                input.request_id.as_deref().map(id).transpose()?,
            )
            .await?;
        Ok(p::AcceptedObservation {
            occurrence_id: accepted.occurrence.occurrence_id.0.to_string(),
            artifact_id: accepted.artifact.map(|a| a.artifact_id.0.to_string()),
            source_region_id: accepted
                .source_region
                .map(|s| s.source_region_id.0.to_string()),
            resident: accepted.resident,
            mention_ids: accepted
                .mention_ids
                .into_iter()
                .map(|i| i.to_string())
                .collect(),
        })
    }
    pub(super) async fn get_artifact(&self, input: p::ObjectRequest) -> Result<p::Artifact> {
        Ok(artifact(
            self.0
                .material
                .artifact(
                    SubjectId(id(&input.subject_id)?),
                    ArtifactId(id(&input.id)?),
                )
                .await?,
        ))
    }
    pub(super) async fn list_artifacts(
        &self,
        input: p::ListRequest,
    ) -> Result<p::ListArtifactsResponse> {
        if !input.status.is_empty() {
            return Err(Error::new(
                nous_core::ErrorOwner::Material,
                nous_core::ErrorCode::InvalidRequest,
                "Artifact has no status filter",
            ));
        }
        let subject = SubjectId(id(&input.subject_id)?);
        self.0.store.require_subject(subject).await?;
        let scope = format!("artifacts:{}", input.subject_id);
        let (limit, last) = page(input.page, &scope)?;
        let mut ids=sqlx::query_scalar::<_,Uuid>("SELECT artifact_id FROM artifacts WHERE subject_id=$1 AND ($2::uuid IS NULL OR artifact_id>$2) ORDER BY artifact_id LIMIT $3")
            .bind(subject.0).bind(last).bind(limit+1).fetch_all(self.0.store.pool()).await.map_err(db)?;
        let more = ids.len() > limit as usize;
        ids.truncate(limit as usize);
        let next_page_token = if more {
            next_token(
                &scope,
                *ids.last().ok_or_else(|| {
                    Error::new(
                        nous_core::ErrorOwner::Material,
                        nous_core::ErrorCode::InternalFailure,
                        "page continuation has no last item",
                    )
                })?,
            )
        } else {
            String::new()
        };
        let mut items = Vec::new();
        for id in ids {
            items.push(artifact(
                self.0.material.artifact(subject, ArtifactId(id)).await?,
            ));
        }
        Ok(p::ListArtifactsResponse {
            items,
            next_page_token,
        })
    }
    pub(super) async fn materialize_evidence(
        &self,
        input: p::MaterializeRequest,
    ) -> Result<p::MaterializedEvidence> {
        if input.max_bytes > MAX_UNARY_MATERIAL_BYTES {
            return Err(Error::new(
                nous_core::ErrorOwner::Material,
                nous_core::ErrorCode::InvalidRequest,
                "Invalid request",
            ));
        }
        let range = match (input.start, input.end) {
            (None, None) => None,
            (Some(start), Some(end)) => Some(ByteRange { start, end }),
            _ => {
                return Err(Error::new(
                    nous_core::ErrorOwner::Material,
                    nous_core::ErrorCode::InvalidRequest,
                    "both range endpoints required",
                ));
            }
        };
        let result = self
            .0
            .materialize(
                SubjectId(id(&input.subject_id)?),
                MaterializeRequest {
                    reference: from_ref(required(input.reference, "reference")?)?,
                    byte_range: range,
                    max_bytes: input.max_bytes,
                    resource_handle: None,
                    resource: None,
                },
            )
            .await?;
        Ok(p::MaterializedEvidence {
            reference: Some(to_ref(result.reference)),
            media_type: result.media_type,
            content: result.bytes,
            evidence: result
                .provenance
                .into_iter()
                .map(|r| p::Evidence {
                    epistemic_relation: None,
                    reference: Some(to_ref(r)),
                    basis_role: "direct".into(),
                })
                .collect(),
            degradation: vec![],
            total_bytes: result.total_bytes,
            range_start: result.byte_range.start,
            range_end: result.byte_range.end,
            partial: result.partial,
        })
    }
}
#[tonic::async_trait]
impl k::artifact_stream_service_server::ArtifactStreamService for KernelService {
    async fn upload_artifact(
        &self,
        request: Request<tonic::Streaming<k::UploadChunk>>,
    ) -> std::result::Result<Response<p::Artifact>, Status> {
        let mut stream = request.into_inner();
        let header = match stream.message().await?.and_then(|c| c.part) {
            Some(k::upload_chunk::Part::Header(h)) => h,
            _ => {
                return Err(Status::invalid_argument(
                    "first chunk must contain upload header",
                ));
            }
        };
        let subject = SubjectId(id(&header.subject_id).map_err(status)?);
        if header.media_type.is_empty() {
            return Err(Status::invalid_argument("media_type required"));
        }
        let chunks = stream.map(|chunk| match chunk {
            Ok(k::UploadChunk {
                part: Some(k::upload_chunk::Part::Content(bytes)),
            }) if bytes.len() <= 262_144 => Ok(bytes),
            Ok(_) => Err(Error::new(
                nous_core::ErrorOwner::Material,
                nous_core::ErrorCode::InvalidRequest,
                "invalid upload chunk",
            )),
            Err(error) => Err(Error::new(
                nous_core::ErrorOwner::Material,
                nous_core::ErrorCode::Unavailable,
                "Upload stream was interrupted",
            )
            .with_source(error)),
        });
        self.0
            .material
            .upload_stream(
                subject,
                UploadMetadata {
                    media_type: header.media_type,
                    metadata: serde_json::json!({}),
                },
                chunks,
            )
            .await
            .map(artifact)
            .map(Response::new)
            .map_err(status)
    }
    type DownloadArtifactStream =
        Pin<Box<dyn Stream<Item = std::result::Result<k::DownloadChunk, Status>> + Send>>;
    async fn download_artifact(
        &self,
        request: Request<k::DownloadRequest>,
    ) -> std::result::Result<Response<Self::DownloadArtifactStream>, Status> {
        let input = request.into_inner();
        let subject = SubjectId(id(&input.subject_id).map_err(status)?);
        let artifact_id = ArtifactId(id(&input.artifact_id).map_err(status)?);
        let meta = self
            .0
            .material
            .artifact(subject, artifact_id)
            .await
            .map_err(status)?;
        let start = input.start.unwrap_or(0);
        let end = input.end.unwrap_or(meta.byte_length);
        if start > end || end > meta.byte_length {
            return Err(Status::out_of_range("invalid byte range"));
        }
        let material = self.0.material.clone();
        let stream =
            futures::stream::try_unfold((start, material), move |(offset, material)| async move {
                if offset == end {
                    return Ok(None);
                }
                let next = (offset + MAX_ARTIFACT_STREAM_CHUNK_BYTES).min(end);
                let result = material
                    .materialize(
                        subject,
                        MaterializeRequest {
                            reference: nous_core::CognitiveRef::Artifact(artifact_id),
                            byte_range: Some(ByteRange {
                                start: offset,
                                end: next,
                            }),
                            max_bytes: MAX_ARTIFACT_STREAM_CHUNK_BYTES,
                            resource_handle: None,
                            resource: None,
                        },
                    )
                    .await
                    .map_err(status)?;
                Ok(Some((
                    k::DownloadChunk {
                        content: result.bytes,
                    },
                    (next, material),
                )))
            });
        Ok(Response::new(Box::pin(stream)))
    }
}
