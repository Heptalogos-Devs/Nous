// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use crate::NousRuntime;
use nous_core::*;
use nous_runtime::{ContextResolver, ContextSource};

#[async_trait::async_trait]
impl ContextResolver for NousRuntime {
    async fn source_read_view(
        &self,
        subject: SubjectId,
        scope: &SourceReadScope,
    ) -> Result<SourceReadView> {
        crate::source_read::SourceRoots {
            store: self.store.clone(),
            material: self.material.clone(),
            memory: self.memory.clone(),
            self_cognition: self.self_cognition.clone(),
        }
        .view(subject, scope)
        .await
    }

    async fn automatic_context_sources(
        &self,
        subject: SubjectId,
        input: nous_runtime::AutomaticContextRequest<'_>,
    ) -> Result<nous_runtime::AutomaticContext> {
        let capabilities = self.subjects.subject(subject).await?.capabilities;
        let mut result = if capabilities.self_cognition {
            self.automatic_self_context(
                subject,
                nous_runtime::AutomaticContextRequest {
                    scope: input.scope,
                    work_context: input.work_context,
                    materialize: input.materialize,
                    max_items: input.max_items,
                    max_bytes: input.max_bytes,
                    ranked: input.ranked,
                },
            )
            .await?
        } else {
            nous_runtime::AutomaticContext::default()
        };
        if capabilities.social_cognition
            && input.scope.source_read.subject_id == Some(subject)
            && (!input.scope.source_read.source_refs.is_empty()
                || !input.scope.source_read.origin_conversation_refs.is_empty())
        {
            let scope = nous_social::SocialScope {
                description: "Actual Host invocation".into(),
                entities: input.scope.entities.clone(),
                group: input.scope.group.clone(),
                channel: input.scope.channel.clone(),
                conversation: Some(input.scope.conversation.clone()),
                work_context: input.work_context.map(WorkContextId),
                domain: input.scope.domain.clone(),
            };
            let selected = self
                .require_social()?
                .select(
                    subject,
                    nous_social::SocialSelectionRequest {
                        scope: &scope,
                        source_read: &input.scope.source_read,
                        max_items: 2048,
                        max_bytes: 4 * 1024 * 1024,
                        after: None,
                        expression: None,
                        endpoint: None,
                        counterpart: None,
                        at: None,
                        as_of: None,
                        history: false,
                    },
                )
                .await?;
            if selected.omitted {
                result.degradation.push(Degradation{code:"social_candidate_omission".into(),detail:Some(format!("Authorized structural candidates exceed 2048 / 4MiB read budget; continuation {:?}; selection does not assert a complete social world",selected.next))});
            }
            let allowed = selected
                .records
                .iter()
                .map(|r| r.reference())
                .collect::<Vec<_>>();
            let (ranked, degradation) = self
                .serving
                .lexical_candidates_authorized(
                    subject,
                    &input.scope.intent,
                    128,
                    &["social_relation", "language_convention"],
                    Some(&allowed),
                )
                .await?;
            result.degradation.extend(degradation);
            let mut records = selected.records;
            records.sort_by_key(|r| {
                ranked
                    .iter()
                    .position(|v| *v == r.reference())
                    .unwrap_or(usize::MAX)
            });
            for record in records {
                if result.sources.len() >= input.max_items {
                    result.degradation.push(Degradation {
                        code: "social_projection_omission".into(),
                        detail: Some(
                            "Complete social units omitted at shared projection item budget".into(),
                        ),
                    });
                    break;
                }
                let mut source =
                    nous_social::SocialService::record_context(&record, input.materialize);
                source.usage = if input.scope.purpose == "expression" {
                    "audience_language_guidance"
                } else {
                    "social_understanding"
                };
                let used = result
                    .sources
                    .iter()
                    .map(|s| s.text.as_ref().map_or(0, String::len))
                    .sum::<usize>();
                if source
                    .text
                    .as_ref()
                    .is_some_and(|t| used + t.len() > input.max_bytes)
                {
                    result.degradation.push(Degradation {
                        code: "social_projection_omission".into(),
                        detail: Some("Complete social unit omitted at shared text budget".into()),
                    });
                    continue;
                }
                result.sources.push(source);
            }
        }
        Ok(result)
    }

    async fn query_descriptors(
        &self,
        query: &CognitiveQuery,
        sources: &[nous_runtime::QuerySource],
        configuration: &nous_configuration::ConfigSnapshot,
        historical: Option<&HistoricalAuthoritySnapshot>,
    ) -> Result<Vec<QueryDescriptor>> {
        let (cognition, other): (Vec<_>, Vec<_>) = sources
            .iter()
            .cloned()
            .partition(|source| nous_memory::MemoryService::owns_content(&source.reference));
        let mut descriptors = if cognition.is_empty() {
            vec![]
        } else {
            self.require_memory()?
                .query_descriptors(
                    query.subject,
                    &cognition
                        .iter()
                        .map(|s| {
                            (
                                s.reference.clone(),
                                !s.is_runtime() || s.origin == "work_context",
                            )
                        })
                        .collect::<Vec<_>>(),
                    query.effort,
                    query.temporal_frame.clock_now,
                    configuration,
                    historical,
                    query.source_read.as_ref(),
                )
                .await?
        };
        let (material, identities): (Vec<_>, Vec<_>) = other
            .into_iter()
            .map(|s| s.reference)
            .partition(|reference| {
                nous_runtime::CognitiveContributor::owns(&self.material, reference)
            });
        descriptors.extend(
            self.material
                .query_descriptors(
                    query.subject,
                    &material,
                    historical,
                    query.source_read.as_ref(),
                )
                .await?,
        );
        let (selves, identities): (Vec<_>, Vec<_>) = identities
            .into_iter()
            .partition(|r| r.domain() == CognitiveDomain::SelfCognition);
        for reference in selves {
            match self
                .require_self()?
                .query_descriptor(query.subject, &reference, query.source_read.as_ref())
                .await
            {
                Ok(value) => descriptors.push(value),
                Err(Error::NotFound(_) | Error::Unavailable(_)) => {}
                Err(error) => return Err(error),
            }
        }
        let identities = identities
            .into_iter()
            .filter(|r| r.domain() != CognitiveDomain::SocialCognition)
            .collect::<Vec<_>>();
        descriptors.extend(
            self.store
                .identity_descriptors(query.subject, &identities, historical)
                .await?,
        );
        Ok(descriptors)
    }

    async fn context_sources(
        &self,
        subject: SubjectId,
        references: &[(CognitiveRef, bool)],
        max_bytes: usize,
        now: chrono::DateTime<chrono::Utc>,
        configuration: &nous_configuration::ConfigSnapshot,
        source_read: Option<&SourceReadView>,
        scope: Option<&nous_runtime::InvocationScope>,
    ) -> Result<Vec<Result<ContextSource>>> {
        let mut result = (0..references.len())
            .map(|_| Err(Error::NotFound("context source not found".into())))
            .collect::<Vec<_>>();
        let (cognition, other): (Vec<_>, Vec<_>) = references
            .iter()
            .enumerate()
            .partition(|(_, (reference, _))| nous_memory::MemoryService::owns_content(reference));
        if !cognition.is_empty() {
            let policy = nous_memory::resolve_accessibility_policy(configuration)?;
            let sources = self
                .require_memory()?
                .context_sources(
                    subject,
                    &cognition
                        .iter()
                        .map(|(_, r)| (*r).clone())
                        .collect::<Vec<_>>(),
                    &policy,
                    now,
                    max_bytes,
                    source_read,
                )
                .await?;
            for ((index, _), source) in cognition.into_iter().zip(sources) {
                result[index] = source;
            }
        }
        let (material, other): (Vec<_>, Vec<_>) =
            other.into_iter().partition(|(_, (reference, _))| {
                matches!(reference, CognitiveRef::ExternalObject(_))
                    || nous_runtime::CognitiveContributor::owns(&self.material, reference)
            });
        if !material.is_empty() {
            let sources = self
                .material
                .context_sources(
                    subject,
                    &material
                        .iter()
                        .map(|(_, (r, _))| r.clone())
                        .collect::<Vec<_>>(),
                    max_bytes,
                    source_read,
                )
                .await?;
            for ((index, _), source) in material.into_iter().zip(sources) {
                result[index] = source;
            }
        }
        for (index, (reference, _)) in other {
            if let Some(read) = source_read {
                let mut tx = self.store.read_view().await?;
                if !nous_persistence::AuthorityStore::source_read_allowed_in(
                    &mut tx, read, reference,
                )
                .await?
                {
                    continue;
                }
            }
            result[index] = if reference.domain() == CognitiveDomain::SelfCognition {
                self.require_self()?
                    .context_source(subject, reference, max_bytes > 0, source_read)
                    .await
            } else if reference.domain() == CognitiveDomain::SocialCognition {
                match (source_read, scope) {
                    (Some(read), Some(scope)) => {
                        self.require_social()?
                            .context_source_authorized(
                                subject,
                                reference,
                                read,
                                scope,
                                max_bytes > 0,
                            )
                            .await
                    }
                    _ => Err(Error::Unavailable(
                        "Social requires actual source and invocation scope".into(),
                    )),
                }
            } else {
                self.cognition
                    .runtime_context_source(subject, reference, max_bytes)
                    .await
            };
        }
        Ok(result)
    }
}

impl NousRuntime {
    async fn automatic_self_context(
        &self,
        subject: SubjectId,
        input: nous_runtime::AutomaticContextRequest<'_>,
    ) -> Result<nous_runtime::AutomaticContext> {
        if !self
            .subjects
            .subject(subject)
            .await?
            .capabilities
            .self_cognition
        {
            return Ok(nous_runtime::AutomaticContext::default());
        }
        let read = self
            .source_read_view(subject, &input.scope.source_read)
            .await?;
        let allowed = self
            .require_self()?
            .authorized_references(subject, &read, true)
            .await?;
        let (discovered, degradation) = self
            .serving
            .lexical_candidates_authorized(
                subject,
                &input.scope.intent.chars().take(2048).collect::<String>(),
                input.max_items.min(128),
                &["self_facet"],
                Some(&allowed),
            )
            .await?;
        let mut ranked = input.ranked.to_vec();
        ranked.extend(discovered);
        ranked.retain(|r| r.domain() == CognitiveDomain::SelfCognition && allowed.contains(r));
        ranked.truncate(128);

        let mut selected = self
            .require_self()?
            .automatic_context(
                subject,
                nous_runtime::AutomaticContextRequest {
                    ranked: &ranked,
                    ..input
                },
                &read,
            )
            .await?;
        selected.degradation.extend(degradation);
        Ok(selected)
    }
}
