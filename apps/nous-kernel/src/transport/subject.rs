// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;
use nous_configuration::SubjectCapabilities;
use nous_core::{OperationId, Result, SessionId, SubjectId};
use nous_subject::{
    CognitiveSeedInput, CognitiveSeedView, CreateSubject, SeedAdoptionKind, SubjectView,
};

fn seed(input: p::CognitiveSeed) -> CognitiveSeedInput {
    CognitiveSeedInput {
        text: input.text,
        format: if input.format.is_empty() {
            nous_subject::COGNITIVE_SEED_FORMAT.into()
        } else {
            input.format
        },
        provenance: object(input.provenance),
    }
}
fn seed_view(input: CognitiveSeedView) -> p::CognitiveSeedVersion {
    p::CognitiveSeedVersion {
        seed: Some(p::CognitiveSeed {
            text: input.text,
            format: input.version.format.clone(),
            provenance: to_object(input.version.provenance.clone()),
        }),
        seed_version_id: input.version.seed_version_id.0.to_string(),
        subject_id: input.version.subject_id.0.to_string(),
        artifact_id: input.version.artifact_id.0.to_string(),
        format: input.version.format,
        content_hash: input.version.content_hash,
        created_at: Some(timestamp(input.version.created_at)),
        adoption_id: input.adoption.adoption_id.to_string(),
        adoption_kind: enum_name(input.adoption.kind),
        operation_id: input.adoption.operation_id.0.to_string(),
    }
}
pub(super) fn seed_source(input: nous_subject::CognitiveSeedSource) -> p::CognitiveSeedVersion {
    p::CognitiveSeedVersion {
        seed: Some(p::CognitiveSeed {
            text: input.text,
            format: input.version.format.clone(),
            provenance: to_object(input.version.provenance),
        }),
        seed_version_id: input.version.seed_version_id.0.to_string(),
        subject_id: input.version.subject_id.0.to_string(),
        artifact_id: input.version.artifact_id.0.to_string(),
        format: input.version.format,
        content_hash: input.version.content_hash,
        created_at: Some(timestamp(input.version.created_at)),
        ..Default::default()
    }
}
fn subject(input: SubjectView) -> p::Subject {
    let capabilities = input.capabilities;
    p::Subject {
        subject_id: input.subject_id.0.to_string(),
        created_at: Some(timestamp(input.created_at)),
        authority_seq: input.authority_seq,
        status: input.status,
        metadata: to_object(input.metadata),
        capabilities: Some(p::SubjectCapabilities {
            memory: capabilities.memory,
            self_cognition: capabilities.self_cognition,
            social_cognition: capabilities.social_cognition,
        }),
    }
}
impl KernelService {
    pub(super) async fn provision_capabilities(
        &self,
        input: p::ProvisionCapabilitiesRequest,
    ) -> Result<p::Subject> {
        let c = required(input.capabilities, "capabilities")?;
        Ok(subject(
            self.0
                .subjects
                .provision_capabilities(
                    SubjectId(id(&input.subject_id)?),
                    OperationId(id(&input.operation_id)?),
                    SubjectCapabilities {
                        memory: c.memory,
                        self_cognition: c.self_cognition,
                        social_cognition: c.social_cognition,
                    },
                )
                .await?,
        ))
    }

    pub(super) async fn create_subject(
        &self,
        input: p::CreateSubjectRequest,
    ) -> Result<p::Subject> {
        let view = self
            .0
            .subjects
            .create_subject(CreateSubject {
                subject_id: input
                    .subject_id
                    .as_deref()
                    .map(id)
                    .transpose()?
                    .map(SubjectId),
                operation_id: OperationId(id(&input.operation_id)?),
                cognitive_seed: input.cognitive_seed.map(seed),
                metadata: object(input.metadata),
                capabilities: input.capabilities.map(|value| SubjectCapabilities {
                    memory: value.memory,
                    self_cognition: value.self_cognition,
                    social_cognition: value.social_cognition,
                }),
            })
            .await?;
        Ok(subject(view))
    }
    pub(super) async fn get_subject(&self, input: p::SubjectRequest) -> Result<p::Subject> {
        Ok(subject(
            self.0
                .subjects
                .subject(SubjectId(id(&input.subject_id)?))
                .await?,
        ))
    }
    pub(super) async fn list_subjects(
        &self,
        input: p::ListRequest,
    ) -> Result<p::ListSubjectsResponse> {
        let scope = format!("subjects:{}", input.status);
        let (limit, last) = page(input.page, &scope)?;
        let (items, more) = self.0.subjects.subjects(&input.status, limit, last).await?;
        let next_page_token = if more {
            next_token(
                &scope,
                items
                    .last()
                    .ok_or_else(|| Error::Internal("empty page".into()))?
                    .subject_id
                    .0,
            )
        } else {
            String::new()
        };
        Ok(p::ListSubjectsResponse {
            items: items.into_iter().map(subject).collect(),
            next_page_token,
        })
    }
    pub(super) async fn get_cognitive_seed_version(
        &self,
        input: p::ObjectRequest,
    ) -> Result<p::CognitiveSeedVersion> {
        Ok(seed_source(
            self.0
                .subjects
                .cognitive_seed_version(
                    SubjectId(id(&input.subject_id)?),
                    nous_core::CognitiveSeedVersionId(id(&input.id)?),
                )
                .await?,
        ))
    }
    pub(super) async fn get_cognitive_seed(
        &self,
        input: p::SubjectRequest,
    ) -> Result<p::CognitiveSeedVersion> {
        Ok(seed_view(
            self.0
                .subjects
                .cognitive_seed(SubjectId(id(&input.subject_id)?), None)
                .await?,
        ))
    }
    pub(super) async fn adopt_cognitive_seed(
        &self,
        input: p::AdoptCognitiveSeedRequest,
    ) -> Result<p::CognitiveSeedVersion> {
        let subject = SubjectId(id(&input.subject_id)?);
        let kind = match input.kind.as_str() {
            "initial" => SeedAdoptionKind::Initial,
            "import" | "" => SeedAdoptionKind::Import,
            _ => {
                return Err(Error::Invalid(
                    "invalid Cognitive Seed adoption kind".into(),
                ));
            }
        };
        Ok(seed_view(
            self.0
                .subjects
                .adopt_cognitive_seed(
                    subject,
                    OperationId(id(&input.operation_id)?),
                    seed(required(input.seed, "seed")?),
                    kind,
                )
                .await?,
        ))
    }
    pub(super) async fn session_view(
        &self,
        subject: SubjectId,
        session: SessionId,
    ) -> Result<p::Session> {
        let view = self.0.cognition.session(subject, session).await?;
        Ok(session_proto(view))
    }
    pub(super) async fn open_session(&self, input: p::OpenSessionRequest) -> Result<p::Session> {
        let subject = SubjectId(id(&input.subject_id)?);
        let view = self
            .0
            .cognition
            .open_session(
                subject,
                input
                    .parent_session_id
                    .as_deref()
                    .map(id)
                    .transpose()?
                    .map(SessionId),
                serde_json::json!({}),
            )
            .await?;
        Ok(session_proto(view))
    }
    pub(super) async fn get_session(&self, input: p::ObjectRequest) -> Result<p::Session> {
        self.session_view(SubjectId(id(&input.subject_id)?), SessionId(id(&input.id)?))
            .await
    }
    pub(super) async fn close_session(&self, input: p::ObjectRequest) -> Result<p::Session> {
        let subject = SubjectId(id(&input.subject_id)?);
        let session = SessionId(id(&input.id)?);
        self.0
            .cognition
            .close_session(subject, session)
            .await
            .map(session_proto)
    }
    pub(super) async fn list_sessions(
        &self,
        input: p::ListRequest,
    ) -> Result<p::ListSessionsResponse> {
        let subject = SubjectId(id(&input.subject_id)?);
        let scope = format!("sessions:{}:{}", input.subject_id, input.status);
        let (limit, last) = page(input.page, &scope)?;
        let (items, more) = self
            .0
            .cognition
            .sessions(subject, &input.status, limit, last)
            .await?;
        let next_page_token = if more {
            next_token(
                &scope,
                items
                    .last()
                    .ok_or_else(|| Error::Internal("empty page".into()))?
                    .session_id
                    .0,
            )
        } else {
            String::new()
        };
        Ok(p::ListSessionsResponse {
            items: items.into_iter().map(session_proto).collect(),
            next_page_token,
        })
    }
}

fn session_proto(view: nous_runtime::SessionView) -> p::Session {
    p::Session {
        session_id: view.session_id.0.to_string(),
        subject_id: view.subject_id.0.to_string(),
        runtime_revision: view.runtime_revision,
        parent_session_id: view.parent_session_id.map(|id| id.0.to_string()),
        history_invalidated: view.history_invalidated,
        history_domains: view.history_domains,
        closed_at: view.closed_at.map(timestamp),
        opened_at: Some(timestamp(view.opened_at)),
        last_activity_at: Some(timestamp(view.last_activity_at)),
        last_meaningful_use_at: view.last_meaningful_use_at.map(timestamp),
        resident: view
            .resident
            .into_iter()
            .map(|r| p::Resident {
                reference: Some(to_ref(r.reference)),
                entry_reason: r.entry_reason,
                state: r.state,
                entered_at: Some(timestamp(r.entered_at)),
                last_meaningful_use_at: r.last_meaningful_use_at.map(timestamp),
                hold_until: r.hold_until.map(timestamp),
            })
            .collect(),
        active_work_context_id: view.active_work_context_id.map(|value| value.to_string()),
    }
}
