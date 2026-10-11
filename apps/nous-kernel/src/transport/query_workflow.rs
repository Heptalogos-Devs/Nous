// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::query_run_adapter::{
    add_degradation, capability_error, host_plan, rpc_status, unavailable_embedding,
    wire_requirement,
};
use super::*;
use futures::Stream;
use nous_core::{Degradation, RequirementStrength, Result};
use nous_retrieval::{QueryEmbedding, TextEmbeddingOutput};
use nous_runtime::{
    BoundQuery, CognitiveContributors, QueryExecution, QueryPlan, QueryRunHostPolicy,
};
use std::pin::Pin;

type QueryUpdates =
    Pin<Box<dyn Stream<Item = std::result::Result<k::QueryRunUpdate, Status>> + Send + 'static>>;

#[tonic::async_trait]
impl k::kernel_query_service_server::KernelQueryService for KernelService {
    type RunQueryStream = QueryUpdates;

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
            let bound = self
                .bind_query_request(required(input.query, "query")?)
                .await?;
            let plan = QueryPlan::for_bound_query(&bound);
            let embedding_text = bound.representation.text.clone();
            let embedding_required = query_embedding_needed(&bound, &plan);
            let inspection = super::query_projection::inspect_bound_query_with_plan(&bound, &plan)?;
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
            let historical_view = bound.read_context.is_historical();
            Ok(p::PreparedQueryResponse {
                bound_query: Some(inspection),
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

    async fn run_query(
        &self,
        request: Request<tonic::Streaming<k::QueryRunRequest>>,
    ) -> std::result::Result<Response<Self::RunQueryStream>, Status> {
        let mut inbound = request.into_inner();
        let start = inbound
            .message()
            .await?
            .ok_or_else(|| Status::invalid_argument("query start frame is required"))?;
        let Some(k::query_run_request::Frame::Start(input)) = start.frame else {
            return Err(Status::invalid_argument(
                "the first query workflow frame must start a query",
            ));
        };

        // Bind and preflight before returning the response stream. Everything
        // after this point lives in that stream's future and drops with it.
        let (bound, plan, host_policy, slot, embedding_handoff) =
            self.start_query_run(input).await.map_err(status)?;
        let first_update = if embedding_handoff {
            Some(
                self.embedding_update(&bound, host_policy)
                    .await
                    .map_err(status)?,
            )
        } else {
            None
        };
        let service = self.clone();
        let updates = async_stream::try_stream! {
            let mut bound = bound;

            'query: {
                if let Some(update) = first_update {
                    yield update;
                    loop {
                        let Some(request) = inbound.message().await? else {
                            break 'query;
                        };
                        match request.frame {
                            Some(k::query_run_request::Frame::EmbeddingNeeds(input)) => {
                                service
                                    .require_historical_embedding(
                                        &bound,
                                        &input.subject_id,
                                        "embedding discovery",
                                    )
                                    .await?;
                                let needs = service
                                    .embedding_needs(input, bound.read_context.historical_view())
                                    .await
                                    .map_err(status)?;
                                yield k::QueryRunUpdate {
                                    frame: Some(k::query_run_update::Frame::EmbeddingNeeds(needs)),
                                };
                            }
                            Some(k::query_run_request::Frame::CommitEmbedding(input)) => {
                                service
                                    .require_historical_embedding(
                                        &bound,
                                        &input.subject_id,
                                        "embedding commit",
                                    )
                                    .await?;
                                service
                                    .commit_embedding(input, bound.read_context.historical_view())
                                    .await
                                    .map_err(status)?;
                                yield k::QueryRunUpdate {
                                    frame: Some(k::query_run_update::Frame::EmbeddingCommitted(())),
                                };
                            }
                            Some(k::query_run_request::Frame::Embedding(input)) => {
                                bound = service.apply_embedding_result(bound, host_policy, input)?;
                                break;
                            }
                            frame => Err(unexpected_query_frame(frame))?,
                        }
                    }
                }

                bound = service
                    .activate_bound_query(bound, &plan)
                    .await
                    .map_err(status)?;

                let concept_host = host_policy.concept_enrichment;
                let concept_requested = !bound.source_query.is_exact_read()
                    && bound.concept_enrichment == nous_runtime::ConceptEnrichment::Model
                    && bound.source_query.capabilities.query_concept_enrichment
                        != RequirementStrength::Forbidden;
                let concept_handoff = concept_requested
                    && concept_host.requirement != RequirementStrength::Forbidden
                    && concept_host.configured;
                if concept_handoff {
                    let update = service
                        .concept_update(&bound, host_policy)
                        .await
                        .map_err(status)?;
                    yield update;
                    let Some(request) = inbound.message().await? else {
                        break 'query;
                    };
                    let input = match request.frame {
                        Some(k::query_run_request::Frame::Concept(input)) => input,
                        frame => Err(unexpected_query_frame(frame))?,
                    };
                    bound = service.apply_concept_result(
                        bound,
                        host_policy,
                        input,
                    )?;
                } else {
                    bound.activation.frozen = true;
                }

                let execution = service
                    .execute_query(bound, &plan, host_policy)
                    .await
                    .map_err(status)?;
                let rerank_expected = rerank_expected(&execution, host_policy);
                let has_resources = !execution.result.resource_actions.is_empty();

                if rerank_expected || has_resources {
                    service
                        .0
                        .cognition
                        .ensure_read_context_current(&execution.bound.read_context)
                        .await
                        .map_err(status)?;
                    service
                        .0
                        .cognition
                        .validate_query_execution_capacity(&execution)
                        .map_err(status)?;
                    let baseline =
                        super::query_projection::query_response(execution.result.clone(), None);
                    let update = k::QueryRunUpdate {
                        frame: Some(k::query_run_update::Frame::Finalize(k::QueryFinalizeWork {
                            baseline: Some(baseline),
                            rerank_query: execution.bound.representation.text.clone(),
                            rerank_candidate_limit: host_policy.rerank_candidate_limit as u32,
                            rerank_expected,
                            rerank_requirement: wire_requirement(host_policy.rerank.requirement),
                        })),
                    };
                    yield update;
                    let Some(request) = inbound.message().await? else {
                        break 'query;
                    };
                    let input = match request.frame {
                        Some(k::query_run_request::Frame::Finalize(input)) => input,
                        frame => Err(unexpected_query_frame(frame))?,
                    };
                    let complete = service
                        .finalize_query_execution(execution, &plan, host_policy, input)
                        .await?;
                    drop(slot);
                    yield complete;
                } else {
                    let completed = service
                        .0
                        .cognition
                        .complete_query_execution(execution)
                        .await
                        .map_err(status)?;
                    let nous_runtime::QueryExecution {
                        read_lease,
                        bound: completed_bound,
                        result,
                        ..
                    } = completed;
                    let inspection = super::query_projection::inspect_bound_query_with_plan(
                        &completed_bound,
                        &plan,
                    )
                    .map_err(status)?;
                    let response =
                        super::query_projection::query_response(result, Some(inspection));
                    service
                        .0
                        .cognition
                        .ensure_read_context_current(&completed_bound.read_context)
                        .await
                        .map_err(status)?;
                    drop(read_lease);
                    drop(completed_bound);
                    drop(slot);
                    yield k::QueryRunUpdate {
                        frame: Some(k::query_run_update::Frame::Complete(response)),
                    };
                }
            }
        };
        let stream: Self::RunQueryStream = Box::pin(updates);
        Ok(Response::new(stream))
    }
}

impl KernelService {
    async fn start_query_run(
        &self,
        input: k::QueryRunStart,
    ) -> Result<(
        BoundQuery,
        QueryPlan,
        QueryRunHostPolicy,
        tokio::sync::OwnedSemaphorePermit,
        bool,
    )> {
        let mut bound = self
            .bind_query_request(required(input.query, "query")?)
            .await?;
        let host = host_plan(input.host);
        let plan = QueryPlan::for_bound_query(&bound);
        let embedding_needed = query_embedding_needed(&bound, &plan);
        let has_embedding_provider = self.0.serving.embedding().is_some();
        let effective_host = QueryRunHostPolicy::for_query(&bound.source_query, host)?;
        let embedding_handoff = embedding_needed
            && effective_host.embedding.requirement != RequirementStrength::Forbidden
            && effective_host.embedding.configured
            && has_embedding_provider;
        if embedding_needed
            && effective_host.embedding.requirement == RequirementStrength::Required
            && !embedding_handoff
        {
            return Err(if effective_host.embedding.configured {
                unavailable_embedding("query_embedding_space")
            } else {
                capability_error("model.query_embedding")
            });
        }
        if embedding_needed
            && effective_host.embedding.requirement != RequirementStrength::Forbidden
            && !embedding_handoff
        {
            bound.activation.degradation.push(Degradation {
                code: "query_embedding_unavailable".into(),
                detail: None,
            });
        }
        let concept_handoff = !bound.source_query.is_exact_read()
            && bound.concept_enrichment == nous_runtime::ConceptEnrichment::Model
            && bound.source_query.capabilities.query_concept_enrichment
                != RequirementStrength::Forbidden
            && effective_host.concept_enrichment.requirement != RequirementStrength::Forbidden
            && effective_host.concept_enrichment.configured;
        if bound.concept_enrichment == nous_runtime::ConceptEnrichment::Model
            && effective_host.concept_enrichment.requirement == RequirementStrength::Required
            && !concept_handoff
        {
            return Err(capability_error("model.query_concept_enrichment"));
        }
        if bound.concept_enrichment == nous_runtime::ConceptEnrichment::Model && !concept_handoff {
            bound.activation.degradation.push(Degradation {
                code: "query_concept_model_unavailable".into(),
                detail: None,
            });
        }
        let limit = bound.config_snapshot.get(nous_runtime::QUERY_SLOTS)?;
        let slot = self.0.cognition.acquire_query_slot(limit)?;
        Ok((bound, plan, effective_host, slot, embedding_handoff))
    }

    async fn bind_query_request(&self, query: p::QueryRequest) -> Result<BoundQuery> {
        let subject = SubjectId(id(&query.subject_id)?);
        let snapshot = self.0.configuration.snapshot_for_subject(subject)?;
        let limit = snapshot.get(nous_runtime::DEFAULT_RESULT_LIMIT)?;
        let mut bound = self
            .0
            .bind_query_with_snapshot(super::query::compile_query(query, limit)?, snapshot)
            .await?;
        bound.selected_embedding_space =
            self.0.serving.embedding().map(|provider| provider.space());
        Ok(bound)
    }

    async fn require_historical_embedding(
        &self,
        bound: &BoundQuery,
        subject_id: &str,
        operation: &str,
    ) -> std::result::Result<(), Status> {
        if !bound.read_context.is_historical()
            || SubjectId(id(subject_id).map_err(status)?) != bound.read_context.subject
        {
            return Err(Status::invalid_argument(format!(
                "{operation} does not match the bound historical query"
            )));
        }
        self.0
            .cognition
            .ensure_read_context_current(&bound.read_context)
            .await
            .map_err(status)
    }

    fn apply_embedding_result(
        &self,
        mut bound: BoundQuery,
        host_policy: QueryRunHostPolicy,
        value: k::QueryEmbeddingResult,
    ) -> std::result::Result<BoundQuery, Status> {
        match value.outcome {
            Some(k::query_embedding_result::Outcome::Material(material)) => {
                let materials = self
                    .query_materials(&bound, vec![material])
                    .map_err(status)?;
                let Some(material) = materials.into_iter().next() else {
                    return Err(status(unavailable_embedding("model.query_embedding")));
                };
                bound.activation.embedding = Some(nous_runtime::QuerySemanticEmbedding {
                    vector: material.output.vector,
                    space: material.output.space,
                    producer: material.output.producer,
                });
            }
            Some(k::query_embedding_result::Outcome::Failure(failure)) => {
                if host_policy.embedding.requirement == RequirementStrength::Required {
                    return Err(rpc_status(failure));
                }
                bound.activation.degradation.push(Degradation {
                    code: "query_embedding_unavailable".into(),
                    detail: None,
                });
            }
            None => {
                return Err(Status::invalid_argument(
                    "query embedding outcome is required",
                ));
            }
        }
        Ok(bound)
    }

    fn apply_concept_result(
        &self,
        mut bound: BoundQuery,
        host_policy: QueryRunHostPolicy,
        value: k::QueryConceptResult,
    ) -> std::result::Result<BoundQuery, Status> {
        match value.outcome {
            Some(k::query_concept_result::Outcome::Output(output)) => {
                if value.model_calls == 0 || value.model_calls > 4 {
                    return Err(Status::invalid_argument(
                        "invalid query concept model call count",
                    ));
                }
                let output = nous_runtime::QueryConceptOutput {
                    existing_tags: output
                        .existing_tags
                        .into_iter()
                        .map(|tag| nous_runtime::QueryConceptSelection {
                            key: tag.key,
                            strength: tag.strength,
                        })
                        .collect(),
                    novel_concepts: output
                        .novel_concepts
                        .into_iter()
                        .map(|item| nous_runtime::NovelConceptHypothesis { text: item.text })
                        .collect(),
                };
                let policy =
                    nous_runtime::ConceptActivationPolicy::from_snapshot(&bound.config_snapshot)
                        .map_err(status)?;
                bound
                    .activation
                    .apply_concept_model(output, &policy)
                    .map_err(status)?;
                bound.activation.model_calls = value.model_calls as usize;
            }
            Some(k::query_concept_result::Outcome::Failure(failure)) => {
                if host_policy.concept_enrichment.requirement == RequirementStrength::Required {
                    return Err(rpc_status(failure));
                }
                bound.activation.model_calls = value.model_calls.min(4) as usize;
                bound.activation.degradation.push(Degradation {
                    code: "query_concept_model_unavailable".into(),
                    detail: None,
                });
            }
            None => {
                return Err(Status::invalid_argument(
                    "query concept outcome is required",
                ));
            }
        }
        bound.activation.frozen = true;
        Ok(bound)
    }

    async fn finalize_query_execution(
        &self,
        mut execution: QueryExecution,
        plan: &QueryPlan,
        host_policy: QueryRunHostPolicy,
        value: k::QueryFinalizeResult,
    ) -> std::result::Result<k::QueryRunUpdate, Status> {
        let rerank_expected = rerank_expected(&execution, host_policy);
        let rerank = value.rerank.unwrap_or_default();
        let mut order = Vec::new();
        match rerank.outcome {
            Some(k::query_rerank_result::Outcome::Order(ordering)) => {
                if !rerank_expected {
                    return Err(Status::invalid_argument(
                        "rerank was not requested for this query result",
                    ));
                }
                for item in ordering.candidates {
                    order.push((
                        from_ref(required(item.reference, "rerank reference").map_err(status)?)
                            .map_err(status)?,
                        item.score,
                    ));
                }
            }
            Some(k::query_rerank_result::Outcome::Failure(failure)) => {
                if rerank_expected
                    && host_policy.rerank.requirement == RequirementStrength::Required
                {
                    return Err(rpc_status(failure));
                }
                if rerank_expected {
                    add_degradation(&mut execution.bound, "query_rerank_unavailable");
                }
            }
            Some(k::query_rerank_result::Outcome::NotRequested(_)) | None => {
                if rerank_expected
                    && host_policy.rerank.requirement == RequirementStrength::Required
                {
                    return Err(status(capability_error("model.query_rerank")));
                }
                if rerank_expected {
                    add_degradation(&mut execution.bound, "query_rerank_unavailable");
                }
            }
        }
        let external_results = value
            .resources
            .into_iter()
            .map(|resource| {
                Ok(nous_core::ExternalResourceResult {
                    action_id: id(&resource.action_id)?,
                    resource_ref: nous_core::ResourceRef::new(resource.resource_ref)?,
                    status: resource.status,
                    records: resource
                        .records
                        .into_iter()
                        .map(super::query_projection::from_resource_record)
                        .collect::<Result<Vec<_>>>()?,
                })
            })
            .collect::<Result<Vec<_>>>()
            .map_err(status)?;
        let inspected =
            super::query_projection::inspect_bound_query_with_plan(&execution.bound, plan)
                .map_err(status)?;
        let subject = execution.bound.read_context.subject;
        let read_context = execution.bound.read_context.clone();
        let contributors = self.contributors(subject).await.map_err(status)?;
        let result = self
            .0
            .cognition
            .finish_query_execution(execution, order, external_results, contributors)
            .await
            .map_err(status)?;
        let response = super::query_projection::query_response(result, Some(inspected));
        self.0
            .cognition
            .ensure_read_context_current(&read_context)
            .await
            .map_err(status)?;
        Ok(k::QueryRunUpdate {
            frame: Some(k::query_run_update::Frame::Complete(response)),
        })
    }

    async fn embedding_update(
        &self,
        bound: &BoundQuery,
        host_policy: QueryRunHostPolicy,
    ) -> Result<k::QueryRunUpdate> {
        self.0
            .cognition
            .ensure_read_context_current(&bound.read_context)
            .await?;
        let config = self.embedding_config()?;
        Ok(k::QueryRunUpdate {
            frame: Some(k::query_run_update::Frame::Embedding(
                k::QueryEmbeddingWork {
                    representation: bound.representation.text.clone(),
                    config: Some(config),
                    historical_view: bound.read_context.is_historical(),
                    effective_requirement: wire_requirement(host_policy.embedding.requirement),
                },
            )),
        })
    }

    async fn activate_bound_query(
        &self,
        mut bound: BoundQuery,
        plan: &QueryPlan,
    ) -> Result<BoundQuery> {
        if !bound.source_query.is_exact_read() && bound.activation_view.is_none() {
            use nous_runtime::SharedLaneProvider;
            let (projection, view, activation) =
                nous_retrieval::with_query_material(Vec::new(), async {
                    let (projection, view) = self.0.serving.prepare_query(&bound, plan).await?;
                    let activation = view.activate(&bound).await?;
                    Ok::<_, nous_core::Error>((projection, view, activation))
                })
                .await?;
            bound.activation = activation;
            bound.activation.degradation.extend(projection.degradation);
            bound.activation_view = Some(view);
        }
        self.0
            .cognition
            .ensure_read_context_current(&bound.read_context)
            .await?;
        Ok(bound)
    }

    async fn concept_update(
        &self,
        bound: &BoundQuery,
        host_policy: QueryRunHostPolicy,
    ) -> Result<k::QueryRunUpdate> {
        self.0
            .cognition
            .ensure_read_context_current(&bound.read_context)
            .await?;
        let representation = super::query_projection::query_representation(&bound.representation);
        let existing_tags = bound
            .activation
            .concept_catalog
            .iter()
            .map(|candidate| p::QueryConceptCandidate {
                key: candidate.key.clone(),
                tag: Some(to_ref(nous_core::CognitiveRef::Tag(candidate.tag))),
                semantic_text: candidate.semantic_text.clone(),
                strength: candidate.strength,
            })
            .collect();
        let temporal_frame = Some(super::query_projection::query_temporal(
            &bound.source_query.temporal_frame,
        ));
        Ok(k::QueryRunUpdate {
            frame: Some(k::query_run_update::Frame::Concept(k::QueryConceptWork {
                representation: Some(representation),
                existing_tags,
                temporal_frame,
                effective_requirement: wire_requirement(host_policy.concept_enrichment.requirement),
            })),
        })
    }

    async fn execute_query(
        &self,
        bound: BoundQuery,
        plan: &QueryPlan,
        host_policy: QueryRunHostPolicy,
    ) -> Result<QueryExecution> {
        let plan = plan.clone();
        let has_text = !bound.representation.text.trim().is_empty();
        let can_rerank = !bound.source_query.is_exact_read()
            && has_text
            && host_policy.rerank.requirement != RequirementStrength::Forbidden
            && host_policy.rerank.configured;
        let pool_limit = can_rerank.then_some(host_policy.rerank_candidate_limit);
        let materials = bound
            .activation
            .embedding
            .as_ref()
            .map(|embedding| {
                vec![QueryEmbedding {
                    text: bound.representation.text.clone(),
                    output: TextEmbeddingOutput {
                        vector: embedding.vector.clone(),
                        space: embedding.space.clone(),
                        producer: embedding.producer.clone(),
                    },
                }]
            })
            .unwrap_or_default();
        let mut execution = Box::pin(nous_retrieval::with_query_material(
            materials,
            self.0
                .execute_bound_query_with_plan(bound, plan, pool_limit),
        ))
        .await?;
        let rerank_candidates = execution
            .result
            .results
            .iter()
            .filter(|hit| {
                hit.representation
                    .as_ref()
                    .is_some_and(|text| !text.trim().is_empty())
            })
            .count();
        if rerank_candidates >= 2
            && host_policy.rerank.requirement == RequirementStrength::Required
            && !host_policy.rerank.configured
        {
            return Err(capability_error("model.query_rerank"));
        }
        let rerank_expected = rerank_expected(&execution, host_policy);
        if !rerank_expected
            && host_policy.rerank.requirement != RequirementStrength::Forbidden
            && !host_policy.rerank.configured
        {
            execution.bound.activation.degradation.push(Degradation {
                code: "query_rerank_unavailable".into(),
                detail: None,
            });
        }
        if rerank_expected
            && host_policy.rerank.requirement == RequirementStrength::Required
            && !host_policy.rerank.configured
        {
            return Err(capability_error("model.query_rerank"));
        }
        Ok(execution)
    }

    async fn contributors(&self, subject: SubjectId) -> Result<CognitiveContributors<'_>> {
        let capabilities = self.0.subjects.subject(subject).await?.capabilities;
        Ok(CognitiveContributors {
            material: Some(&self.0.material),
            shared: None,
            memory: self
                .0
                .memory
                .as_ref()
                .filter(|_| capabilities.memory)
                .map(|owner| owner as &dyn nous_runtime::CognitiveContributor),
        })
    }
}

fn unexpected_query_frame(frame: Option<k::query_run_request::Frame>) -> Status {
    match frame {
        Some(k::query_run_request::Frame::EmbeddingNeeds(_)) => {
            Status::invalid_argument("embedding discovery is only valid during embedding work")
        }
        Some(k::query_run_request::Frame::CommitEmbedding(_)) => {
            Status::invalid_argument("embedding commit is only valid during embedding work")
        }
        Some(k::query_run_request::Frame::Embedding(_)) => {
            Status::invalid_argument("query embedding result is not expected at this stage")
        }
        Some(k::query_run_request::Frame::Concept(_)) => {
            Status::invalid_argument("query concept result is not expected at this stage")
        }
        Some(k::query_run_request::Frame::Finalize(_)) => {
            Status::invalid_argument("query finalization result is not expected at this stage")
        }
        Some(k::query_run_request::Frame::Start(_)) | None => {
            Status::invalid_argument("query workflow continuation frame is required")
        }
    }
}

fn rerank_expected(execution: &QueryExecution, host_policy: QueryRunHostPolicy) -> bool {
    !execution.bound.source_query.is_exact_read()
        && !execution.bound.representation.text.trim().is_empty()
        && host_policy.rerank.requirement != RequirementStrength::Forbidden
        && host_policy.rerank.configured
        && execution
            .result
            .results
            .iter()
            .filter(|hit| {
                hit.representation
                    .as_ref()
                    .is_some_and(|text| !text.trim().is_empty())
            })
            .count()
            >= 2
}

fn query_embedding_needed(bound: &BoundQuery, plan: &QueryPlan) -> bool {
    let need = plan.serving_need(&bound.source_query);
    !bound.source_query.is_exact_read()
        && bound.source_query.capabilities.text_embedding != RequirementStrength::Forbidden
        && !bound.representation.text.is_empty()
        && (need.dense || need.concept_vectors)
}
