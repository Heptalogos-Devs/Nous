// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;
use nous_core::{EvidenceFamily, RequirementStrength, Result};

#[tonic::async_trait]
impl k::kernel_query_service_server::KernelQueryService for KernelService {
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
            let mut bound = self
                .0
                .bind_query_with_snapshot(super::query::compile_query(query, limit)?, snapshot)
                .await?;
            bound.selected_embedding_space =
                self.0.serving.embedding().map(|provider| provider.space());
            let embedding_text = bound.representation.text.clone();
            let embedding_required = bound.source_query.capabilities.text_embedding
                != RequirementStrength::Forbidden
                && !embedding_text.is_empty()
                && (bound.concept_enrichment != nous_runtime::ConceptEnrichment::Off
                    || bound.enabled_lanes.iter().any(|lane| {
                        *lane == EvidenceFamily::Dense
                            || (*lane == EvidenceFamily::TopologyWave
                                && bound
                                    .retrieval_policy
                                    .cognitive_profile
                                    .requirements()
                                    .query_embedding)
                    }));
            let inspection = super::query::inspect_bound_query(&bound)?;
            let text_embedding_requirement =
                enum_name(bound.source_query.capabilities.text_embedding);
            let rerank_requirement = enum_name(if bound.source_query.is_exact_read() {
                RequirementStrength::Forbidden
            } else {
                bound.source_query.capabilities.rerank
            });
            let concept_enrichment_mode = enum_name(bound.concept_enrichment);
            let concept_enrichment_requirement =
                enum_name(bound.source_query.capabilities.query_concept_enrichment);
            let historical_view = bound.historical_authority.is_some();
            let token = if input.reserve_execution {
                let lease = nous_runtime::QueryLease::new(std::time::Duration::from_secs(
                    u64::from(input.lease_seconds),
                ))?;
                let reservation = nous_runtime::QueryReservation::new(bound, lease)?;
                Some(
                    self.0
                        .cognition
                        .retain_prepared_query(reservation)?
                        .to_string(),
                )
            } else {
                None
            };
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
