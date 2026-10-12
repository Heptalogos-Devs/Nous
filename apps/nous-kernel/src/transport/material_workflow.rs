// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;
use nous_core::Result;
#[tonic::async_trait]
impl k::kernel_material_workflow_service_server::KernelMaterialWorkflowService for KernelService {
    async fn segment_description(
        &self,
        request: Request<p::ObjectRequest>,
    ) -> std::result::Result<Response<k::DescriptionSegments>, Status> {
        let input = request.into_inner();
        let result: Result<_> = async {
            let segments = self
                .0
                .material
                .segment_description(
                    SubjectId(id(&input.subject_id)?),
                    nous_core::DerivedRepresentationId(id(&input.id)?),
                )
                .await?;
            Ok(k::DescriptionSegments {
                segments: segments
                    .into_iter()
                    .map(|item| k::DescriptionSegment {
                        key: item.key,
                        text: item.text,
                        reference: Some(to_ref(nous_core::CognitiveRef::DerivedRegion(
                            item.region.derived_region_id,
                        ))),
                    })
                    .collect(),
            })
        }
        .await;
        result.map(Response::new).map_err(status)
    }
    async fn get_resolved_mentions(
        &self,
        request: Request<k::ResolvedMentionsRequest>,
    ) -> std::result::Result<Response<k::ResolvedMentionsResponse>, Status> {
        let input = request.into_inner();
        let result: Result<_> = async {
            let candidates = self
                .0
                .material
                .resolved_mentions(
                    SubjectId(id(&input.subject_id)?),
                    nous_core::OccurrenceId(id(&input.occurrence_id)?),
                )
                .await?
                .into_iter()
                .map(|(key, surface, entity_ref)| k::ResolvedMention {
                    key: key.to_string(),
                    surface,
                    entity_ref: entity_ref.as_str().to_owned(),
                })
                .collect();
            Ok(k::ResolvedMentionsResponse { candidates })
        }
        .await;
        result.map(Response::new).map_err(status)
    }
    async fn get_embedding_config(
        &self,
        _: Request<()>,
    ) -> std::result::Result<Response<k::EmbeddingConfig>, Status> {
        self.embedding_config().map(Response::new).map_err(status)
    }
    async fn list_embedding_needs(
        &self,
        request: Request<k::EmbeddingNeedsRequest>,
    ) -> std::result::Result<Response<k::EmbeddingNeedsResponse>, Status> {
        let input = request.into_inner();
        let result: Result<_> = async {
            let config = self.embedding_config()?;
            let subject = SubjectId(id(&input.subject_id)?);
            let bound =
                self.embedding_prepared_view(subject, input.preparation_token.as_deref())?;
            let cursor = input
                .cursor
                .map(|cursor| -> Result<nous_retrieval::EmbeddingCursor> {
                    Ok(nous_retrieval::EmbeddingCursor {
                        subject: SubjectId(id(&cursor.subject_id)?),
                        content_revision: cursor.content_revision,
                        view_digest: cursor.view_digest,
                        after: from_ref(required(cursor.after, "cursor reference")?)?,
                    })
                })
                .transpose()?;
            let page = self
                .0
                .serving
                .embedding_needs_page(
                    subject,
                    input.limit as usize,
                    cursor.as_ref(),
                    bound
                        .as_ref()
                        .and_then(|bound| bound.historical_authority.as_deref()),
                )
                .await?;
            Ok(k::EmbeddingNeedsResponse {
                config: Some(config),
                needs: page
                    .needs
                    .into_iter()
                    .map(|need| k::EmbeddingNeed {
                        reference: Some(to_ref(need.reference)),
                        text: need.text,
                        digest: need.digest,
                    })
                    .collect(),
                next_cursor: page.next_cursor.map(|cursor| k::EmbeddingCursor {
                    subject_id: cursor.subject.0.to_string(),
                    content_revision: cursor.content_revision,
                    view_digest: cursor.view_digest,
                    after: Some(to_ref(cursor.after)),
                }),
            })
        }
        .await;
        result.map(Response::new).map_err(status)
    }
    async fn commit_embeddings(
        &self,
        request: Request<k::CommitEmbeddingsRequest>,
    ) -> std::result::Result<Response<k::CommitEmbeddingsResponse>, Status> {
        let input = request.into_inner();
        let result: Result<_> = async {
            let subject = SubjectId(id(&input.subject_id)?);
            let bound =
                self.embedding_prepared_view(subject, input.preparation_token.as_deref())?;
            let items = input
                .items
                .into_iter()
                .map(|item| -> Result<_> {
                    let material = required(item.material, "material")?;
                    Ok(nous_retrieval::EmbeddingSubmission {
                        reference: from_ref(required(item.reference, "reference")?)?,
                        text: material.text,
                        space: material.space_hash,
                        producer: material.producer_hash,
                        vector: material.vector,
                    })
                })
                .collect::<Result<Vec<_>>>()?;
            let committed = self
                .0
                .serving
                .commit_embeddings(
                    subject,
                    items,
                    bound
                        .as_ref()
                        .and_then(|bound| bound.historical_authority.as_deref()),
                )
                .await?;
            Ok(k::CommitEmbeddingsResponse { committed })
        }
        .await;
        result.map(Response::new).map_err(status)
    }
    async fn commit_interpretation(
        &self,
        request: Request<k::CommitInterpretationRequest>,
    ) -> std::result::Result<Response<p::DerivedRepresentation>, Status> {
        let input = request.into_inner();
        let result: Result<_> = async {
            if input.text.trim().is_empty() || input.text.len() > 1_048_576 {
                return Err(Error::Invalid(
                    "invalid interpretation proposal bounds".into(),
                ));
            }
            let subject = SubjectId(id(&input.subject_id)?);
            let kind: nous_core::RepresentationKind = enum_value(&input.kind)?;
            let producer = required(input.producer, "producer")?;
            let producer = from_producer(producer)?;
            let inputs = input
                .inputs
                .into_iter()
                .map(|item| {
                    Ok(nous_material::DerivationInput {
                        ordinal: item.ordinal,
                        reference: from_ref(required(item.reference, "input reference")?)?,
                        role: item.role,
                    })
                })
                .collect::<Result<Vec<_>>>()?;
            let operation = input
                .operation_identity
                .map(super::workflow_convert::identity)
                .transpose()?;
            let representation = self
                .0
                .material
                .persist_derived_representation(
                    nous_material::DerivedRepresentation {
                        derived_representation_id: nous_core::DerivedRepresentationId::new(),
                        subject_id: subject,
                        inputs,
                        strategy: input.strategy,
                        representation_kind: kind,
                        producer,
                        revision: 1,
                        payload_text: Some(input.text),
                        payload_json: input.structured_payload.map(|value| object(Some(value))),
                        payload_artifact_id: None,
                        quality: object(input.quality),
                        created_at: self.0.cognition.now(subject),
                        supersedes: input
                            .supersedes
                            .as_deref()
                            .map(id)
                            .transpose()?
                            .map(nous_core::DerivedRepresentationId),
                    },
                    operation.as_ref(),
                )
                .await?;
            Ok(evidence::representation(representation))
        }
        .await;
        result.map(Response::new).map_err(status)
    }
}

impl KernelService {
    fn embedding_prepared_view(
        &self,
        subject: SubjectId,
        token: Option<&str>,
    ) -> Result<Option<nous_runtime::BoundQuery>> {
        let Some(token) = token else { return Ok(None) };
        let bound = self.0.cognition.prepared_query(subject, id(token)?)?;
        if bound.source_query.capabilities.text_embedding
            == nous_core::RequirementStrength::Forbidden
        {
            return Err(Error::Invalid(
                "prepared query forbids embedding material activity".into(),
            ));
        }
        Ok(Some(bound))
    }
}
