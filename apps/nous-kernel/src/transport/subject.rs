// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;
use nous_configuration::SubjectCapabilities;
use nous_core::{OperationId, Result, SessionId, SubjectId};
use nous_persistence::database_error as db;
use nous_subject::{
    CognitiveSeedInput, CognitiveSeedView, CreateSubject, SeedAdoptionKind, SubjectView,
};
use uuid::Uuid;

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
        }),
    }
}
impl KernelService {
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
                cognitive_seed: seed(required(input.cognitive_seed, "cognitive_seed")?),
                metadata: object(input.metadata),
                capabilities: input.capabilities.map(|value| SubjectCapabilities {
                    memory: value.memory,
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
        let mut ids = sqlx::query_scalar::<_,Uuid>("SELECT subject_id FROM subjects WHERE ($1::uuid IS NULL OR subject_id>$1) AND ($2='' OR status=$2) ORDER BY subject_id LIMIT $3")
            .bind(last).bind(&input.status).bind(limit+1).fetch_all(self.0.store.pool()).await.map_err(db)?;
        let more = ids.len() > limit as usize;
        ids.truncate(limit as usize);
        let next_page_token = if more {
            next_token(
                &scope,
                *ids.last()
                    .ok_or_else(|| Error::Internal("page continuation has no last item".into()))?,
            )
        } else {
            String::new()
        };
        let mut items = Vec::new();
        for id in ids {
            items.push(subject(self.0.subjects.subject(SubjectId(id)).await?));
        }
        Ok(p::ListSubjectsResponse {
            items,
            next_page_token,
        })
    }
    pub(super) async fn get_cognitive_seed(
        &self,
        input: p::SubjectRequest,
    ) -> Result<p::CognitiveSeedVersion> {
        Ok(seed_view(
            self.0
                .subjects
                .latest_cognitive_seed(SubjectId(id(&input.subject_id)?))
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
        Ok(p::Session {
            session_id: session.0.to_string(),
            subject_id: subject.0.to_string(),
            runtime_revision: view.runtime_revision,
            closed: view.closed_at.is_some(),
            resident_refs: view
                .resident
                .into_iter()
                .map(|r| to_ref(r.reference))
                .collect(),
            active_work_context_id: view.active_work_context_id.map(|value| value.to_string()),
        })
    }
    pub(super) async fn open_session(&self, input: p::SubjectRequest) -> Result<p::Session> {
        let subject = SubjectId(id(&input.subject_id)?);
        let view = self
            .0
            .cognition
            .open_session(subject, serde_json::json!({}))
            .await?;
        self.session_view(subject, view.session_id).await
    }
    pub(super) async fn get_session(&self, input: p::ObjectRequest) -> Result<p::Session> {
        self.session_view(SubjectId(id(&input.subject_id)?), SessionId(id(&input.id)?))
            .await
    }
    pub(super) async fn close_session(&self, input: p::ObjectRequest) -> Result<p::Session> {
        let subject = SubjectId(id(&input.subject_id)?);
        let session = SessionId(id(&input.id)?);
        self.0.cognition.close_session(subject, session).await?;
        self.session_view(subject, session).await
    }
    pub(super) async fn list_sessions(
        &self,
        input: p::ListRequest,
    ) -> Result<p::ListSessionsResponse> {
        let subject = SubjectId(id(&input.subject_id)?);
        self.0.store.require_subject(subject).await?;
        if !matches!(input.status.as_str(), "" | "open" | "closed") {
            return Err(Error::Invalid("invalid session status".into()));
        }
        let scope = format!("sessions:{}:{}", input.subject_id, input.status);
        let (limit, last) = page(input.page, &scope)?;
        let mut ids=sqlx::query_scalar::<_,Uuid>("SELECT session_id FROM cognitive_sessions WHERE subject_id=$1 AND ($2::uuid IS NULL OR session_id>$2) AND ($3='' OR ($3='open' AND closed_at IS NULL) OR ($3='closed' AND closed_at IS NOT NULL)) ORDER BY session_id LIMIT $4")
            .bind(subject.0).bind(last).bind(input.status).bind(limit+1).fetch_all(self.0.store.pool()).await.map_err(db)?;
        let more = ids.len() > limit as usize;
        ids.truncate(limit as usize);
        let next_page_token = if more {
            next_token(
                &scope,
                *ids.last()
                    .ok_or_else(|| Error::Internal("page continuation has no last item".into()))?,
            )
        } else {
            String::new()
        };
        let mut items = Vec::new();
        for id in ids {
            items.push(self.session_view(subject, SessionId(id)).await?);
        }
        Ok(p::ListSessionsResponse {
            items,
            next_page_token,
        })
    }
}
