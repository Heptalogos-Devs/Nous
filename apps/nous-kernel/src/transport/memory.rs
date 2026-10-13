// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;
use nous_core::{CognitiveRef, EntityRef, OperationId, Result, SubjectId};
use nous_memory::ExplicitMemoryInput;
use nous_memory::{MemoryView, ReviseMemoryInput};

pub(super) fn view(input: MemoryView) -> p::Memory {
    let object = input.object;
    let revision = input.revision;
    p::Memory {
        producer_signature_id: revision.producer_signature_id.map(|id| id.to_string()),
        memory_id: object.memory_id.0.to_string(),
        revision_id: revision.memory_revision_id.0.to_string(),
        subject_id: object.subject_id.0.to_string(),
        cognitive_role: enum_name(object.cognitive_role),
        object_epoch: object.object_epoch,
        acceptance_state: enum_name(object.acceptance_state),
        integrity_state: enum_name(object.integrity_state),
        suppression_state: enum_name(object.suppression_state),
        accessibility_mode: enum_name(object.accessibility_mode),
        accessibility_level: enum_name(input.accessibility_level),
        semantic_role: revision.semantic_role,
        epistemic_class: enum_name(revision.epistemic_class),
        grounding_occurrence_id: revision.grounding_occurrence_id.map(|id| id.0.to_string()),
        text: revision.representation_text,
        title: revision.title,
        basis: input.basis.into_iter().map(basis_proto).collect(),
        aboutness: input
            .aboutness
            .into_iter()
            .map(|v| v.as_str().to_owned())
            .collect(),
        tags: input.tags.into_iter().map(|v| v.0.to_string()).collect(),
        created_at: Some(timestamp(object.created_at)),
        temporal_evidence: Some(p::TemporalEvidence {
            occurred: input
                .temporal_evidence
                .occurred
                .iter()
                .map(temporal_proto)
                .collect(),
            observed_at: input.temporal_evidence.observed_at.map(timestamp),
        }),
        valid_time: Some(temporal_proto(&revision.valid_time)),
        formed_at: Some(timestamp(revision.formed_at)),
        recorded_at: Some(timestamp(revision.recorded_at)),
        formation_mode: enum_name(revision.formation_mode),
        revision_no: revision.revision_no,
        relations: input
            .relations
            .into_iter()
            .map(|value| p::RevisionRelation {
                from_revision_id: value.from_revision_id.0.to_string(),
                to_revision_id: value.to_revision_id.0.to_string(),
                relation: enum_name(value.relation),
            })
            .collect(),
        relations_truncated: input.relations_truncated,
        revision_intent: revision.revision_intent.map(enum_name),
    }
}

fn memory_content(content: p::MemoryContent) -> Result<nous_memory::MemoryContent> {
    Ok(nous_memory::MemoryContent {
        producer: content.producer.map(from_producer).transpose()?,
        formation_mode: enum_value(&content.formation_mode)?,
        grounding_occurrence_id: content
            .grounding_occurrence_id
            .as_deref()
            .map(id)
            .transpose()?
            .map(nous_core::OccurrenceId),
        semantic_role: content.semantic_role,
        representation_text: content.text,
        title: content.title,
        basis: content
            .basis
            .into_iter()
            .map(basis)
            .collect::<Result<_>>()?,
        aboutness: content
            .aboutness
            .into_iter()
            .map(EntityRef::new)
            .collect::<Result<_>>()?,
        tags: content
            .tags
            .into_iter()
            .map(|value| Ok(nous_core::TagId(id(&value)?)))
            .collect::<Result<_>>()?,
        valid_time: temporal(content.valid_time)?,
        epistemic_class: enum_value(&content.epistemic_class).map_err(|_| Error::Invalid("MemoryContent.epistemic_class must be observed, reported, derived, inferred, narrative, or simulated".into()))?,
    })
}

impl KernelService {
    pub(super) async fn trace(&self, input: p::CognitionTraceRequest) -> Result<p::CognitionTrace> {
        let subject = SubjectId(id(&input.subject_id)?);
        let reference = from_ref(required(input.reference, "reference")?)?;
        let max_nodes = if input.max_nodes == 0 {
            64
        } else {
            input.max_nodes as usize
        };
        let max_bytes = if input.max_bytes == 0 {
            65536
        } else {
            input.max_bytes as usize
        };
        let trace = self
            .0
            .require_memory()?
            .trace(subject, reference.clone(), max_nodes, max_bytes)
            .await?;
        let remaining_nodes = max_nodes.saturating_sub(trace.nodes.len());
        let remaining_bytes = max_bytes.saturating_sub(trace.text_bytes);
        let has_material = !trace.material_roots.is_empty();
        let lineage = if has_material && remaining_nodes > 0 && remaining_bytes > 0 {
            Some(
                self.read_lineage(p::MaterialLineageRequest {
                    subject_id: subject.0.to_string(),
                    roots: trace.material_roots.into_iter().map(to_ref).collect(),
                    max_nodes: remaining_nodes as u32,
                    max_bytes: remaining_bytes as u32,
                })
                .await?,
            )
        } else {
            None
        };
        let producers = self
            .0
            .material
            .producer_signatures(&trace.producers)
            .await?
            .into_iter()
            .map(evidence::producer)
            .collect();
        let nodes = trace
            .nodes
            .into_iter()
            .map(|node| {
                use nous_memory::CognitionContent as N;
                use p::cognition_trace_node::Content as C;
                let (reference, content) = match node {
                    N::Memory(v) => (
                        CognitiveRef::MemoryRevision(v.revision.memory_revision_id),
                        C::Memory(view(v)),
                    ),
                    N::Schema(v) => (
                        CognitiveRef::CognitiveSchemaRevision(v.revision.schema_revision_id),
                        C::Schema(super::schema::schema_view(v)),
                    ),
                    N::Episode(v) => (
                        CognitiveRef::EpisodeRevision(v.revision.episode_revision_id),
                        C::Episode(super::episode::revision(
                            v.revision,
                            v.members,
                            v.basis,
                            v.relations,
                        )),
                    ),
                    N::Journal(v) => (
                        CognitiveRef::JournalRevision(v.revision.journal_revision_id),
                        C::Journal(super::journal::journal_revision_proto(v)),
                    ),
                    N::Tag { .. } => unreachable!("trace has only exact cognition revisions"),
                };
                p::CognitionTraceNode {
                    reference: Some(to_ref(reference)),
                    content: Some(content),
                }
            })
            .collect();
        Ok(p::CognitionTrace {
            reference: Some(to_ref(reference)),
            nodes,
            producers,
            truncated: trace.truncated
                || (has_material && lineage.is_none())
                || lineage
                    .as_ref()
                    .is_some_and(|l| l.truncated || l.payloads_truncated),
            lineage,
        })
    }

    pub(super) async fn set_accessibility(
        &self,
        input: p::SetAccessibilityRequest,
    ) -> Result<p::Memory> {
        let subject = SubjectId(id(&input.subject_id)?);
        Ok(view(
            self.require_memory()?
                .set_accessibility(
                    subject,
                    nous_core::MemoryId(id(&input.memory_id)?),
                    OperationId(id(&input.operation_id)?),
                    input.expected_object_epoch,
                    enum_value(&input.mode)?,
                )
                .await?,
        ))
    }
    pub(super) async fn link_revisions(&self, input: p::LinkRevisionsRequest) -> Result<()> {
        self.require_memory()?
            .link_revisions(
                SubjectId(id(&input.subject_id)?),
                OperationId(id(&input.operation_id)?),
                nous_core::MemoryRevisionId(id(&input.from_revision_id)?),
                nous_core::MemoryRevisionId(id(&input.to_revision_id)?),
                enum_value(&input.relation)?,
            )
            .await
    }
    pub(super) async fn get_memory(&self, input: p::ObjectRequest) -> Result<p::Memory> {
        Ok(view(
            self.require_memory()?
                .memory(
                    SubjectId(id(&input.subject_id)?),
                    nous_core::MemoryId(id(&input.id)?),
                    None,
                )
                .await?,
        ))
    }
    pub(super) async fn list_memories(
        &self,
        input: p::ListRequest,
    ) -> Result<p::ListMemoriesResponse> {
        let subject = SubjectId(id(&input.subject_id)?);
        let scope = format!("memories:{}:{}", input.subject_id, input.status);
        let (limit, last) = page(input.page, &scope)?;
        let (items, more) = self
            .require_memory()?
            .memories(subject, &input.status, limit, last)
            .await?;
        let next_page_token = if more {
            next_token(
                &scope,
                items
                    .last()
                    .ok_or_else(|| Error::Internal("empty page".into()))?
                    .object
                    .memory_id
                    .0,
            )
        } else {
            String::new()
        };
        Ok(p::ListMemoriesResponse {
            items: items.into_iter().map(view).collect(),
            next_page_token,
        })
    }
    pub(super) async fn get_memory_revision(&self, input: p::ObjectRequest) -> Result<p::Memory> {
        let subject = SubjectId(id(&input.subject_id)?);
        Ok(view(
            self.require_memory()?
                .revision(subject, nous_core::MemoryRevisionId(id(&input.id)?))
                .await?,
        ))
    }
    pub(super) async fn list_memory_revisions(
        &self,
        input: p::MemoryHistoryRequest,
    ) -> Result<p::ListMemoriesResponse> {
        let subject = SubjectId(id(&input.subject_id)?);
        let memory = nous_core::MemoryId(id(&input.memory_id)?);
        let scope = format!("memory-history:{}:{}", subject.0, memory.0);
        let (limit, after) = page(input.page, &scope)?;
        let (history, more) = self
            .require_memory()?
            .memory_history(subject, memory, limit, after)
            .await?;
        let next_page_token = if more {
            next_token(
                &scope,
                history
                    .last()
                    .ok_or_else(|| Error::Internal("empty page".into()))?
                    .revision
                    .memory_revision_id
                    .0,
            )
        } else {
            String::new()
        };
        Ok(p::ListMemoriesResponse {
            items: history.into_iter().map(view).collect(),
            next_page_token,
        })
    }
    pub(super) async fn form_memory(&self, input: p::FormMemoryRequest) -> Result<p::Memory> {
        let subject = SubjectId(id(&input.subject_id)?);
        let content = required(input.input, "input")?;
        Ok(view(
            self.require_memory()?
                .form_memory(ExplicitMemoryInput {
                    subject,
                    operation_id: OperationId(id(&input.operation_id)?),
                    cognitive_role: enum_value(&input.cognitive_role)?,
                    content: memory_content(content)?,
                })
                .await?,
        ))
    }
    pub(super) async fn revise_memory(&self, input: p::ReviseMemoryRequest) -> Result<p::Memory> {
        let subject = SubjectId(id(&input.subject_id)?);
        let content = required(input.input, "input")?;
        let operation_id = OperationId(id(&input.operation_id)?);
        Ok(view(
            self.require_memory()?
                .revise_memory(ReviseMemoryInput {
                    operation_id,
                    subject,
                    memory_id: nous_core::MemoryId(id(&input.memory_id)?),
                    expected_object_epoch: input.expected_object_epoch,
                    intent: enum_value(&input.intent).map_err(|_| {
                        Error::Invalid(
                            "Memory revision intent must be correct, rephrase, or reinterpret"
                                .into(),
                        )
                    })?,

                    content: memory_content(content)?,
                })
                .await?,
        ))
    }
    async fn memory_lifecycle(
        &self,
        input: p::MemoryMutationRequest,
        action: nous_memory::LifecycleAction,
    ) -> Result<p::Memory> {
        Ok(view(
            self.require_memory()?
                .mutate_memory_lifecycle(
                    SubjectId(id(&input.subject_id)?),
                    nous_core::MemoryId(id(&input.memory_id)?),
                    OperationId(id(&input.operation_id)?),
                    input.expected_object_epoch,
                    action,
                )
                .await?,
        ))
    }
    pub(super) async fn suppress_memory(
        &self,
        input: p::MemoryMutationRequest,
    ) -> Result<p::Memory> {
        self.memory_lifecycle(input, nous_memory::LifecycleAction::Suppress)
            .await
    }
    pub(super) async fn restore_memory(
        &self,
        input: p::MemoryMutationRequest,
    ) -> Result<p::Memory> {
        self.memory_lifecycle(input, nous_memory::LifecycleAction::Restore)
            .await
    }
    pub(super) async fn withdraw_memory(
        &self,
        input: p::MemoryMutationRequest,
    ) -> Result<p::Memory> {
        self.memory_lifecycle(input, nous_memory::LifecycleAction::Withdraw)
            .await
    }
    pub(super) async fn reaccept_memory(
        &self,
        input: p::MemoryMutationRequest,
    ) -> Result<p::Memory> {
        self.memory_lifecycle(input, nous_memory::LifecycleAction::Reaccept)
            .await
    }
    pub(super) async fn purge_memory(&self, input: p::MemoryMutationRequest) -> Result<()> {
        self.require_memory()?
            .purge_memory(
                SubjectId(id(&input.subject_id)?),
                nous_core::MemoryId(id(&input.memory_id)?),
                OperationId(id(&input.operation_id)?),
                input.expected_object_epoch,
            )
            .await
    }
}
