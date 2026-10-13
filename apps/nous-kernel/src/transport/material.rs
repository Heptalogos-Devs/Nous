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
use std::pin::Pin;

const MAX_UNARY_MATERIAL_BYTES: u64 = 1_048_576;

fn materialize_request(input: p::MaterializeRequest) -> Result<(SubjectId, MaterializeRequest)> {
    let byte_range = match (input.start, input.end) {
        (None, None) => None,
        (Some(start), Some(end)) => Some(ByteRange { start, end }),
        _ => return Err(Error::Invalid("both range endpoints required".into())),
    };
    Ok((
        SubjectId(id(&input.subject_id)?),
        MaterializeRequest {
            reference: from_ref(required(input.reference, "reference")?)?,
            byte_range,
            max_bytes: input.max_bytes,
        },
    ))
}

fn materialized_evidence(result: nous_material::MaterializedEvidence) -> p::MaterializedEvidence {
    p::MaterializedEvidence {
        reference: Some(to_ref(result.reference)),
        media_type: result.media_type,
        content: result.bytes,
        evidence: result
            .provenance
            .into_iter()
            .map(|reference| p::Evidence {
                epistemic_relation: None,
                reference: Some(to_ref(reference)),
                basis_role: "direct".into(),
            })
            .collect(),
        degradation: Vec::new(),
        total_bytes: result.total_bytes,
        range_start: result.byte_range.start,
        range_end: result.byte_range.end,
        partial: result.partial,
    }
}

pub(super) fn artifact(value: nous_material::Artifact) -> p::Artifact {
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
    pub(super) async fn read_lineage(
        &self,
        input: p::MaterialLineageRequest,
    ) -> Result<p::MaterialLineage> {
        let roots = input
            .roots
            .into_iter()
            .map(from_ref)
            .collect::<Result<Vec<_>>>()?;
        let view = self
            .0
            .material
            .lineage_view(
                SubjectId(id(&input.subject_id)?),
                &roots,
                if input.max_nodes == 0 {
                    512
                } else {
                    input.max_nodes as usize
                },
                if input.max_bytes == 0 {
                    65536
                } else {
                    input.max_bytes as usize
                },
            )
            .await?;
        let nodes = view
            .nodes
            .into_iter()
            .map(|node| {
                use nous_material::MaterialLineageNode as N;
                use p::material_lineage_node::Value as V;
                let reference = Some(to_ref(node.reference()));
                let value = Some(match node {
                    N::Occurrence(v) => V::Occurrence(evidence::occurrence(v)),
                    N::Artifact(v) => V::Artifact(artifact(v)),
                    N::SourceRegion(v) => V::SourceRegion(evidence::source_region(v)),
                    N::Representation(v) => V::Representation(evidence::representation(*v)),
                    N::DerivedRegion(v) => V::DerivedRegion(evidence::derived_region(v)),
                });
                p::MaterialLineageNode { reference, value }
            })
            .collect();
        Ok(p::MaterialLineage {
            nodes,
            truncated: view.truncated,
            payloads_truncated: view.payloads_truncated,
        })
    }

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
            return Err(Error::Invalid("Artifact has no status filter".into()));
        }
        let subject = SubjectId(id(&input.subject_id)?);
        let scope = format!("artifacts:{}", input.subject_id);
        let (limit, last) = page(input.page, &scope)?;
        let (items, more) = self.0.material.artifacts(subject, limit, last).await?;
        let next_page_token = if more {
            next_token(
                &scope,
                items
                    .last()
                    .ok_or_else(|| Error::Internal("empty page".into()))?
                    .artifact_id
                    .0,
            )
        } else {
            String::new()
        };
        Ok(p::ListArtifactsResponse {
            items: items.into_iter().map(artifact).collect(),
            next_page_token,
        })
    }
    pub(super) async fn materialize_evidence(
        &self,
        input: p::MaterializeRequest,
    ) -> Result<p::MaterializedEvidence> {
        if input.max_bytes > MAX_UNARY_MATERIAL_BYTES {
            return Err(Error::Invalid(
                "unary materialization exceeds 1 MiB; use stream".into(),
            ));
        }
        let (subject, request) = materialize_request(input)?;
        self.0
            .material
            .materialize(subject, request)
            .await
            .map(materialized_evidence)
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
            Ok(_) => Err(Error::Invalid("invalid upload chunk".into())),
            Err(error) => Err(Error::Unavailable(error.to_string())),
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
        let bytes = self
            .0
            .material
            .artifact_stream(subject, artifact_id, input.start, input.end)
            .await
            .map_err(status)?;
        let stream = bytes.map(|chunk| {
            chunk
                .map(|content| k::DownloadChunk { content })
                .map_err(status)
        });
        Ok(Response::new(Box::pin(stream)))
    }
    type MaterializeEvidenceStream =
        Pin<Box<dyn Stream<Item = std::result::Result<k::MaterializedChunk, Status>> + Send>>;
    async fn materialize_evidence(
        &self,
        request: Request<p::MaterializeRequest>,
    ) -> std::result::Result<Response<Self::MaterializeEvidenceStream>, Status> {
        let (subject, request) = materialize_request(request.into_inner()).map_err(status)?;
        let (header, bytes) = self
            .0
            .material
            .materialize_stream(subject, request, None)
            .await
            .map_err(status)?;
        let header = futures::stream::once(async move {
            Ok(k::MaterializedChunk {
                part: Some(k::materialized_chunk::Part::Header(materialized_evidence(
                    header,
                ))),
            })
        });
        let content = bytes.map(|bytes| {
            bytes
                .map(|content| k::MaterializedChunk {
                    part: Some(k::materialized_chunk::Part::Content(content)),
                })
                .map_err(status)
        });
        Ok(Response::new(Box::pin(header.chain(content))))
    }
}
