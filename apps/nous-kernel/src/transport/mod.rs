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
mod cognition;
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
mod operation;
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
mod query_projection;
mod query_run_adapter;
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

    pub async fn build_contribution_batch(
        &self,
        input: p::ProjectionRequest,
    ) -> nous_core::Result<p::Projection> {
        use nous_core::{AuthorityView, CognitiveEffort, RevisionView, TemporalFrame};
        use nous_runtime::{
            AccessibilityQueryPolicy, AuthorityReadView, ContextResolver, ReadContext,
        };

        let subject = SubjectId(id(&input.subject_id)?);
        let session = SessionId(id(&input.session_id)?);
        let stamp = self.0.store.read_context_stamp(subject).await?;
        let read_policy_snapshot = self.0.configuration.snapshot_for_subject(subject)?;
        let runtime = self.0.cognition.session(subject, session).await?;
        let mut references = self
            .0
            .cognition
            .runtime_references(subject, session)
            .await?
            .into_iter()
            .map(|(reference, _)| reference)
            .collect::<Vec<_>>();
        let situation_refs = input
            .situation_refs
            .into_iter()
            .map(from_ref)
            .collect::<nous_core::Result<Vec<_>>>()?;
        let explicit_references = situation_refs.iter().cloned().collect();
        references.extend(situation_refs);
        let now = self.0.cognition.now(subject);
        let context = ReadContext {
            subject,
            view: AuthorityReadView::Current {
                bound_authority_seq: stamp.authority_seq,
            },
            temporal: TemporalFrame {
                clock_now: now,
                authority_view: AuthorityView::Current,
                revision_view: RevisionView::Current,
                original_expressions: Vec::new(),
            },
            accessibility: AccessibilityQueryPolicy {
                effort: CognitiveEffort::Normal,
                exact_target_bypasses_auto_level: true,
            },
            config_snapshot: std::sync::Arc::new(read_policy_snapshot),
            purge_epoch: stamp.serving_purge_epoch,
        };
        let (contributions, degradation) = self
            .0
            .context_sources(
                &context,
                &references,
                &explicit_references,
                input.max_items as usize,
                input.max_text_bytes as usize,
            )
            .await?;
        let mut segments = Vec::with_capacity(contributions.len());
        let mut wire_degradation = degradation
            .into_iter()
            .map(|value| p::Degradation {
                code: value.code,
                detail: value.detail.unwrap_or_default(),
            })
            .collect::<Vec<_>>();
        for contribution in contributions {
            let Some(text) = contribution.text.filter(|text| !text.is_empty()) else {
                wire_degradation.push(p::Degradation {
                    code: "projection_source_unavailable".into(),
                    detail: contribution.reference.to_string(),
                });
                continue;
            };
            segments.push(p::ContextSegment {
                segment_id: uuid::Uuid::now_v7().to_string(),
                text,
                semantic_role: contribution.semantic_role,
                source_refs: vec![to_ref(contribution.reference)],
                evidence: contribution
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
                authority: serde_json::to_value(contribution.authority)
                    .ok()
                    .and_then(|value| value.as_str().map(str::to_owned))
                    .unwrap_or_else(|| "subject_cognition".into()),
                stability: if contribution.source_revision.is_some() {
                    "revision_bound".into()
                } else {
                    "current_owner_read".into()
                },
                source_revision: contribution.source_revision.map(to_ref),
            });
        }
        self.0
            .cognition
            .ensure_read_context_current(&context)
            .await?;
        Ok(p::Projection {
            projection_id: uuid::Uuid::now_v7().to_string(),
            consumer_id: input.consumer_id,
            source_runtime_revision: runtime.runtime_revision,
            segments,
            degradation: wire_degradation,
        })
    }
}
