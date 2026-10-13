// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use crate::*;

pub struct ContextSource {
    pub reference: CognitiveRef,
    pub source_revision: Option<String>,
    pub text: Option<String>,
    pub authority: AuthorityClass,
    pub evidence: Vec<EvidenceHandle>,
}

#[async_trait::async_trait]
pub trait ContextResolver: Send + Sync {
    async fn query_descriptors(
        &self,
        query: &CognitiveQuery,
        sources: &[crate::QuerySource],
        configuration: &nous_configuration::ConfigSnapshot,
        historical: Option<&HistoricalAuthoritySnapshot>,
    ) -> Result<Vec<QueryDescriptor>>;

    async fn context_source(
        &self,
        subject: SubjectId,
        reference: &CognitiveRef,
        max_bytes: usize,
        explicit: bool,
        now: DateTime<Utc>,
        configuration: &nous_configuration::ConfigSnapshot,
    ) -> Result<ContextSource>;
}

pub struct ProjectionRequest {
    pub subject: SubjectId,
    pub session: SessionId,
    pub active_work_context: Option<Uuid>,
    pub consumer_id: String,
    pub situation_refs: Vec<CognitiveRef>,
    pub max_items: usize,
    pub max_text_bytes: usize,
    pub materialize: bool,
    pub memory: RequirementStrength,
    pub runtime: RequirementStrength,
    pub resource: RequirementStrength,
}

pub struct ProjectionSegment {
    pub semantic_role: &'static str,
    pub stability: &'static str,
    pub source: ContextSource,
}

pub struct ProjectionView {
    pub subject: SubjectId,
    pub session: SessionId,
    pub active_work_context: Option<Uuid>,
    pub runtime_revision: i64,
    pub consumer_id: String,
    pub segments: Vec<ProjectionSegment>,
    pub degradation: Vec<Degradation>,
}

impl CognitiveRuntimeService {
    pub async fn runtime_context_source(
        &self,
        subject: SubjectId,
        reference: &CognitiveRef,
        max_bytes: usize,
    ) -> Result<crate::ContextSource> {
        if let CognitiveRef::Resource(resource) = reference {
            let descriptor = self.resource(subject, resource).await?.descriptor;
            return Ok(crate::ContextSource {
                reference: reference.clone(),
                source_revision: None,
                text: (max_bytes > 0).then(|| {
                    crate::projection::bounded_text(
                        descriptor
                            .display_label
                            .unwrap_or_else(|| resource.as_str().to_owned()),
                        max_bytes,
                    )
                }),
                authority: AuthorityClass::ResourceDescriptor,
                evidence: Vec::new(),
            });
        }
        if let CognitiveRef::WorkContext(id) = reference {
            let context = self.work_context(subject, id.0).await?;
            if context.state != WorkContextState::Open {
                return Err(Error::Unavailable("WorkContext is no longer open".into()));
            }
            return Ok(work_context_source(&context, max_bytes));
        }
        self.store.validate_reference(subject, reference).await?;
        let text = if max_bytes > 0 && matches!(reference, CognitiveRef::Entity(_)) {
            self.store
                .identity_descriptors(subject, std::slice::from_ref(reference), None)
                .await?
                .into_iter()
                .find(|descriptor| descriptor.reference == *reference)
                .map(|descriptor| bounded_text(descriptor.text, max_bytes))
        } else {
            None
        };
        Ok(crate::ContextSource {
            reference: reference.clone(),
            source_revision: None,
            text,
            authority: AuthorityClass::ExternalCurrentAuthority,
            evidence: Vec::new(),
        })
    }

    #[expect(
        clippy::too_many_lines,
        reason = "one owner operation captures Session context and applies consumer budgets before source reads"
    )]
    pub async fn projection(
        &self,
        input: ProjectionRequest,
        resolver: &dyn ContextResolver,
    ) -> Result<ProjectionView> {
        if input.max_items == 0 {
            return Err(Error::Invalid(
                "projection requires a positive item budget".into(),
            ));
        }
        let mut tx = self.store.read_view().await?;
        let session = self
            .session_in(input.subject, input.session, &mut tx)
            .await?;
        if session.closed_at.is_some() {
            return Err(crate::sessions::stale_session(input.session));
        }
        if input.active_work_context.is_some()
            && input.active_work_context != session.active_work_context_id
        {
            return Err(DomainError::new(
                DomainErrorCode::StaleContext,
                "projection WorkContext is not active",
            )
            .into());
        }
        let now = self.now(input.subject);
        let configuration = self.configuration.snapshot_for_subject(input.subject)?;
        let mut foreground = None;
        let mut references = session
            .resident
            .into_iter()
            .map(|resident| (resident.reference, "runtime_resident", "checkpoint", false))
            .collect::<Vec<_>>();
        if let Some(id) = session.active_work_context_id {
            let context = self.work_context_in(input.subject, id, &mut tx).await?;
            if context.state != WorkContextState::Open {
                return Err(DomainError::new(
                    DomainErrorCode::StaleContext,
                    "projection WorkContext is no longer open",
                )
                .into());
            }
            references.extend(
                context
                    .cognition_anchors
                    .iter()
                    .cloned()
                    .chain(
                        context
                            .entity_anchors
                            .iter()
                            .cloned()
                            .map(CognitiveRef::Entity),
                    )
                    .chain(context.tag_anchors.iter().copied().map(CognitiveRef::Tag))
                    .map(|reference| (reference, "runtime_work_context", "checkpoint", true)),
            );
            references.insert(
                0,
                (
                    CognitiveRef::WorkContext(WorkContextId(id)),
                    "runtime_work_context",
                    "checkpoint",
                    true,
                ),
            );
            foreground = Some(context);
        }
        tx.commit()
            .await
            .map_err(nous_persistence::database_error)?;
        references.extend(
            input
                .situation_refs
                .into_iter()
                .map(|reference| (reference, "request_situation", "request", true)),
        );
        let requirement = |reference: &CognitiveRef| match reference {
            CognitiveRef::Memory(_)
            | CognitiveRef::MemoryRevision(_)
            | CognitiveRef::CognitiveSchema(_)
            | CognitiveRef::CognitiveSchemaRevision(_)
            | CognitiveRef::Episode(_)
            | CognitiveRef::EpisodeRevision(_)
            | CognitiveRef::Journal(_)
            | CognitiveRef::JournalRevision(_)
            | CognitiveRef::Tag(_) => input.memory,
            CognitiveRef::Resource(_) => input.resource,
            _ => input.runtime,
        };
        references
            .retain(|(reference, ..)| requirement(reference) != RequirementStrength::Forbidden);
        references.sort_by_key(|(reference, ..)| requirement(reference));
        let explicit_refs = references
            .iter()
            .filter(|(_, _, _, explicit)| *explicit)
            .map(|(reference, ..)| reference.clone())
            .collect::<HashSet<_>>();
        let mut seen = HashSet::new();
        references.retain(|(reference, ..)| seen.insert(reference.clone()));
        let mut remaining = if input.materialize {
            input.max_text_bytes
        } else {
            0
        };
        let mut segments = Vec::new();
        let mut emitted = HashSet::new();
        let mut degradation = Vec::new();
        for (reference, role, stability, _) in references {
            if segments.len() >= input.max_items {
                break;
            }
            if emitted.contains(&reference) {
                continue;
            }
            let explicit = explicit_refs.contains(&reference);
            let source = if let Some(context) = foreground.as_ref().filter(|context| {
                reference == CognitiveRef::WorkContext(WorkContextId(context.work_context_id))
            }) {
                Ok(work_context_source(context, remaining))
            } else {
                resolver
                    .context_source(
                        input.subject,
                        &reference,
                        remaining,
                        explicit,
                        now,
                        &configuration,
                    )
                    .await
            };
            match source {
                Ok(source) => {
                    let reference = source.reference.clone();
                    if !emitted.insert(reference.clone()) {
                        continue;
                    }
                    remaining =
                        remaining.saturating_sub(source.text.as_ref().map_or(0, String::len));
                    segments.push(ProjectionSegment {
                        semantic_role: role,
                        stability,
                        source,
                    });
                }
                Err(
                    Error::Invalid(message)
                    | Error::NotFound(message)
                    | Error::Unavailable(message),
                ) => {
                    degradation.push(Degradation {
                        code: "projection_source_unavailable".into(),
                        detail: Some(format!("{reference}: {message}")),
                    });
                }
                Err(error) => return Err(error),
            }
        }
        let current: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM cognitive_sessions WHERE subject_id=$1 AND session_id=$2 AND runtime_revision=$3 AND closed_at IS NULL AND active_work_context_id IS NOT DISTINCT FROM $4 AND ($4::uuid IS NULL OR EXISTS(SELECT 1 FROM work_contexts WHERE subject_id=$1 AND work_context_id=$4 AND revision=$5 AND state='open')))",
        )
        .bind(input.subject.0)
        .bind(input.session.0)
        .bind(session.runtime_revision)
        .bind(session.active_work_context_id)
        .bind(foreground.as_ref().map(|context| context.revision))
        .fetch_one(self.store.pool())
        .await
        .map_err(nous_persistence::database_error)?;
        if !current {
            return Err(DomainError::new(
                DomainErrorCode::StaleContext,
                "Session or foreground WorkContext changed during projection",
            )
            .with_context("session_id", input.session.0)
            .with_context("expected_revision", session.runtime_revision)
            .into());
        }
        Ok(ProjectionView {
            subject: input.subject,
            session: input.session,
            active_work_context: session.active_work_context_id,
            runtime_revision: session.runtime_revision,
            consumer_id: input.consumer_id,
            segments,
            degradation,
        })
    }
}

pub(crate) fn bounded_text(mut text: String, max_bytes: usize) -> String {
    text.truncate(text.floor_char_boundary(max_bytes));
    text
}

fn work_context_source(context: &WorkContextView, max_bytes: usize) -> ContextSource {
    ContextSource {
        reference: CognitiveRef::WorkContext(WorkContextId(context.work_context_id)),
        source_revision: Some(context.revision.to_string()),
        text: (max_bytes > 0).then(|| bounded_text(context.task_text(), max_bytes)),
        authority: AuthorityClass::ExternalCurrentAuthority,
        evidence: Vec::new(),
    }
}
