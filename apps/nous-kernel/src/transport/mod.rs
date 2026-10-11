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
mod model_workflow;
mod projection_workflow;
mod query_workflow;
mod workflow_convert;
pub use hosting::router;
mod concepts;
mod journal;
mod longitudinal;
mod maintenance_plan;
mod management;
mod material;
mod memory;
mod model;
mod query;
mod schema;
mod subject;
mod work_context;

use crate::NousRuntime;
use convert::*;
use nous_core::{Error, SessionId, SubjectId};
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
        reason = "projection coordination keeps reference origin, current owner validation and bounded materialization in one boundary"
    )]
    pub async fn build_contribution_batch(
        &self,
        input: p::ProjectionRequest,
    ) -> nous_core::Result<p::Projection> {
        let subject = SubjectId(id(&input.subject_id)?);
        let session = SessionId(id(&input.session_id)?);
        let runtime = self.0.cognition.session(subject, session).await?;
        let mut segments = self
            .0
            .cognition
            .runtime_references(subject, session)
            .await?
            .into_iter()
            .map(|(reference, source)| p::ContextSegment {
                segment_id: uuid::Uuid::now_v7().to_string(),
                text: String::new(),
                semantic_role: source.into(),
                source_refs: vec![to_ref(reference.clone())],
                evidence: Vec::new(),
                authority: "subject_cognition".into(),
                stability: "checkpoint".into(),
                source_revision: Some(reference.to_string()),
            })
            .collect::<Vec<_>>();
        let situation_refs = input
            .situation_refs
            .into_iter()
            .map(from_ref)
            .collect::<nous_core::Result<Vec<_>>>()?;
        segments.extend(
            situation_refs
                .into_iter()
                .map(|reference| p::ContextSegment {
                    segment_id: uuid::Uuid::now_v7().to_string(),
                    text: String::new(),
                    semantic_role: "request_situation".into(),
                    source_refs: vec![to_ref(reference.clone())],
                    evidence: Vec::new(),
                    authority: "external_current_authority".into(),
                    stability: "request".into(),
                    source_revision: Some(reference.to_string()),
                }),
        );
        let mut remaining = input.max_text_bytes as usize;
        let mut degradation = Vec::new();
        let mut eligible_segments = Vec::new();
        for mut segment in segments {
            let Some(reference) = segment.source_refs.first() else {
                continue;
            };
            let reference = from_ref(reference.clone())?;
            match self.0.store.validate_reference(subject, &reference).await {
                Ok(()) => {}
                Err(Error::Invalid(_) | Error::NotFound(_)) => {
                    degradation.push(p::Degradation {
                        code: "projection_source_unavailable".into(),
                        detail: reference.to_string(),
                    });
                    continue;
                }
                Err(error) => return Err(error),
            }
            if matches!(
                reference,
                nous_core::CognitiveRef::Memory(_)
                    | nous_core::CognitiveRef::MemoryRevision(_)
                    | nous_core::CognitiveRef::CognitiveSchema(_)
                    | nous_core::CognitiveRef::CognitiveSchemaRevision(_)
                    | nous_core::CognitiveRef::Episode(_)
                    | nous_core::CognitiveRef::EpisodeRevision(_)
                    | nous_core::CognitiveRef::Journal(_)
                    | nous_core::CognitiveRef::JournalRevision(_)
            ) {
                if !self
                    .require_memory()?
                    .contextual_cognition_eligible(subject, &reference)
                    .await?
                {
                    degradation.push(p::Degradation {
                        code: "projection_source_unavailable".into(),
                        detail: reference.to_string(),
                    });
                    continue;
                }
                segment.authority = "subject_cognition".into();
            }
            if !matches!(
                reference,
                nous_core::CognitiveRef::Memory(_) | nous_core::CognitiveRef::MemoryRevision(_)
            ) {
                eligible_segments.push(segment);
                continue;
            }
            match nous_runtime::ContextResolver::context_source(
                &self.0, subject, &reference, remaining, true,
            )
            .await
            {
                Ok(source) => {
                    segment.text = source.text.unwrap_or_default();
                    remaining = remaining.saturating_sub(segment.text.len());
                    segment.evidence = source
                        .evidence
                        .into_iter()
                        .map(|evidence| p::Evidence {
                            epistemic_relation: evidence
                                .epistemic_relation
                                .map(|relation| relation.as_str().into()),
                            reference: Some(to_ref(evidence.reference)),
                            basis_role: evidence.basis_role,
                        })
                        .collect();
                    segment.source_revision = source
                        .source_revision
                        .map(|revision| revision.0.to_string());
                    segment.authority = "subject_cognition".into();
                }
                Err(_) => {
                    degradation.push(p::Degradation {
                        code: "projection_source_unavailable".into(),
                        detail: reference.to_string(),
                    });
                    continue;
                }
            }
            eligible_segments.push(segment);
        }
        Ok(p::Projection {
            projection_id: uuid::Uuid::now_v7().to_string(),
            consumer_id: input.consumer_id,
            source_runtime_revision: runtime.runtime_revision,
            segments: eligible_segments,
            degradation,
        })
    }
}
