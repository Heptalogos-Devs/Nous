// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;
use nous_core::{
    CognitiveDomain, EntityRef, RequirementStrength, Result, SourceReadScope, WorkContextId,
};

#[tonic::async_trait]
impl k::kernel_query_service_server::KernelQueryService for KernelService {
    async fn read_sources(
        &self,
        request: Request<p::ReadSourcesRequest>,
    ) -> std::result::Result<Response<p::ReadSourcesResponse>, Status> {
        rpc_reply(self.read_sources_inner(request.into_inner())).await
    }
    async fn trace(
        &self,
        request: Request<p::CognitionTraceRequest>,
    ) -> std::result::Result<Response<p::CognitionTrace>, Status> {
        KernelService::trace(self, request.into_inner())
            .await
            .map(Response::new)
            .map_err(status)
    }
    async fn get_cognitive_time(
        &self,
        request: Request<p::SubjectRequest>,
    ) -> std::result::Result<Response<prost_types::Timestamp>, Status> {
        let result: nous_core::Result<_> = async {
            let subject = SubjectId(id(&request.into_inner().subject_id)?);
            self.0.store.require_subject(subject).await?;
            Ok(timestamp(self.0.cognition.now(subject)))
        }
        .await;
        result.map(Response::new).map_err(status)
    }
    async fn prepare_query(
        &self,
        request: Request<k::PrepareQueryRequest>,
    ) -> std::result::Result<Response<p::PreparedQueryResponse>, Status> {
        let input = request.into_inner();
        let result: Result<_> = async {
            let query = required(input.query, "query")?;
            let subject = SubjectId(id(&query.subject_id)?);
            let snapshot = self.0.configuration.snapshot_for_subject(subject)?;
            let limit = snapshot.get(nous_runtime::DEFAULT_RESULT_LIMIT)?;
            let (social_refs,social_omitted)=self.authorized_query_social_refs(&query).await?;
            let managed_read=query.usage.as_ref().map(|usage|usage.source_read.clone().unwrap_or_default());
            let mut compiled=super::query::compile_query(query,limit)?;
            if let Some(read)=managed_read {
                let scope=SourceReadScope{origin_conversation_refs:read.origin_conversation_refs.clone(),
 subject_id:if read.subject_id.is_empty(){None}else{Some(SubjectId(id(&read.subject_id)?))},source_refs:read.source_refs.into_iter().map(from_ref).collect::<Result<_>>()?};
                compiled.source_read=Some(crate::source_read::SourceRoots{store:self.0.store.clone(),material:self.0.material.clone(),memory:self.0.memory.clone(),self_cognition:self.0.self_cognition.clone()}.view(subject,&scope).await?);
            }
            if let Some(read)=&compiled.source_read {let mut tx=self.0.store.read_view().await?;compiled.source_allowed=Some(nous_persistence::AuthorityStore::source_read_authorized_refs_in(&mut tx,read).await?);}
            let mut bound=self.0.bind_query_with_snapshot_authorized(compiled,snapshot,&social_refs).await?;
            if social_omitted {bound.activation.degradation.push(nous_core::Degradation{code:"social_candidate_omission".into(),detail:Some("Authorized scope exceeds bounded 2048 / 4MiB owner candidate read; query does not assert a complete Social world".into())});}
            bound.selected_embedding_space =
                self.0.serving.embedding().map(|provider| provider.space());
            let embedding_text = bound.representation.text.clone();
            let host = input.host.unwrap_or_default();
            let (embedding_required, needs_host) =
                bound.host_work(&nous_runtime::QueryHostCapabilities {
                    embedding_available: host.embedding_available,
                    embedding_required: host.embedding_required,
                    enrichment_available: host.enrichment_available,
                    enrichment_required: host.enrichment_required,
                    rerank_available: host.rerank_available,
                    rerank_required: host.rerank_required,
                })?;
            let inspection = super::query::inspect_bound_query(&bound)?;
            let text_embedding_requirement =
                enum_name(bound.source_query.capabilities.text_embedding);
            let rerank_requirement = enum_name(if bound.source_query.is_exact_read() {
                RequirementStrength::Forbidden
            } else {
                if host.rerank_required
                    && bound.source_query.capabilities.rerank != RequirementStrength::Forbidden
                {
                    RequirementStrength::Required
                } else {
                    bound.source_query.capabilities.rerank
                }
            });
            let concept_enrichment_mode = enum_name(bound.concept_enrichment);
            let concept_enrichment_requirement =
                enum_name(bound.source_query.capabilities.query_concept_enrichment);
            let historical_view = bound.historical_authority.is_some();
            let (token, execution) = if input.execute_query {
                let lease = nous_runtime::QueryLease::new(std::time::Duration::from_secs(
                    u64::from(input.lease_seconds),
                ))?;
                if needs_host {
                    let reservation = nous_runtime::QueryReservation::new(bound, lease)?;
                    (
                        Some(
                            self.0
                                .cognition
                                .retain_prepared_query(reservation)?
                                .to_string(),
                        ),
                        None,
                    )
                } else {
                    (None, Some(Box::pin(self.query(bound, lease, None)).await?))
                }
            } else {
                (None, None)
            };
            let (result, validation_ticket) = execution
                .map(|value| (value.response, value.validation_ticket))
                .unwrap_or_default();
            Ok(p::PreparedQueryResponse {
                bound_query: inspection,
                preparation_token: token,
                embedding_text,
                embedding_required,
                text_embedding_requirement,
                rerank_requirement,
                concept_enrichment_mode,
                concept_enrichment_requirement,
                historical_view,
                result,
                validation_ticket,
            })
        }
        .await;
        result.map(Response::new).map_err(status)
    }
    async fn activate_query(
        &self,
        request: Request<k::KernelQueryRequest>,
    ) -> std::result::Result<Response<k::QueryActivationResponse>, Status> {
        self.activate_query_with_material(request.into_inner())
            .await
            .map(Response::new)
            .map_err(status)
    }
    async fn query(
        &self,
        request: Request<k::KernelQueryRequest>,
    ) -> std::result::Result<Response<k::KernelQueryResponse>, Status> {
        Box::pin(KernelService::query_with_material(
            self,
            request.into_inner(),
        ))
        .await
        .map(Response::new)
        .map_err(status)
    }
    async fn finalize_query(
        &self,
        request: Request<k::FinalizeQueryRequest>,
    ) -> std::result::Result<Response<p::QueryResponse>, Status> {
        rpc_reply(KernelService::finalize_query(self, request.into_inner())).await
    }
    async fn release_query(
        &self,
        request: Request<k::ReleaseQueryRequest>,
    ) -> std::result::Result<Response<()>, Status> {
        let input = request.into_inner();
        let result: nous_core::Result<_> = self.0.cognition.release_query(
            SubjectId(id(&input.subject_id).map_err(status)?),
            id(&input.validation_ticket).map_err(status)?,
        );
        result.map(Response::new).map_err(status)
    }
}

impl KernelService {
    #[expect(
        clippy::too_many_lines,
        reason = "captured source authorization and bounded owner materialization stay in one read view"
    )]
    async fn read_sources_inner(
        &self,
        input: p::ReadSourcesRequest,
    ) -> Result<p::ReadSourcesResponse> {
        use nous_runtime::ContextResolver;
        let subject = SubjectId(id(&input.subject_id)?);
        if input.references.len() > 128 || input.max_text_bytes > 1024 * 1024 {
            return Err(Error::Invalid(
                "bounded source read requires<=128 refs and<=1MiB".into(),
            ));
        }
        let usage = required(input.usage, "trusted cognition usage")?;
        let read = usage.source_read.clone().unwrap_or_default();
        let scope = nous_runtime::InvocationScope {
            work_context: input
                .work_context_id
                .as_deref()
                .map(id)
                .transpose()?
                .map(WorkContextId),
            source_read: SourceReadScope {
                subject_id: if read.subject_id.is_empty() {
                    None
                } else {
                    Some(SubjectId(id(&read.subject_id)?))
                },
                source_refs: read
                    .source_refs
                    .into_iter()
                    .map(from_ref)
                    .collect::<Result<_>>()?,
                origin_conversation_refs: read.origin_conversation_refs,
            },
            group: usage
                .audience
                .as_ref()
                .and_then(|a| a.group_entity_ref.clone())
                .map(EntityRef::new)
                .transpose()?,
            channel: usage.audience.as_ref().and_then(|a| a.channel_ref.clone()),
            domain: usage.social_domain,
            intent: usage.intent,
            purpose: usage.purpose,
            conversation: usage
                .audience
                .as_ref()
                .map(|a| a.conversation_ref.clone())
                .unwrap_or_default(),
            entities: usage
                .participants
                .into_iter()
                .map(|p| p.entity_ref)
                .chain(
                    usage
                        .audience
                        .into_iter()
                        .flat_map(|a| a.recipient_entity_refs),
                )
                .map(EntityRef::new)
                .collect::<Result<_>>()?,
        };
        let read = self.0.source_read_view(subject, &scope.source_read).await?;
        if scope
            .work_context
            .is_some_and(|id| !read.work_contexts.contains(&id.0))
        {
            return Err(Error::Unavailable(
                "actual WorkContext source not explicitly granted".into(),
            ));
        }
        let refs = input
            .references
            .into_iter()
            .map(from_ref)
            .map(|v| v.map(|r| (r, true)))
            .collect::<Result<Vec<_>>>()?;
        let configuration = self.0.configuration.snapshot_for_subject(subject)?;
        let limit = if input.max_text_bytes == 0 {
            65536
        } else {
            input.max_text_bytes as usize
        };
        let sources = self
            .0
            .context_sources(
                subject,
                &refs,
                limit,
                self.0.cognition.now(subject),
                &configuration,
                Some(&read),
                Some(&scope),
            )
            .await?;
        let mut items = Vec::new();
        let mut bytes = 0;
        let mut omitted = false;
        for source in sources {
            let source = match source {
                Ok(source) => source,
                Err(Error::Unavailable(_) | Error::NotFound(_) | Error::Invalid(_)) => {
                    omitted = true;
                    continue;
                }
                Err(error) => return Err(error),
            };
            let text = source.text.unwrap_or_default();
            if bytes + text.len() > limit {
                omitted = true;
                continue;
            }
            bytes += text.len();
            items.push(p::ContextSegment {
                segment_id: uuid::Uuid::now_v7().to_string(),
                text,
                semantic_role: "requested_source".into(),
                source_refs: vec![to_ref(source.reference.clone())],
                evidence: source
                    .evidence
                    .into_iter()
                    .map(|e| p::Evidence {
                        reference: Some(to_ref(e.reference)),
                        basis_role: e.basis_role,
                        epistemic_relation: e.epistemic_relation.map(|v| v.as_str().into()),
                    })
                    .collect(),
                authority: enum_name(source.authority),
                stability: "request".into(),
                source_revision: source.source_revision,
                usage: source.usage.into(),
                domain: match source.reference.domain() {
                    CognitiveDomain::Memory => "memory",
                    CognitiveDomain::SelfCognition => "self",
                    CognitiveDomain::SocialCognition => "social",
                    _ => "runtime",
                }
                .into(),
                required: false,
            });
        }
        Ok(p::ReadSourcesResponse {
            items,
            degradation: if omitted {
                vec![p::Degradation{code:"source_read_omission".into(),detail:"Requested complete sources unavailable under current source/scope or bounded read; unknown/cyclic/over-budget lineage is withheld".into()}]
            } else {
                Vec::new()
            },
        })
    }
}
