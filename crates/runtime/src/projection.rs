// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use crate::*;
use std::collections::HashMap;

#[derive(Clone)]
pub struct ContextSource {
    pub usage: &'static str,
    pub required: bool,
    pub reference: CognitiveRef,
    pub source_revision: Option<String>,
    pub text: Option<String>,
    pub authority: AuthorityClass,
    pub evidence: Vec<EvidenceHandle>,
}

#[derive(Default)]
pub struct InvocationScope {
    pub work_context: Option<WorkContextId>,
    pub source_read: SourceReadScope,
    pub group: Option<EntityRef>,
    pub channel: Option<String>,
    pub domain: Option<String>,
    pub intent: String,
    pub purpose: String,
    pub conversation: String,
    pub entities: Vec<EntityRef>,
}
pub struct AutomaticContextRequest<'a> {
    pub scope: &'a InvocationScope,
    pub work_context: Option<Uuid>,
    pub materialize: bool,
    pub max_items: usize,
    pub max_bytes: usize,
    pub ranked: &'a [CognitiveRef],
}
#[derive(Default)]
pub struct AutomaticContext {
    pub sources: Vec<ContextSource>,
    pub degradation: Vec<Degradation>,
}
#[async_trait::async_trait]
pub trait ContextResolver: Send + Sync {
    async fn source_read_view(
        &self,
        subject: SubjectId,
        scope: &SourceReadScope,
    ) -> Result<SourceReadView> {
        let _ = scope;
        Ok(SourceReadView {
            subject,
            ..Default::default()
        })
    }

    async fn automatic_context_sources(
        &self,
        _subject: SubjectId,
        _input: AutomaticContextRequest<'_>,
    ) -> Result<AutomaticContext> {
        Ok(AutomaticContext::default())
    }

    async fn query_descriptors(
        &self,
        query: &CognitiveQuery,
        sources: &[crate::QuerySource],
        configuration: &nous_configuration::ConfigSnapshot,
        historical: Option<&HistoricalAuthoritySnapshot>,
    ) -> Result<Vec<QueryDescriptor>>;

    async fn context_sources(
        &self,
        subject: SubjectId,
        references: &[(CognitiveRef, bool)],
        max_bytes: usize,
        now: DateTime<Utc>,
        configuration: &nous_configuration::ConfigSnapshot,
        source_read: Option<&SourceReadView>,
        scope: Option<&InvocationScope>,
    ) -> Result<Vec<Result<ContextSource>>>;
}

pub struct ProjectionRequest {
    pub subject: SubjectId,
    pub scope: Option<InvocationScope>,
    pub session: SessionId,
    pub active_work_context: Option<Uuid>,
    pub consumer_id: String,
    pub situation_refs: Vec<CognitiveRef>,
    pub max_items: usize,
    pub max_text_bytes: usize,
    pub materialize: bool,
    pub memory: RequirementStrength,
    pub self_cognition: RequirementStrength,
    pub social_cognition: RequirementStrength,
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
                usage: "task_evidence",
                required: false,
                reference: reference.clone(),
                source_revision: None,
                text: (max_bytes > 0).then(|| {
                    descriptor
                        .display_label
                        .unwrap_or_else(|| resource.as_str().to_owned())
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
                .map(|descriptor| descriptor.text)
        } else {
            None
        };
        Ok(crate::ContextSource {
            usage: "task_evidence",
            required: false,
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
        let source_read = if let Some(scope) = &input.scope {
            Some(
                resolver
                    .source_read_view(input.subject, &scope.source_read)
                    .await?,
            )
        } else {
            None
        };
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
        let mut foreground_revision: Option<i64> = None;
        let mut references = session
            .resident
            .into_iter()
            .map(|resident| (resident.reference, "runtime_resident", "checkpoint", false))
            .collect::<Vec<_>>();
        if let Some(id) = session.active_work_context_id {
            foreground_revision=sqlx::query_scalar("SELECT revision FROM work_contexts WHERE subject_id=$1 AND work_context_id=$2 AND state='open'").bind(input.subject.0).bind(id).fetch_optional(&mut *tx).await.map_err(nous_persistence::database_error)?;
            if foreground_revision.is_none() {
                return Err(Error::Unavailable("active WorkContext unavailable".into()));
            }
            let context_read = if let Some(read) = &source_read {
                nous_persistence::AuthorityStore::source_read_allowed_in(
                    &mut tx,
                    read,
                    &CognitiveRef::WorkContext(WorkContextId(id)),
                )
                .await?
            } else {
                true
            };
            if !context_read {
                let anchors:Vec<(String,String)>=sqlx::query_as("SELECT r.ref_kind,r.ref_value FROM work_context_refs r JOIN work_contexts w USING(work_context_id) WHERE w.subject_id=$1 AND w.work_context_id=$2").bind(input.subject.0).bind(id).fetch_all(&mut *tx).await.map_err(nous_persistence::database_error)?;
                for (kind, value) in anchors {
                    references.push((
                        parse_reference(&kind, &value)?,
                        "runtime_work_context",
                        "checkpoint",
                        true,
                    ));
                }
            } else {
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
        }
        if let Some(read) = &source_read {
            let mut admitted = Vec::new();
            for reference in references.drain(..) {
                if nous_persistence::AuthorityStore::source_read_allowed_in(
                    &mut tx,
                    read,
                    &reference.0,
                )
                .await?
                {
                    admitted.push(reference);
                }
            }
            references = admitted;
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
        if let Some(read) = &source_read {
            let mut tx = self.store.read_view().await?;
            let mut admitted = Vec::new();
            for r in references.drain(..) {
                if nous_persistence::AuthorityStore::source_read_allowed_in(&mut tx, read, &r.0)
                    .await?
                {
                    admitted.push(r);
                }
            }
            references = admitted;
        }
        let automatic = if input.self_cognition != RequirementStrength::Forbidden
            || input.social_cognition != RequirementStrength::Forbidden
        {
            if let Some(scope) = &input.scope {
                resolver
                    .automatic_context_sources(
                        input.subject,
                        AutomaticContextRequest {
                            scope,
                            work_context: session.active_work_context_id,
                            materialize: input.materialize,
                            max_items: input.max_items,
                            max_bytes: input.max_text_bytes,
                            ranked: &references
                                .iter()
                                .map(|(r, ..)| r.clone())
                                .collect::<Vec<_>>(),
                        },
                    )
                    .await?
            } else {
                AutomaticContext::default()
            }
        } else {
            AutomaticContext::default()
        };
        let mut degradation = automatic.degradation;
        let automatic = automatic.sources;
        let required_bytes: usize = automatic
            .iter()
            .filter(|s| s.required)
            .map(|s| s.text.as_ref().map_or(0, String::len))
            .sum();
        let required_count = automatic.iter().filter(|s| s.required).count();
        let foreground_text = foreground
            .as_ref()
            .filter(|_| input.runtime != RequirementStrength::Forbidden)
            .map(|w| w.task_text());
        if required_count + usize::from(foreground_text.is_some()) > input.max_items
            || (input.materialize
                && required_bytes + foreground_text.as_ref().map_or(0, String::len)
                    > input.max_text_bytes)
        {
            return Err(Error::Unavailable(
                "required cognition exceeds projection budget".into(),
            ));
        }
        let automatic_refs = automatic
            .iter()
            .map(|s| s.reference.clone())
            .collect::<Vec<_>>();
        let cached = automatic
            .into_iter()
            .map(|s| (s.reference.clone(), s))
            .collect::<HashMap<_, _>>();
        references.splice(
            0..0,
            automatic_refs
                .into_iter()
                .map(|r| (r, "current_self", "checkpoint", true)),
        );
        let requirement = |reference: &CognitiveRef| match reference.domain() {
            CognitiveDomain::Memory => input.memory,
            CognitiveDomain::SelfCognition => input.self_cognition,
            CognitiveDomain::SocialCognition => input.social_cognition,
            CognitiveDomain::Resource => input.resource,
            _ => input.runtime,
        };
        references
            .retain(|(reference, ..)| requirement(reference) != RequirementStrength::Forbidden);
        references.sort_by_key(|(reference, ..)| {
            (
                !cached.get(reference).is_some_and(|s| s.required),
                !matches!(reference, CognitiveRef::WorkContext(_)),
                requirement(reference),
            )
        });
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
        let mut references = references.into_iter();
        while segments.len() < input.max_items {
            if input.materialize && remaining == 0 {
                if references.next().is_some() {
                    degradation.push(Degradation {
                        code: "context_budget_omission".into(),
                        detail: Some("remaining semantic units omitted whole".into()),
                    });
                }
                break;
            }
            let batch = references
                .by_ref()
                .filter(|(r, ..)| !emitted.contains(r))
                .take((input.max_items - segments.len()).min(16))
                .collect::<Vec<_>>();
            if batch.is_empty() {
                break;
            }
            let requested = batch
                .iter()
                .filter(|(r, ..)| {
                    !cached.contains_key(r)
                        && foreground.as_ref().is_none_or(|c| {
                            *r != CognitiveRef::WorkContext(WorkContextId(c.work_context_id))
                        })
                })
                .map(|(r, ..)| (r.clone(), explicit_refs.contains(r)))
                .collect::<Vec<_>>();
            let mut sources = resolver
                .context_sources(
                    input.subject,
                    &requested,
                    remaining,
                    now,
                    &configuration,
                    source_read.as_ref(),
                    input.scope.as_ref(),
                )
                .await?
                .into_iter();
            for (reference, role, stability, _) in batch {
                let source = if let Some(source) = cached.get(&reference) {
                    Ok(source.clone())
                } else if let Some(context) = foreground.as_ref().filter(|c| {
                    reference == CognitiveRef::WorkContext(WorkContextId(c.work_context_id))
                }) {
                    Ok(work_context_source(context, remaining))
                } else {
                    sources.next().expect("resolver preserves request order")
                };
                let source = match source {
                    Ok(source) => source,
                    Err(
                        Error::Invalid(message)
                        | Error::NotFound(message)
                        | Error::Unavailable(message),
                    ) => {
                        degradation.push(Degradation {
                            code: "projection_source_unavailable".into(),
                            detail: Some(format!("{reference}: {message}")),
                        });
                        continue;
                    }
                    Err(error) => return Err(error),
                };
                if !emitted.insert(source.reference.clone()) {
                    continue;
                }
                let exceeds_budget = input.materialize
                    && source
                        .text
                        .as_ref()
                        .is_some_and(|text| text.len() > remaining);
                if exceeds_budget && source.required {
                    return Err(Error::Unavailable(
                        "required WorkContext exceeds projection budget".into(),
                    ));
                }
                if exceeds_budget {
                    degradation.push(Degradation {
                        code: "context_budget_omission".into(),
                        detail: Some(format!("{reference}: complete semantic unit omitted")),
                    });
                    continue;
                }
                remaining = remaining.saturating_sub(source.text.as_ref().map_or(0, String::len));
                segments.push(ProjectionSegment {
                    semantic_role: role,
                    stability,
                    source,
                });
            }
        }
        if references.next().is_some() {
            degradation.push(Degradation {
                code: "context_budget_omission".into(),
                detail: Some("remaining semantic units omitted at projection item budget".into()),
            });
        }
        let current: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM cognitive_sessions WHERE subject_id=$1 AND session_id=$2 AND runtime_revision=$3 AND closed_at IS NULL AND active_work_context_id IS NOT DISTINCT FROM $4 AND ($4::uuid IS NULL OR EXISTS(SELECT 1 FROM work_contexts WHERE subject_id=$1 AND work_context_id=$4 AND revision=$5 AND state='open')))",
        )
        .bind(input.subject.0)
        .bind(input.session.0)
        .bind(session.runtime_revision)
        .bind(session.active_work_context_id)
        .bind(foreground_revision)
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

fn work_context_source(context: &WorkContextView, max_bytes: usize) -> ContextSource {
    ContextSource {
        usage: "situational_guidance",
        required: true,
        reference: CognitiveRef::WorkContext(WorkContextId(context.work_context_id)),
        source_revision: Some(context.revision.to_string()),
        text: (max_bytes > 0).then(|| context.task_text()),
        authority: AuthorityClass::ExternalCurrentAuthority,
        evidence: Vec::new(),
    }
}
