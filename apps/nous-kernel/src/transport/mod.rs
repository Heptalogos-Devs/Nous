//! Private protocol adapters. Domain owners remain the semantic boundary.
// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

#[macro_use]
mod forwarding;
mod canonical_concepts;
mod canonical_identity;
mod canonical_material;
mod canonical_memory;
mod canonical_resource;
mod canonical_runtime;
mod canonical_subject;
mod canonical_system;
mod clock;
mod configuration;
mod consolidation_plan;
mod convert;
mod episode;
mod errors;
mod evidence;
pub use errors::status;
mod hosting;
mod identity;
mod maintenance_workflow;
mod material_workflow;
mod model_result_codec;
mod model_results;
mod projection_workflow;
mod query_workflow;
pub use hosting::router;
mod cognition_trace;
mod concepts;
mod journal;
mod longitudinal;
mod maintenance_plan;
mod management;
mod material;
mod memory;
mod model;
mod motivation;
mod motivation_workflow;
mod query;
mod schema;
mod self_cognition;
mod social_cognition;
mod subject;
mod work_context;

use crate::NousRuntime;
use convert::*;
use nous_core::{Error, SessionId, SubjectId, WorkContextId};
use nous_protocol::{kernel as k, public as p};
use tonic::{Request, Response, Status};

#[derive(Clone)]
pub struct KernelService(pub NousRuntime);

async fn rpc_reply<T>(
    operation: impl std::future::Future<Output = nous_core::Result<T>>,
) -> std::result::Result<Response<T>, Status> {
    operation.await.map(Response::new).map_err(status)
}

impl KernelService {
    pub fn require_memory(&self) -> nous_core::Result<&nous_memory::MemoryService> {
        self.0.require_memory()
    }

    #[expect(
        clippy::too_many_lines,
        reason = "captured source authorization and bounded owner materialization stay in one read view"
    )]
    pub async fn build_contribution_batch(
        &self,
        input: p::ProjectionRequest,
    ) -> nous_core::Result<p::Projection> {
        let subject = SubjectId(id(&input.subject_id)?);
        if input.memory_requirement == "REQUIRED" {
            self.require_memory()?;
            if !self.0.subjects.subject(subject).await?.capabilities.memory {
                return Err(nous_core::DomainError::new(
                    nous_core::DomainErrorCode::CapabilityUnavailable,
                    "Required Subject Memory capability is disabled",
                )
                .with_context("capability", "memory")
                .into());
            }
        }
        if input.self_requirement == "REQUIRED" {
            self.0.require_self()?.require_enabled(subject).await?;
        }
        let requirement = |value: &str| -> nous_core::Result<nous_core::RequirementStrength> {
            Ok(match value {
                "REQUIRED" => nous_core::RequirementStrength::Required,
                "PREFERRED" => nous_core::RequirementStrength::Preferred,
                "OPTIONAL" => nous_core::RequirementStrength::Optional,
                "FORBIDDEN" => nous_core::RequirementStrength::Forbidden,
                _ => return Err(Error::Invalid("invalid contribution requirement".into())),
            })
        };
        let view = self
            .0
            .cognition
            .projection(
                nous_runtime::ProjectionRequest {
                    subject,
                    scope: input
                        .usage
                        .as_ref()
                        .map(|u| {
                            Ok::<_, nous_core::Error>(nous_runtime::InvocationScope {
                                work_context: input
                                    .active_work_context_id
                                    .as_deref()
                                    .map(id)
                                    .transpose()?
                                    .map(WorkContextId),
                                source_read: u
                                    .source_read
                                    .as_ref()
                                    .map(|s| {
                                        Ok::<_, Error>(nous_core::SourceReadScope {
                                            origin_conversation_refs: s
                                                .origin_conversation_refs
                                                .clone(),
                                            subject_id: Some(SubjectId(id(&s.subject_id)?)),
                                            source_refs: s
                                                .source_refs
                                                .clone()
                                                .into_iter()
                                                .map(from_ref)
                                                .collect::<nous_core::Result<_>>()?,
                                        })
                                    })
                                    .transpose()?
                                    .unwrap_or_default(),
                                group: u
                                    .audience
                                    .as_ref()
                                    .and_then(|a| a.group_entity_ref.clone())
                                    .map(nous_core::EntityRef::new)
                                    .transpose()?,
                                channel: u.audience.as_ref().and_then(|a| a.channel_ref.clone()),
                                domain: u.social_domain.clone(),
                                intent: u.intent.clone(),
                                purpose: u.purpose.clone(),
                                conversation: u
                                    .audience
                                    .as_ref()
                                    .map(|a| a.conversation_ref.clone())
                                    .unwrap_or_default(),
                                entities: u
                                    .participants
                                    .iter()
                                    .map(|p| p.entity_ref.clone())
                                    .chain(
                                        u.audience
                                            .iter()
                                            .flat_map(|a| a.recipient_entity_refs.clone()),
                                    )
                                    .map(nous_core::EntityRef::new)
                                    .collect::<nous_core::Result<_>>()?,
                            })
                        })
                        .transpose()?,
                    session: SessionId(id(&input.session_id)?),
                    active_work_context: input
                        .active_work_context_id
                        .as_deref()
                        .map(id)
                        .transpose()?,
                    consumer_id: input.consumer_id,
                    current_materials: input
                        .current_materials
                        .into_iter()
                        .map(convert::material_source)
                        .collect::<nous_core::Result<_>>()?,
                    situation_refs: input
                        .situation_refs
                        .into_iter()
                        .map(from_ref)
                        .collect::<nous_core::Result<_>>()?,
                    max_items: input.max_items as usize,
                    max_text_bytes: input.max_text_bytes as usize,
                    materialize: input.materialize,
                    memory: requirement(&input.memory_requirement)?,
                    self_cognition: if input.self_requirement.is_empty() {
                        nous_core::RequirementStrength::Preferred
                    } else {
                        requirement(&input.self_requirement)?
                    },
                    motivation: if input.motivation_requirement.is_empty() {
                        nous_core::RequirementStrength::Optional
                    } else {
                        requirement(&input.motivation_requirement)?
                    },
                    social_cognition: if input.social_requirement.is_empty() {
                        nous_core::RequirementStrength::Optional
                    } else {
                        requirement(&input.social_requirement)?
                    },
                    runtime: requirement(&input.runtime_requirement)?,
                    resource: requirement(&input.resource_requirement)?,
                },
                &self.0,
            )
            .await?;
        Ok(projection_proto(view, input.usage))
    }
}

fn projection_proto(
    view: nous_runtime::ProjectionView,
    usage: Option<p::CognitionUsage>,
) -> p::Projection {
    p::Projection {
        epistemic_view: None,
        semantic_clock: Some(domain_stamp_proto(&view.clock)),
        projection_id: uuid::Uuid::now_v7().to_string(),
        consumer_id: view.consumer_id,
        source_runtime_revision: view.runtime_revision,
        subject_id: view.subject.0.to_string(),
        session_id: view.session.0.to_string(),
        active_work_context_id: view.active_work_context.map(|id| id.to_string()),
        usage,
        segments: view
            .segments
            .into_iter()
            .map(|segment| p::ContextSegment {
                epistemic_class: segment
                    .source
                    .epistemic_class
                    .map(|class| class.as_str().into()),
                domain_knowledge: segment
                    .source
                    .domain_knowledge
                    .clone()
                    .map(|knowledge| domain_knowledge_proto(*knowledge)),
                temporal: segment
                    .source
                    .temporal
                    .as_ref()
                    .map(temporal_metadata_proto),
                segment_id: uuid::Uuid::now_v7().to_string(),
                domain: match segment.source.reference.domain() {
                    nous_core::CognitiveDomain::Memory => "memory",
                    nous_core::CognitiveDomain::SelfCognition => "self",
                    nous_core::CognitiveDomain::SocialCognition => "social",
                    nous_core::CognitiveDomain::Motivation => "motivation",
                    nous_core::CognitiveDomain::Resource => "resource",
                    _ => "runtime",
                }
                .into(),
                text: segment.source.text.unwrap_or_default(),
                semantic_role: segment.semantic_role.into(),
                source_refs: vec![to_ref(segment.source.reference.clone())],
                evidence: segment
                    .source
                    .evidence
                    .into_iter()
                    .map(|evidence| p::Evidence {
                        epistemic_relation: evidence
                            .epistemic_relation
                            .map(|relation| relation.as_str().into()),
                        reference: Some(to_ref(evidence.reference)),
                        basis_role: evidence.basis_role,
                    })
                    .collect(),
                authority: enum_name(segment.source.authority),
                stability: segment.stability.into(),
                source_revision: segment.source.source_revision,
                usage: segment.source.usage.into(),
                required: segment.source.required,
            })
            .collect(),
        degradation: view
            .degradation
            .into_iter()
            .map(|value| p::Degradation {
                code: value.code,
                detail: value.detail.unwrap_or_default(),
            })
            .collect(),
    }
}
