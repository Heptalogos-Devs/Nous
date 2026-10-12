// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;
use nous_core::*;

impl KernelService {
    pub(super) async fn plan_maintenance(
        &self,
        input: k::PlanMaintenanceRequest,
    ) -> Result<k::MaintenancePlan> {
        let selected = longitudinal::need(required(input.selected, "selected")?)?;
        self.0
            .cognition
            .require_current_maintenance(&selected)
            .await?;
        let subject = selected.subject_id;
        let member_bytes = self
            .0
            .configuration
            .snapshot_for_subject(subject)?
            .get(nous_runtime::MEMBER_TEXT_MAX_BYTES)?;
        let sequence = self.0.store.authority_seq(subject).await?;
        let memory = self.require_memory()?;
        if selected.kind == "concept_maintenance" {
            return self.concept_maintenance_plan(&selected).await;
        }
        let scope = match selected.kind.as_str() {
            "episode_resegment" => {
                memory
                    .plan_episode_review(
                        subject,
                        &selected.scope_kind,
                        &selected.scope_ref,
                        selected.trigger_authority_seq,
                    )
                    .await?
            }
            "journal_review" | "journal_revalidate" => {
                memory
                    .plan_journal_review(subject, &selected.kind, &selected.scope_ref)
                    .await?
            }
            "memory_consolidate" => {
                memory
                    .plan_consolidation_scope(subject, &selected.scope_kind, &selected.scope_ref)
                    .await?
            }
            _ => return Err(Error::Invalid("maintenance kind has no model plan".into())),
        };
        let source = if selected.kind == "memory_consolidate" {
            consolidation_source(&scope)
        } else {
            None
        };
        let catalog = scope.basis_catalog();
        let mut plan = k::MaintenancePlan {
            subject_id: subject.0.to_string(),
            authority_seq: sequence,
            cognitive_now: Some(timestamp(self.0.cognition.now(subject))),
            status: scope.status,
            next_due: scope.next_due.map(timestamp),
            problem_code: scope.problem,
            ..Default::default()
        };
        plan.consolidation_source = source;
        if let Some(journal) = scope.journal {
            plan.target = Some(k::JournalTarget {
                journal_id: journal.object.journal_id.0.to_string(),
                expected_revision_id: journal.revision.journal_revision_id.0.to_string(),
                expected_epoch: journal.object.object_epoch,
                intent: "revalidate".into(),
            });
            plan.journal = Some(journal::journal_revision_proto(journal));
        }
        plan.sources = scope
            .episodes
            .iter()
            .map(|episode| k::EpisodePartitionSource {
                revision_id: episode.revision.episode_revision_id.0.to_string(),
                expected_epoch: episode.object.object_epoch,
            })
            .collect();
        plan.episodes = scope.episodes.into_iter().map(episode::view).collect();
        let (members, scope_exceeded) = self
            .maintenance_member_catalog(subject, &scope.occurrences, member_bytes as usize)
            .await?;
        plan.members = members;
        if selected.kind == "episode_resegment" && scope_exceeded {
            plan.status = "blocked".into();
            plan.problem_code = Some("historical_repair_scope_exceeded".into());
            plan.next_due = None;
        }
        plan.basis = catalog
            .into_iter()
            .map(|(key, basis)| k::BasisCatalogEntry {
                key,
                basis: Some(basis_proto(basis)),
            })
            .collect();
        if selected.kind == "memory_consolidate" && plan.status == "ready" {
            self.consolidation_context(&mut plan).await?;
        }
        if self.0.store.authority_seq(subject).await? != sequence {
            return Err(nous_core::DomainError::new(
                nous_core::DomainErrorCode::StaleRevision,
                "maintenance source snapshot changed during planning",
            )
            .into());
        }
        Ok(plan)
    }

    async fn concept_maintenance_plan(
        &self,
        selected: &nous_runtime::MaintenanceNeed,
    ) -> Result<k::MaintenancePlan> {
        let subject = selected.subject_id;
        let sequence = self.0.store.authority_seq(subject).await?;
        let memory = self.require_memory()?;
        {
            let focus = parse_reference(&selected.scope_kind, &selected.scope_ref)?;
            let concepts = match memory.plan_concepts(subject, focus).await {
                Ok(plan) => plan,
                Err(Error::NotFound(_)) => {
                    return Ok(k::MaintenancePlan {
                        subject_id: subject.0.to_string(),
                        authority_seq: sequence,
                        status: "obsolete".into(),
                        ..Default::default()
                    });
                }
                Err(error) => return Err(error),
            };
            Ok(k::MaintenancePlan {
                subject_id: subject.0.to_string(),
                authority_seq: concepts.authority_seq,
                cognitive_now: Some(timestamp(self.0.cognition.now(subject))),
                status: "ready".into(),
                concept_model_input_json: Some(concepts.model_input.to_string()),
                concept_catalog: Some(k::ConceptMaintenanceCatalog {
                    max_suggestions: concepts.policy.max_suggestions as u32,
                    config_digest: concepts.config_digest,
                    references: concepts
                        .references
                        .into_iter()
                        .map(|(key, reference)| k::ConceptReference {
                            key,
                            reference: Some(to_ref(reference)),
                        })
                        .collect(),
                    tags: concepts
                        .tags
                        .into_iter()
                        .map(|tag| k::ConceptTag {
                            key: tag.key,
                            target: Some(p::TagRevisionTarget {
                                tag_id: tag.target.tag_id.0.to_string(),
                                expected_revision_id: tag.target.expected_revision_id.to_string(),
                            }),
                        })
                        .collect(),
                    associations: concepts
                        .associations
                        .into_iter()
                        .map(|a| k::ConceptAssociation {
                            key: a.key,
                            association_id: a.id.0.to_string(),
                            from_key: a.from,
                            to_key: a.to,
                            relation: a.relation,
                        })
                        .collect(),
                    basis: concepts
                        .basis
                        .into_iter()
                        .map(|(key, basis)| k::ConceptBasis {
                            key,
                            basis: Some(super::concepts::association_basis_proto(basis)),
                        })
                        .collect(),
                }),
                ..Default::default()
            })
        }
    }
    async fn maintenance_member_catalog(
        &self,
        subject: SubjectId,
        occurrences: &[OccurrenceId],
        member_bytes: usize,
    ) -> Result<(Vec<k::ExperienceMember>, bool)> {
        let (members, exceeded) = self
            .require_memory()?
            .maintenance_members(subject, occurrences, member_bytes)
            .await?;
        let members = members
            .into_iter()
            .map(|member| {
                let occurrence = member.occurrence;
                k::ExperienceMember {
                    key: format!("occurrence:{}", occurrence.occurrence_id.0),
                    occurrence_id: occurrence.occurrence_id.0.to_string(),
                    recorded_seq: member.experience.recorded_seq,
                    observed_at: Some(timestamp(occurrence.observed_at)),
                    session_id: member.experience.session_id.map(|id| id.to_string()),
                    work_context_id: member.experience.work_context_id.map(|id| id.to_string()),
                    work_context_revision: member.experience.work_context_revision,
                    conversation_ref: occurrence.conversation_ref,
                    actor_entity_ref: occurrence
                        .actor_entity_ref
                        .map(|value| value.as_str().to_owned()),
                    source_class: occurrence.source_class.as_str().to_owned(),
                    text: member.text,
                    text_partial: member.text_partial,
                    occurred_time: Some(temporal_proto(&occurrence.occurred_time)),
                }
            })
            .collect();
        Ok((members, exceeded))
    }
}

fn consolidation_source(scope: &nous_memory::MaintenanceScope) -> Option<k::ExpectedCognition> {
    if let Some(journal) = &scope.journal {
        return Some(k::ExpectedCognition {
            reference: Some(to_ref(CognitiveRef::JournalRevision(
                journal.revision.journal_revision_id,
            ))),
            expected_epoch: journal.object.object_epoch,
            object_id: journal.object.journal_id.0.to_string(),
        });
    }
    scope.episodes.first().map(|episode| k::ExpectedCognition {
        reference: Some(to_ref(CognitiveRef::EpisodeRevision(
            episode.revision.episode_revision_id,
        ))),
        expected_epoch: episode.object.object_epoch,
        object_id: episode.object.episode_id.0.to_string(),
    })
}
