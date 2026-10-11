// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;
use nous_core::{CognitiveRef, OperationId, Result, SubjectId};
use nous_memory::{CognitionAction, CognitionKind, CognitionView};
use uuid::Uuid;

pub(super) fn cognition_header_proto(value: &nous_memory::CognitionHeader) -> p::CognitionHeader {
    p::CognitionHeader {
        object_ref: Some(to_ref(value.object_ref.clone())),
        subject_id: value.subject_id.0.to_string(),
        current_revision_ref: Some(to_ref(value.current_revision_ref.clone())),
        object_epoch: value.object_epoch,
        acceptance_state: enum_name(value.lifecycle.acceptance),
        integrity_state: enum_name(value.lifecycle.integrity),
        suppression_state: enum_name(value.lifecycle.suppression),
        purge_state: enum_name(value.lifecycle.purge),
        created_at: Some(timestamp(value.created_at)),
    }
}

pub(super) fn cognition_revision_header_proto(
    value: &nous_memory::CognitionRevisionHeader,
) -> p::CognitionRevisionHeader {
    p::CognitionRevisionHeader {
        revision_ref: Some(to_ref(value.revision_ref.clone())),
        object_ref: Some(to_ref(value.object_ref.clone())),
        revision_no: value.revision_no,
        parent_revision_ref: value.parent_revision_ref.clone().map(to_ref),
        formed_at: Some(timestamp(value.formed_at)),
        recorded_at: Some(timestamp(value.recorded_at)),
        producer_signature_id: value.producer_signature_id.map(|id| id.to_string()),
    }
}

fn cognition_result(view: CognitionView) -> p::CognitionResult {
    use p::cognition_result::Result as Typed;
    let result = match view {
        CognitionView::Memory(value) => Typed::Memory(super::memory::view(value)),
        CognitionView::CognitiveSchema(value) => {
            Typed::CognitiveSchema(super::schema::schema_view(value))
        }
        CognitionView::Episode(value) => Typed::Episode(super::episode::view(value)),
        CognitionView::Journal(value) => Typed::Journal(super::journal::view(value)),
    };
    p::CognitionResult {
        result: Some(result),
        degradation: Vec::new(),
    }
}

fn cognition_kind(value: &str) -> Result<CognitionKind> {
    match value {
        "memory" => Ok(CognitionKind::Memory),
        "cognitive_schema" => Ok(CognitionKind::CognitiveSchema),
        "episode" => Ok(CognitionKind::Episode),
        "journal" => Ok(CognitionKind::Journal),
        _ => Err(Error::new(
            nous_core::ErrorOwner::Memory,
            nous_core::ErrorCode::InvalidRequest,
            "kind must be memory, cognitive_schema, episode, or journal",
        )),
    }
}

fn ref_id(reference: &CognitiveRef) -> Uuid {
    CognitionKind::from_reference(reference)
        .expect("validated Cognition ref")
        .1
}

impl KernelService {
    pub(super) async fn read_cognition(
        &self,
        input: p::ReadCognitionRequest,
    ) -> Result<p::CognitionResult> {
        let subject = SubjectId(id(&input.subject_id)?);
        let reference = from_ref(required(input.reference, "reference")?)?;
        let view = self
            .require_memory()?
            .read_cognition(subject, reference)
            .await?;
        Ok(cognition_result(view))
    }

    pub(super) async fn list_cognitions(
        &self,
        input: p::CognitionListRequest,
    ) -> Result<p::CognitionPage> {
        let subject = SubjectId(id(&input.subject_id)?);
        let kind = cognition_kind(&input.kind)?;
        let scope = format!("cognition:{}:{}:{}", subject.0, input.kind, input.status);
        let (limit, after) = page(input.page, &scope)?;
        let mut references = self
            .require_memory()?
            .list_cognition_refs(subject, kind, &input.status, after, limit + 1)
            .await?;
        let more = references.len() > limit as usize;
        if more {
            references.pop();
        }
        let next_page_token = if more {
            references
                .last()
                .map(|r| next_token(&scope, ref_id(r)))
                .unwrap_or_default()
        } else {
            String::new()
        };
        let memory = self.require_memory()?;
        let context = memory.current_read_context(subject).await?;
        let views = memory.read_cognitions(&context, &references).await?;
        let items = references
            .iter()
            .filter_map(|reference| views.get(reference).cloned())
            .map(cognition_result)
            .collect();
        Ok(p::CognitionPage {
            items,
            next_page_token,
        })
    }

    pub(super) async fn list_cognition_history(
        &self,
        input: p::CognitionHistoryRequest,
    ) -> Result<p::CognitionPage> {
        let subject = SubjectId(id(&input.subject_id)?);
        let object = from_ref(required(input.object_ref, "object_ref")?)?;
        let (kind, id, revision) = CognitionKind::from_reference(&object)?;
        if revision {
            return Err(Error::new(
                nous_core::ErrorOwner::Memory,
                nous_core::ErrorCode::InvalidRequest,
                "history target must be an object reference",
            ));
        }
        let scope = format!("cognition-history:{}:{}:{}", subject.0, kind.as_str(), id);
        let (limit, after_id) = page(input.page, &scope)?;
        let after = after_id.map(|id| kind.revision_ref(id));
        let mut references = self
            .require_memory()?
            .cognition_history_refs(subject, &object, after.as_ref(), limit + 1)
            .await?;
        let more = references.len() > limit as usize;
        if more {
            references.pop();
        }
        let next_page_token = if more {
            references
                .last()
                .map(|r| next_token(&scope, ref_id(r)))
                .unwrap_or_default()
        } else {
            String::new()
        };
        let memory = self.require_memory()?;
        let context = memory.current_read_context(subject).await?;
        let views = memory.read_cognitions(&context, &references).await?;
        let items = references
            .iter()
            .filter_map(|reference| views.get(reference).cloned())
            .map(cognition_result)
            .collect();
        Ok(p::CognitionPage {
            items,
            next_page_token,
        })
    }

    pub(super) async fn mutate_cognition(
        &self,
        input: p::CognitionMutationRequest,
    ) -> Result<p::CognitionMutationResponse> {
        use p::cognition_mutation_request::Action;
        use p::cognition_mutation_response::Outcome;
        let subject = SubjectId(id(&input.subject_id)?);
        let operation = OperationId(id(&input.operation_id)?);
        let reference = from_ref(required(input.object_ref, "object_ref")?)?;
        let action = match input.action.ok_or_else(|| {
            Error::new(
                nous_core::ErrorOwner::Memory,
                nous_core::ErrorCode::InvalidRequest,
                "lifecycle action is required",
            )
        })? {
            Action::SetAccessibility(value) => {
                CognitionAction::SetAccessibility(enum_value(&value.mode)?)
            }
            Action::Suppress(()) => CognitionAction::Suppress,
            Action::Restore(()) => CognitionAction::Restore,
            Action::Withdraw(()) => CognitionAction::Withdraw,
            Action::Reaccept(()) => CognitionAction::Reaccept,
            Action::Purge(()) => CognitionAction::Purge,
        };
        let memory = self.require_memory()?;
        let result = if action == CognitionAction::Purge {
            self.0
                .purge_cognition(subject, reference, operation, input.expected_object_epoch)
                .await?;
            None
        } else {
            Some(
                memory
                    .mutate_cognition(
                        subject,
                        reference,
                        operation,
                        input.expected_object_epoch,
                        action,
                    )
                    .await?,
            )
        };
        let outcome = match result {
            Some(view) => Outcome::Cognition(cognition_result(view)),
            None => Outcome::Purged(()),
        };
        Ok(p::CognitionMutationResponse {
            outcome: Some(outcome),
        })
    }
}
