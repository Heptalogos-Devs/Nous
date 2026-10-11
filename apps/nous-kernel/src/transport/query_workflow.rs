// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::query_run_adapter::{
    add_degradation, capability_error, host_plan, response_for_run, rpc_status,
    unavailable_embedding, wire_requirement,
};
use super::*;
use nous_core::{Degradation, ErrorCode, ErrorOwner, EvidenceFamily, RequirementStrength, Result};
use nous_retrieval::{QueryEmbedding, TextEmbeddingOutput};
use nous_runtime::{CognitiveContributors, QueryLease, QueryRun, QueryRunHandle, QueryRunStage};
use std::time::Duration;

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
            let inspection = super::query_projection::inspect_bound_query(&bound)?;
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
        request: Request<k::QueryRunRequest>,
    ) -> std::result::Result<Response<k::QueryRunUpdate>, Status> {
        let update = match request.into_inner().command {
            Some(k::query_run_request::Command::Start(input)) => {
                Box::pin(self.start_query_run(input))
                    .await
                    .map_err(status)?
            }
            Some(k::query_run_request::Command::Resume(input)) => {
                Box::pin(self.resume_query_run(input)).await?
            }
            Some(k::query_run_request::Command::Cancel(input)) => {
                let subject = SubjectId(id(&input.subject_id).map_err(status)?);
                let handle =
                    query_run_handle(required(input.handle, "query run handle").map_err(status)?)
                        .map_err(status)?;
                self.0
                    .cognition
                    .cancel_query_run(subject, handle)
                    .map_err(status)?;
                k::QueryRunUpdate {
                    handle: None,
                    next: None,
                    cancelled: true,
                }
            }
            None => return Err(Status::invalid_argument("query run command is required")),
        };
        Ok(Response::new(update))
    }
}

impl KernelService {
    async fn start_query_run(&self, input: k::QueryRunStart) -> Result<k::QueryRunUpdate> {
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
        let host = host_plan(input.host);
        let lease = QueryLease::new(Duration::from_secs(if input.lease_seconds == 0 {
            120
        } else {
            u64::from(input.lease_seconds)
        }))?;
        let plan = nous_runtime::QueryPlan::for_bound_query(&bound);
        let need = plan.serving_need(&bound.source_query);
        let embedding_needed = !bound.source_query.is_exact_read()
            && bound.source_query.capabilities.text_embedding != RequirementStrength::Forbidden
            && !bound.representation.text.is_empty()
            && (need.dense || need.concept_vectors);
        let has_embedding_provider = self.0.serving.embedding().is_some();
        let effective_host =
            nous_runtime::QueryRunHostPolicy::for_query(&bound.source_query, host)?;
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
        let first_stage = if embedding_handoff {
            QueryRunStage::Embedding
        } else if concept_handoff {
            QueryRunStage::Concept
        } else {
            QueryRunStage::Finalize
        };
        let run = QueryRun::new(bound, lease, effective_host, first_stage, plan)?;
        if embedding_handoff {
            self.embedding_update(run).await
        } else {
            Box::pin(self.activate_and_advance(run, Vec::new())).await
        }
    }

    async fn resume_query_run(
        &self,
        input: k::QueryRunResume,
    ) -> std::result::Result<k::QueryRunUpdate, Status> {
        let subject = SubjectId(id(&input.subject_id).map_err(status)?);
        let handle = query_run_handle(required(input.handle, "query run handle").map_err(status)?)
            .map_err(status)?;
        let expected_stage = match input.outcome.as_ref() {
            Some(k::query_run_resume::Outcome::Embedding(_)) => QueryRunStage::Embedding,
            Some(k::query_run_resume::Outcome::Concept(_)) => QueryRunStage::Concept,
            Some(k::query_run_resume::Outcome::Finalize(_)) => QueryRunStage::Finalize,
            None => return Err(Status::invalid_argument("query run outcome is required")),
        };
        let mut run = self
            .0
            .cognition
            .take_query_run(subject, handle, expected_stage)
            .map_err(status)?;
        match input.outcome.expect("validated outcome") {
            k::query_run_resume::Outcome::Embedding(value) => {
                match value.outcome {
                    Some(k::query_embedding_result::Outcome::Material(material)) => {
                        let materials = self
                            .query_materials(run.bound(), vec![material])
                            .map_err(status)?;
                        let Some(material) = materials.into_iter().next() else {
                            return Err(status(unavailable_embedding("model.query_embedding")));
                        };
                        run.bound_mut().activation.embedding =
                            Some(nous_runtime::QuerySemanticEmbedding {
                                vector: material.output.vector,
                                space: material.output.space,
                                producer: material.output.producer,
                            });
                    }
                    Some(k::query_embedding_result::Outcome::Failure(failure)) => {
                        if run.host_policy.embedding.requirement == RequirementStrength::Required {
                            return Err(rpc_status(failure));
                        }
                        add_degradation(&mut run, "query_embedding_unavailable");
                    }
                    None => {
                        return Err(Status::invalid_argument(
                            "query embedding outcome is required",
                        ));
                    }
                }
                Box::pin(self.activate_and_advance(run, Vec::new()))
                    .await
                    .map_err(status)
            }
            k::query_run_resume::Outcome::Concept(value) => {
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
                                .map(|item| nous_runtime::NovelConceptHypothesis {
                                    text: item.text,
                                })
                                .collect(),
                        };
                        let policy = nous_runtime::ConceptActivationPolicy::from_snapshot(
                            &run.bound().config_snapshot,
                        )
                        .map_err(status)?;
                        run.bound_mut()
                            .activation
                            .apply_concept_model(output, &policy)
                            .map_err(status)?;
                        run.bound_mut().activation.model_calls = value.model_calls as usize;
                    }
                    Some(k::query_concept_result::Outcome::Failure(failure)) => {
                        if run.host_policy.concept_enrichment.requirement
                            == RequirementStrength::Required
                        {
                            return Err(rpc_status(failure));
                        }
                        run.bound_mut().activation.model_calls = value.model_calls.min(4) as usize;
                        add_degradation(&mut run, "query_concept_model_unavailable");
                    }
                    None => {
                        return Err(Status::invalid_argument(
                            "query concept outcome is required",
                        ));
                    }
                }
                run.bound_mut().activation.frozen = true;
                Box::pin(self.execute_and_advance(run))
                    .await
                    .map_err(status)
            }
            k::query_run_resume::Outcome::Finalize(value) => {
                let rerank = value.rerank.unwrap_or_default();
                let rerank_expected = run.execution().is_some_and(|execution| {
                    !execution.bound.source_query.is_exact_read()
                        && !execution.bound.representation.text.trim().is_empty()
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
                });
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
                                from_ref(
                                    required(item.reference, "rerank reference").map_err(status)?,
                                )
                                .map_err(status)?,
                                item.score,
                            ));
                        }
                    }
                    Some(k::query_rerank_result::Outcome::Failure(failure)) => {
                        if rerank_expected
                            && run.host_policy.rerank.requirement == RequirementStrength::Required
                        {
                            return Err(rpc_status(failure));
                        }
                        if rerank_expected {
                            add_degradation(&mut run, "query_rerank_unavailable");
                        }
                    }
                    Some(k::query_rerank_result::Outcome::NotRequested(_)) | None => {
                        if rerank_expected
                            && run.host_policy.rerank.requirement == RequirementStrength::Required
                        {
                            return Err(status(capability_error("model.query_rerank")));
                        }
                        if rerank_expected {
                            add_degradation(&mut run, "query_rerank_unavailable");
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
                let bound = run.bound().clone();
                let plan = run.plan().clone();
                let contributors = self.contributors(subject).await.map_err(status)?;
                let result = self
                    .0
                    .cognition
                    .finish_query_run(run, order, external_results, contributors)
                    .await
                    .map_err(status)?;
                let response = response_for_run(result, &bound, &plan).map_err(status)?;
                self.0
                    .cognition
                    .ensure_read_context_current(&bound.read_context)
                    .await
                    .map_err(status)?;
                Ok(k::QueryRunUpdate {
                    handle: None,
                    next: Some(k::query_run_update::Next::Complete(response)),
                    cancelled: false,
                })
            }
        }
    }

    async fn embedding_update(&self, mut run: QueryRun) -> Result<k::QueryRunUpdate> {
        self.0
            .cognition
            .ensure_read_context_current(&run.bound().read_context)
            .await?;
        let config = self.embedding_config()?;
        let representation = run.bound().representation.text.clone();
        let historical_view = run.bound().read_context.is_historical();
        let effective_requirement = wire_requirement(run.host_policy.embedding.requirement);
        run.set_stage(QueryRunStage::Embedding);
        let handle = self.0.cognition.retain_query_run(run)?;
        let update = k::QueryEmbeddingWork {
            representation,
            config: Some(config),
            historical_view,
            effective_requirement,
        };
        Ok(k::QueryRunUpdate {
            handle: Some(wire_query_run_handle(handle)),
            next: Some(k::query_run_update::Next::Embedding(update)),
            cancelled: false,
        })
    }

    async fn activate_and_advance(
        &self,
        mut run: QueryRun,
        materials: Vec<QueryEmbedding>,
    ) -> Result<k::QueryRunUpdate> {
        if !run.bound().source_query.is_exact_read() && run.bound().activation_view.is_none() {
            let plan = run.plan().clone();
            let bound = run.bound_mut();
            use nous_runtime::SharedLaneProvider;
            let (projection, view, activation) =
                nous_retrieval::with_query_material(materials, async {
                    let (projection, view) = self.0.serving.prepare_query(bound, &plan).await?;
                    let activation = view.activate(bound).await?;
                    Ok::<_, nous_core::Error>((projection, view, activation))
                })
                .await?;
            bound.activation = activation;
            bound.activation.degradation.extend(projection.degradation);
            bound.activation_view = Some(view);
        }

        self.0
            .cognition
            .ensure_read_context_current(&run.bound().read_context)
            .await?;
        let concept_requested = !run.bound().source_query.is_exact_read()
            && run.bound().concept_enrichment == nous_runtime::ConceptEnrichment::Model
            && run
                .bound()
                .source_query
                .capabilities
                .query_concept_enrichment
                != RequirementStrength::Forbidden;
        if concept_requested
            && run.host_policy.concept_enrichment.requirement != RequirementStrength::Forbidden
            && run.host_policy.concept_enrichment.configured
        {
            return self.concept_update(run).await;
        }
        run.bound_mut().activation.frozen = true;
        Box::pin(self.execute_and_advance(run)).await
    }

    async fn concept_update(&self, mut run: QueryRun) -> Result<k::QueryRunUpdate> {
        let bound = run.bound();
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
        let requirement = wire_requirement(run.host_policy.concept_enrichment.requirement);
        run.set_stage(QueryRunStage::Concept);
        let handle = self.0.cognition.retain_query_run(run)?;
        Ok(k::QueryRunUpdate {
            handle: Some(wire_query_run_handle(handle)),
            next: Some(k::query_run_update::Next::Concept(k::QueryConceptWork {
                representation: Some(representation),
                existing_tags,
                temporal_frame,
                effective_requirement: requirement,
            })),
            cancelled: false,
        })
    }

    async fn execute_and_advance(&self, mut run: QueryRun) -> Result<k::QueryRunUpdate> {
        let bound = run.bound();
        let has_text = !bound.representation.text.trim().is_empty();
        let candidate_limit = run.host_policy.rerank_candidate_limit;
        let can_rerank = !bound.source_query.is_exact_read()
            && has_text
            && run.host_policy.rerank.requirement != RequirementStrength::Forbidden
            && run.host_policy.rerank.configured;
        let pool_limit = can_rerank.then_some(candidate_limit);
        let plan = run.plan().clone();
        let bound = run.bound().clone();
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
        let execution = Box::pin(nous_retrieval::with_query_material(
            materials,
            self.0
                .execute_bound_query_with_plan(bound, plan, pool_limit),
        ))
        .await?;
        run.set_execution(execution);
        let result = &run
            .execution()
            .ok_or_else(|| {
                nous_core::Error::new(
                    ErrorOwner::Runtime,
                    ErrorCode::InternalFailure,
                    "Query execution result is unavailable",
                )
            })?
            .result;
        let rerank_candidates = result
            .results
            .iter()
            .filter(|hit| {
                hit.representation
                    .as_ref()
                    .is_some_and(|text| !text.trim().is_empty())
            })
            .count();
        if rerank_candidates >= 2
            && run.host_policy.rerank.requirement == RequirementStrength::Required
            && !run.host_policy.rerank.configured
        {
            return Err(capability_error("model.query_rerank"));
        }
        let rerank_expected = can_rerank && rerank_candidates >= 2;
        if !rerank_expected
            && run.host_policy.rerank.requirement != RequirementStrength::Forbidden
            && !run.host_policy.rerank.configured
        {
            add_degradation(&mut run, "query_rerank_unavailable");
        }
        let has_resources = !run
            .execution()
            .expect("execution retained")
            .result
            .resource_actions
            .is_empty();
        if rerank_expected || has_resources {
            self.0
                .cognition
                .ensure_read_context_current(&run.bound().read_context)
                .await?;
            let execution = run.execution().expect("execution retained");
            let baseline = super::query_projection::query_response(execution.result.clone());
            let rerank_query = run.bound().representation.text.clone();
            let requirement = wire_requirement(run.host_policy.rerank.requirement);
            run.set_stage(QueryRunStage::Finalize);
            let candidate_limit = run.host_policy.rerank_candidate_limit as u32;
            let handle = self.0.cognition.retain_query_run(run)?;
            Ok(k::QueryRunUpdate {
                handle: Some(wire_query_run_handle(handle)),
                next: Some(k::query_run_update::Next::Finalize(k::QueryFinalizeWork {
                    baseline: Some(baseline),
                    rerank_query,
                    rerank_candidate_limit: candidate_limit,
                    rerank_expected,
                    rerank_requirement: requirement,
                })),
                cancelled: false,
            })
        } else {
            let bound = run.bound().clone();
            let plan = run.plan().clone();
            let result = self.0.cognition.complete_query_run(run).await?;
            let response = response_for_run(result, &bound, &plan)?;
            self.0
                .cognition
                .ensure_read_context_current(&bound.read_context)
                .await?;
            Ok(k::QueryRunUpdate {
                handle: None,
                next: Some(k::query_run_update::Next::Complete(response)),
                cancelled: false,
            })
        }
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

fn query_run_handle(value: k::QueryRunHandle) -> Result<QueryRunHandle> {
    Ok(QueryRunHandle {
        query_run_id: id(&value.query_run_id)?,
        revision: value.revision,
    })
}

fn wire_query_run_handle(value: QueryRunHandle) -> k::QueryRunHandle {
    k::QueryRunHandle {
        query_run_id: value.query_run_id.to_string(),
        revision: value.revision,
    }
}
