// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;
use nous_core::{EntityRef, MemoryId, MemoryRevisionId, OperationId, Result, SubjectId};
use nous_memory::{CognitionView, ExplicitMemoryInput, MemoryView, ReviseMemoryInput};

pub(super) fn header(value: &MemoryView) -> p::CognitionHeader {
    super::cognition::cognition_header_proto(&value.header)
}

pub(super) fn revision_header(value: &MemoryView) -> p::CognitionRevisionHeader {
    super::cognition::cognition_revision_header_proto(&value.revision.header)
}

pub(super) fn revision_proto(value: &MemoryView) -> p::MemoryRevision {
    p::MemoryRevision {
        header: Some(revision_header(value)),
        revision_intent: value.revision.revision_intent.map(enum_name),
        details: Some(p::MemoryRevisionDetails {
            formation_mode: enum_name(value.revision.formation_mode),
            grounding_occurrence_id: value
                .revision
                .grounding_occurrence_id
                .map(|id| id.0.to_string()),
            semantic_role: value.revision.semantic_role.clone(),
            text: value.revision.representation_text.clone(),
            title: value.revision.title.clone(),
            basis: value.basis.iter().cloned().map(basis_proto).collect(),
            aboutness: value
                .aboutness
                .iter()
                .map(|item| item.as_str().to_owned())
                .collect(),
            tags: value.tags.iter().map(|item| item.0.to_string()).collect(),
            valid_time: Some(temporal_proto(&value.revision.valid_time)),
            epistemic_class: enum_name(value.revision.epistemic_class),
            temporal_evidence: Some(p::TemporalEvidence {
                occurred: value
                    .temporal_evidence
                    .occurred
                    .iter()
                    .map(temporal_proto)
                    .collect(),
                observed_at: value.temporal_evidence.observed_at.map(timestamp),
            }),
        }),
        relations: value
            .relations
            .iter()
            .map(|relation| p::RevisionRelation {
                from_revision_id: relation.from_revision_id.0.to_string(),
                to_revision_id: relation.to_revision_id.0.to_string(),
                relation: enum_name(relation.relation),
            })
            .collect(),
        relations_truncated: value.relations_truncated,
    }
}

pub(super) fn view(value: MemoryView) -> p::Memory {
    p::Memory {
        header: Some(header(&value)),
        details: Some(p::MemoryDetails {
            cognitive_role: enum_name(value.header.details.memory_role()),
            accessibility_mode: enum_name(value.header.details.accessibility_mode()),
            accessibility_level: enum_name(value.accessibility_level),
        }),
        revision: Some(revision_proto(&value)),
    }
}

fn memory_input(
    subject: SubjectId,
    operation_id: OperationId,
    content: p::MemoryContent,
) -> Result<ExplicitMemoryInput> {
    Ok(ExplicitMemoryInput {
        producer: content.producer.map(from_producer).transpose()?,
        operation_id,
        subject,
        cognitive_role: enum_value(&content.cognitive_role)?,
        formation_mode: enum_value(&content.formation_mode)?,
        grounding_occurrence_id: content.grounding_occurrence_id.as_deref().map(id).transpose()?.map(nous_core::OccurrenceId),
        semantic_role: content.semantic_role,
        representation_text: content.text,
        title: content.title,
        basis: content.basis.into_iter().map(basis).collect::<Result<_>>()?,
        aboutness: content.aboutness.into_iter().map(EntityRef::new).collect::<Result<_>>()?,
        tags: content.tags.into_iter().map(|value| Ok(nous_core::TagId(id(&value)?))).collect::<Result<_>>()?,
        valid_time: temporal(content.valid_time)?,
        epistemic_class: enum_value(&content.epistemic_class).map_err(|error| Error::new(nous_core::ErrorOwner::Memory, nous_core::ErrorCode::InvalidRequest, "MemoryContent.epistemic_class must be observed, reported, derived, inferred, narrative, or simulated").with_source(error))?,
    })
}

impl KernelService {
    async fn memory_mutation_response(
        &self,
        subject: SubjectId,
        result: &nous_memory::MutationResult,
    ) -> Result<p::Memory> {
        let mut views = self
            .require_memory()?
            .read_mutation_cognitions(subject, std::slice::from_ref(result))
            .await?;
        match views.pop() {
            Some(CognitionView::Memory(value)) => Ok(view(value)),
            _ => Err(Error::new(
                nous_core::ErrorOwner::Memory,
                nous_core::ErrorCode::InternalFailure,
                "memory mutation result did not materialize as Memory",
            )),
        }
    }

    pub(super) async fn link_revisions(&self, input: p::LinkRevisionsRequest) -> Result<()> {
        self.require_memory()?
            .link_revisions(
                SubjectId(id(&input.subject_id)?),
                OperationId(id(&input.operation_id)?),
                MemoryRevisionId(id(&input.from_revision_id)?),
                MemoryRevisionId(id(&input.to_revision_id)?),
                enum_value(&input.relation)?,
            )
            .await
    }

    pub(super) async fn form_memory(&self, input: p::FormMemoryRequest) -> Result<p::Memory> {
        let subject = SubjectId(id(&input.subject_id)?);
        let operation = OperationId(id(&input.operation_id)?);
        let content = required(input.input, "input")?;
        let result = self
            .require_memory()?
            .form_memory(memory_input(subject, operation, content)?)
            .await?;
        self.memory_mutation_response(subject, &result).await
    }

    pub(super) async fn revise_memory(&self, input: p::ReviseMemoryRequest) -> Result<p::Memory> {
        let subject = SubjectId(id(&input.subject_id)?);
        let operation_id = OperationId(id(&input.operation_id)?);
        let parsed = memory_input(subject, operation_id, required(input.input, "input")?)?;
        let result = self
            .require_memory()?
            .revise_memory(ReviseMemoryInput {
                producer: parsed.producer,
                operation_id,
                subject,
                memory_id: MemoryId(id(&input.memory_id)?),
                expected_object_epoch: input.expected_object_epoch,
                intent: enum_value(&input.intent).map_err(|_| {
                    Error::new(
                        nous_core::ErrorOwner::Memory,
                        nous_core::ErrorCode::InvalidRequest,
                        "Invalid request",
                    )
                })?,
                formation_mode: parsed.formation_mode,
                grounding_occurrence_id: parsed.grounding_occurrence_id,
                semantic_role: parsed.semantic_role,
                representation_text: parsed.representation_text,
                title: parsed.title,
                basis: parsed.basis,
                aboutness: parsed.aboutness,
                tags: parsed.tags,
                valid_time: parsed.valid_time,
                epistemic_class: parsed.epistemic_class,
            })
            .await?;
        self.memory_mutation_response(subject, &result).await
    }
}
