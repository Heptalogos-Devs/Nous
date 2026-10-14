// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;
use nous_core::{EntityRef, OperationId, Result, SessionId, SubjectId, TagId};
use nous_runtime::{
    CreateWorkContextInput, UpdateWorkContextInput, WorkContextAnchors, WorkContextClearScope,
    WorkContextFields, WorkContextIntent,
};

fn view(value: nous_runtime::WorkContextView) -> p::WorkContext {
    p::WorkContext {
        work_context_id: value.work_context_id.to_string(),
        subject_id: value.subject_id.0.to_string(),
        state: serde_json::to_value(value.state)
            .ok()
            .and_then(|value| value.as_str().map(str::to_owned))
            .unwrap_or_default(),
        purpose: value.purpose,
        unresolved_questions: value.unresolved_questions,
        constraints: to_object(value.constraints),
        resume_conditions: value.resume_conditions,
        budget_summary: to_object(value.budget_summary),
        revision: value.revision,
        created_at: Some(timestamp(value.created_at)),
        updated_at: Some(timestamp(value.updated_at)),
        ended_at: value.ended_at.map(timestamp),
        cognition_anchors: value.cognition_anchors.into_iter().map(to_ref).collect(),
        context_text: value.context_text,
        entity_anchors: value
            .entity_anchors
            .into_iter()
            .map(|v| v.as_str().to_owned())
            .collect(),
        tag_anchors: value
            .tag_anchors
            .into_iter()
            .map(|v| v.0.to_string())
            .collect(),
    }
}

impl KernelService {
    pub(super) async fn create_work_context(
        &self,
        input: p::CreateWorkContextRequest,
    ) -> Result<p::WorkContextResponse> {
        let subject = SubjectId(id(&input.subject_id)?);
        let value = self
            .0
            .cognition
            .create_work_context(CreateWorkContextInput {
                operation_id: OperationId(id(&input.operation_id)?),
                subject,
                purpose: input.purpose,
                unresolved_questions: input.unresolved_questions,
                constraints: object(input.constraints),
                resume_conditions: input.resume_conditions,
                budget_summary: object(input.budget_summary),
                context_text: input.context_text,
                entity_anchors: input
                    .entity_anchors
                    .into_iter()
                    .map(EntityRef::new)
                    .collect::<Result<_>>()?,
                tag_anchors: input
                    .tag_anchors
                    .into_iter()
                    .map(|v| Ok(TagId(id(&v)?)))
                    .collect::<Result<_>>()?,
                cognition_anchors: input
                    .cognition_anchors
                    .into_iter()
                    .map(from_ref)
                    .collect::<Result<_>>()?,
            })
            .await?;
        Ok(p::WorkContextResponse {
            work_context: Some(view(value)),
            degradation: Vec::new(),
        })
    }

    pub(super) async fn get_work_context(
        &self,
        input: p::GetWorkContextRequest,
    ) -> Result<p::WorkContextResponse> {
        let value = self
            .0
            .cognition
            .work_context(
                SubjectId(id(&input.subject_id)?),
                id(&input.work_context_id)?,
            )
            .await?;
        Ok(p::WorkContextResponse {
            work_context: Some(view(value)),
            degradation: Vec::new(),
        })
    }

    pub(super) async fn list_work_contexts(
        &self,
        input: p::ListWorkContextsRequest,
    ) -> Result<p::ListWorkContextsResponse> {
        let subject = SubjectId(id(&input.subject_id)?);
        let scope = format!("work_contexts:{}", subject.0);
        let (limit, after) = page(input.page, &scope)?;
        let (values, more) = self
            .0
            .cognition
            .list_work_contexts(subject, limit, after)
            .await?;
        let next_page_token = if more {
            next_token(
                &scope,
                values
                    .last()
                    .ok_or_else(|| Error::Internal("empty page".into()))?
                    .work_context_id,
            )
        } else {
            String::new()
        };
        Ok(p::ListWorkContextsResponse {
            items: values.into_iter().map(view).collect(),
            next_page_token,
        })
    }

    pub(super) async fn update_work_context(
        &self,
        input: p::UpdateWorkContextRequest,
    ) -> Result<p::WorkContextResponse> {
        let subject = SubjectId(id(&input.subject_id)?);
        let value = self
            .0
            .cognition
            .update_work_context(UpdateWorkContextInput {
                operation_id: OperationId(id(&input.operation_id)?),
                subject,
                work_context_id: id(&input.work_context_id)?,
                expected_revision: input.expected_revision,
                intent: match input
                    .intent
                    .ok_or_else(|| Error::Invalid("WorkContext intent is required".into()))?
                {
                    p::update_work_context_request::Intent::Fields(fields) => {
                        WorkContextIntent::Fields(WorkContextFields {
                            purpose: fields.purpose,
                            unresolved_questions: fields.unresolved_questions.map(|v| v.values),
                            constraints: fields.constraints.map(|v| object(Some(v))),
                            resume_conditions: fields.resume_conditions.map(|v| v.values),
                            budget_summary: fields.budget_summary.map(|v| object(Some(v))),
                            context_text: fields.context_text,
                        })
                    }
                    p::update_work_context_request::Intent::Pin(anchors) => {
                        WorkContextIntent::Pin(anchor_input(anchors)?)
                    }
                    p::update_work_context_request::Intent::Unpin(anchors) => {
                        WorkContextIntent::Unpin(anchor_input(anchors)?)
                    }
                    p::update_work_context_request::Intent::Clear(scope) => {
                        WorkContextIntent::Clear(match scope {
                            1 => WorkContextClearScope::Text,
                            2 => WorkContextClearScope::Anchors,
                            3 => WorkContextClearScope::All,
                            _ => {
                                return Err(Error::Invalid(
                                    "WorkContext clear scope is required".into(),
                                ));
                            }
                        })
                    }
                },
            })
            .await?;
        Ok(p::WorkContextResponse {
            work_context: Some(view(value)),
            degradation: Vec::new(),
        })
    }

    async fn transition_work_context(
        &self,
        input: p::WorkContextMutationRequest,
        transition: &str,
    ) -> Result<p::WorkContextResponse> {
        let subject = SubjectId(id(&input.subject_id)?);
        let context_id = id(&input.work_context_id)?;
        let operation_id = OperationId(id(&input.operation_id)?);
        let value = match transition {
            "pause" => {
                self.0
                    .cognition
                    .pause_work_context(subject, context_id, input.expected_revision, operation_id)
                    .await?
            }
            "resume" => {
                self.0
                    .cognition
                    .resume_work_context(subject, context_id, input.expected_revision, operation_id)
                    .await?
            }
            "end" => {
                self.0
                    .cognition
                    .end_work_context(subject, context_id, input.expected_revision, operation_id)
                    .await?
            }
            _ => return Err(Error::Invalid("unknown WorkContext transition".into())),
        };
        Ok(p::WorkContextResponse {
            work_context: Some(view(value)),
            degradation: Vec::new(),
        })
    }

    pub(super) async fn pause_work_context(
        &self,
        input: p::WorkContextMutationRequest,
    ) -> Result<p::WorkContextResponse> {
        self.transition_work_context(input, "pause").await
    }

    pub(super) async fn resume_work_context(
        &self,
        input: p::WorkContextMutationRequest,
    ) -> Result<p::WorkContextResponse> {
        self.transition_work_context(input, "resume").await
    }

    pub(super) async fn end_work_context(
        &self,
        input: p::WorkContextMutationRequest,
    ) -> Result<p::WorkContextResponse> {
        self.transition_work_context(input, "end").await
    }

    pub(super) async fn set_active_work_context(
        &self,
        input: p::SetActiveWorkContextRequest,
    ) -> Result<p::Session> {
        let subject = SubjectId(id(&input.subject_id)?);
        let session = SessionId(id(&input.session_id)?);
        self.0
            .cognition
            .set_active_work_context(
                subject,
                session,
                input.work_context_id.as_deref().map(id).transpose()?,
                input.expected_runtime_revision,
                OperationId(id(&input.operation_id)?),
            )
            .await?;
        self.session_view(subject, session).await
    }

    pub(super) async fn report_use(
        &self,
        input: p::ReportUseRequest,
    ) -> Result<p::ReportUseResponse> {
        if input.events.len() > 256 || input.consumer_ref.is_empty() {
            return Err(Error::Invalid("invalid use feedback bounds".into()));
        }
        self.0
            .cognition
            .use_feedback(nous_runtime::UseFeedback {
                subject: SubjectId(id(&input.subject_id)?),
                session_id: input
                    .session_id
                    .as_deref()
                    .map(id)
                    .transpose()?
                    .map(SessionId),
                consumer_ref: input.consumer_ref,
                events: input
                    .events
                    .into_iter()
                    .map(|event| {
                        Ok(nous_runtime::UseFeedbackEvent {
                            query_id: event.query_id.as_deref().map(id).transpose()?,
                            event_id: nous_core::UseEventId(id(&event.event_id)?),
                            reference: from_ref(required(event.reference, "reference")?)?,
                            use_kind: enum_value(&event.kind)?,
                            occurred_at: time(Some(event.occurred_at.ok_or_else(|| {
                                Error::Invalid("occurred_at is required".into())
                            })?))?
                            .ok_or_else(|| Error::Invalid("occurred_at is invalid".into()))?,
                            context: object(event.context),
                        })
                    })
                    .collect::<Result<_>>()?,
            })
            .await
            .map(
                |(accepted_count, duplicate_count, session_runtime_revision)| {
                    p::ReportUseResponse {
                        accepted_count,
                        duplicate_count,
                        session_runtime_revision,
                    }
                },
            )
    }
}

fn anchor_input(input: p::WorkContextAnchors) -> Result<WorkContextAnchors> {
    Ok(WorkContextAnchors {
        cognition_anchors: input
            .cognition_anchors
            .into_iter()
            .map(from_ref)
            .collect::<Result<_>>()?,
        entity_anchors: input
            .entity_anchors
            .into_iter()
            .map(EntityRef::new)
            .collect::<Result<_>>()?,
        tag_anchors: input
            .tag_anchors
            .into_iter()
            .map(|v| Ok(TagId(id(&v)?)))
            .collect::<Result<_>>()?,
    })
}
