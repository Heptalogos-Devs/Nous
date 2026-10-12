// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::super::consolidation_context::member_basis;
use super::*;
pub(super) struct ConceptFocus {
    pub epoch: i64,
    pub text: String,
    pub basis: Vec<RevisionBasis>,
    pub entities: Vec<EntityRef>,
    pub owner_context: serde_json::Value,
    pub members: Vec<EpisodeMember>,
}

impl ConceptFocus {
    pub fn context(&self, basis: &BTreeMap<String, AssociationBasis>) -> serde_json::Value {
        let source_keys = basis
            .iter()
            .filter_map(|(key, basis)| {
                matches!(
                    basis,
                    AssociationBasis::Revision(RevisionBasis::Evidence(_))
                )
                .then_some(key)
            })
            .collect::<Vec<_>>();
        let mut context = self.owner_context.clone();
        if !self.members.is_empty() {
            context["orderedMembers"] = serde_json::json!(self.members.iter().take(8).map(|member| {
                let keys = basis.iter().filter_map(|(key,basis)| match (&member.reference, basis) {
                    (CognitiveRef::Occurrence(id), AssociationBasis::Revision(RevisionBasis::Evidence(e))) if *id == e.occurrence_id => Some(key),
                    (reference, AssociationBasis::Revision(RevisionBasis::CognitionDependency(d))) if *reference == d.target_revision => Some(key),
                    _ => None,
                }).collect::<Vec<_>>();
                serde_json::json!({"ordinal":member.ordinal,"role":member.role.chars().take(128).collect::<String>(),"basisKeys":keys})
            }).collect::<Vec<_>>());
        }
        serde_json::json!({"sourceBasisKeys":source_keys,"ownerContext":context})
    }
}

impl MemoryService {
    #[expect(
        clippy::too_many_lines,
        reason = "one current focus projection preserves four owned cognition content contracts"
    )]
    pub(super) async fn concept_focus(
        &self,
        subject: SubjectId,
        focus: &CognitiveRef,
        policy: &AccessibilityPolicy,
    ) -> Result<ConceptFocus> {
        let mut members = Vec::new();
        let (head, epoch, accepted, integrity, suppression, text, basis, entities, context) =
            match focus {
                CognitiveRef::MemoryRevision(id) => {
                    let memory = self
                        .memory_views(
                            subject,
                            &[id.0],
                            policy,
                            None,
                            false,
                            self.cognition.now(subject),
                        )
                        .await?
                        .remove(&id.0)
                        .ok_or_else(|| Error::NotFound("Memory concept focus not found".into()))?;
                    (
                        CognitiveRef::MemoryRevision(memory.object.current_revision_id),
                        memory.object.object_epoch,
                        memory.object.acceptance_state,
                        memory.object.integrity_state,
                        memory.object.suppression_state,
                        match memory.revision.title {
                            Some(title) => {
                                format!("{title}: {}", memory.revision.representation_text)
                            }
                            None => memory.revision.representation_text,
                        },
                        memory.basis,
                        memory.aboutness,
                        serde_json::json!({"cognitiveRole":memory.object.cognitive_role.as_str(),"semanticRole":memory.revision.semantic_role,"validTime":memory.revision.valid_time}),
                    )
                }
                CognitiveRef::CognitiveSchemaRevision(id) => {
                    let schema = self.schema_revision(subject, *id).await?;
                    let revision = schema.revision;
                    let text = format!(
                        "Title: {}\nClaim: {}\nApplicability: {}\nBoundary: {}",
                        revision.title.as_deref().unwrap_or_default(),
                        revision.structural_claim,
                        revision.applicability_scope.description,
                        revision.boundary_definition
                    );
                    (
                        CognitiveRef::CognitiveSchemaRevision(schema.schema.current_revision_id),
                        schema.schema.object_epoch,
                        schema.schema.acceptance_state,
                        schema.schema.integrity_state,
                        schema.schema.suppression_state,
                        text,
                        schema
                            .evidence_links
                            .into_iter()
                            .map(|link| link.basis)
                            .collect(),
                        revision.applicability_scope.aboutness,
                        serde_json::json!({"applicability":revision.applicability_scope.description.chars().take(512).collect::<String>(),"boundary":revision.boundary_definition.chars().take(512).collect::<String>(),"validTime":revision.applicability_scope.valid_time}),
                    )
                }
                CognitiveRef::EpisodeRevision(id) => {
                    let episode = self.episode_revision(subject, *id).await?;
                    let mut basis = episode.basis;
                    basis.extend(episode.members.iter().take(8).filter_map(member_basis));
                    members = episode.members;
                    let text = format!(
                        "{}\n{}",
                        episode.revision.title.as_deref().unwrap_or_default(),
                        episode.revision.boundary_explanation
                    );
                    (
                        CognitiveRef::EpisodeRevision(episode.object.current_revision_id),
                        episode.object.object_epoch,
                        episode.object.acceptance_state,
                        episode.object.integrity_state,
                        episode.object.suppression_state,
                        text,
                        basis,
                        Vec::new(),
                        serde_json::json!({"experienceTime":episode.revision.experience_time}),
                    )
                }
                CognitiveRef::JournalRevision(id) => {
                    let journal = self.journal_revision(subject, *id).await?;
                    let text = format!(
                        "{}\n{}",
                        journal.revision.title.as_deref().unwrap_or_default(),
                        journal.revision.narrative
                    );
                    (
                        CognitiveRef::JournalRevision(journal.object.current_revision_id),
                        journal.object.object_epoch,
                        journal.object.acceptance_state,
                        journal.object.integrity_state,
                        journal.object.suppression_state,
                        text,
                        journal
                            .points
                            .into_iter()
                            .flat_map(|point| point.basis)
                            .take(16)
                            .collect(),
                        Vec::new(),
                        serde_json::json!({"temporalScope":journal.revision.temporal_scope}),
                    )
                }
                _ => {
                    return Err(Error::Invalid(
                        "concept focus must be exact cognition".into(),
                    ));
                }
            };
        if head != *focus
            || accepted != AcceptanceState::Accepted
            || integrity != IntegrityState::Valid
            || suppression != SuppressionState::Normal
        {
            return Err(Error::NotFound(
                "concept focus is no longer current and eligible".into(),
            ));
        }
        Ok(ConceptFocus {
            epoch,
            text: text.chars().take(2048).collect(),
            basis,
            entities: entities.into_iter().take(8).collect(),
            owner_context: context,
            members,
        })
    }
    pub(super) async fn concept_source_text(
        &self,
        subject: SubjectId,
        basis: &BTreeMap<String, AssociationBasis>,
    ) -> Result<serde_json::Value> {
        let entries = basis
            .iter()
            .filter_map(|(key, basis)| match basis {
                AssociationBasis::Revision(RevisionBasis::Evidence(evidence)) => {
                    Some((key, evidence.clone()))
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        let evidence = entries
            .iter()
            .map(|(_, evidence)| evidence.clone())
            .collect::<Vec<_>>();
        let sources = self
            .material
            .evidence_text_views(subject, &evidence, 2048, None)
            .await?;
        let mut result = serde_json::Map::new();
        let mut remaining = 8192usize;
        for ((key, _), (occurrence, selected)) in entries.into_iter().zip(sources) {
            if remaining == 0 {
                break;
            }
            let mut text = selected.map(|selected| selected.text).unwrap_or_default();
            let mut end = text.len().min(remaining);
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            text.truncate(end);
            remaining -= text.len();
            result.insert(key.clone(), serde_json::json!({"text":text,"occurredTime":occurrence.occurred_time,"sourceClass":occurrence.source_class,"observedAt":occurrence.observed_at}));
        }
        Ok(serde_json::Value::Object(result))
    }
}
