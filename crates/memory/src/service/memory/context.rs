// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;
use crate::service::accessibility_eligible;
use nous_runtime::{ReadContext, bounded_text};
use std::collections::{HashMap, HashSet};

impl MemoryService {
    /// Build context views from one complete Memory owner batch. The exact
    /// selected revision is carried through the public context contribution.
    pub async fn context_contributions(
        &self,
        context: &ReadContext,
        references: &[CognitiveRef],
        explicit_references: &HashSet<CognitiveRef>,
        max_items: usize,
        max_text_bytes: usize,
    ) -> Result<(Vec<ContextContribution>, Vec<Degradation>)> {
        let references = references
            .iter()
            .filter(|reference| {
                matches!(
                    reference,
                    CognitiveRef::Memory(_) | CognitiveRef::MemoryRevision(_)
                )
            })
            .take(max_items.min(256))
            .cloned()
            .collect::<Vec<_>>();
        if references.is_empty() {
            return Ok((Vec::new(), Vec::new()));
        }
        let views = self
            .memory_views_for_references_at_policy(
                context.subject,
                &references,
                context.temporal.clock_now,
                super::super::resolve_accessibility_policy(context.config_snapshot.as_ref())?,
                context.historical_view(),
            )
            .await?;
        let mut remaining = max_text_bytes;
        let mut contributions = Vec::with_capacity(references.len());
        let mut degradation = Vec::new();
        for reference in references {
            let Some(view) = views.get(&reference) else {
                degradation.push(Degradation {
                    code: "projection_source_unavailable".into(),
                    detail: Some(reference.to_string()),
                });
                continue;
            };
            if !matches!(view.object.acceptance_state, AcceptanceState::Accepted)
                || !matches!(view.object.integrity_state, IntegrityState::Valid)
                || !matches!(view.object.suppression_state, SuppressionState::Normal)
                || !matches!(view.object.purge_state, PurgeState::Normal)
            {
                degradation.push(Degradation {
                    code: "projection_source_unavailable".into(),
                    detail: Some(reference.to_string()),
                });
                continue;
            }
            let explicit = explicit_references.contains(&reference)
                && context.accessibility.exact_target_bypasses_auto_level;
            if !accessibility_eligible(
                view.accessibility_level,
                context.accessibility.effort,
                explicit,
            ) {
                degradation.push(Degradation {
                    code: "projection_source_inaccessible".into(),
                    detail: Some(reference.to_string()),
                });
                continue;
            }
            let text = if remaining == 0 {
                None
            } else {
                let bounded = bounded_text(view.revision.representation_text.clone(), remaining);
                remaining = remaining.saturating_sub(bounded.len());
                Some(bounded)
            };
            let evidence = basis_handles(&view.basis);
            let source_revision = CognitiveRef::MemoryRevision(view.revision.memory_revision_id);
            contributions.push(ContextContribution {
                source_revision: Some(source_revision),
                reference,
                semantic_role: view.revision.semantic_role.clone(),
                estimated_tokens: text
                    .as_ref()
                    .map(|text| ((text.len().saturating_add(3)) / 4).min(u32::MAX as usize) as u32),
                text,
                authority: AuthorityClass::SubjectCognition,
                freshness: FreshnessDescriptor {
                    occurred: view.temporal_evidence.occurred.clone(),
                    observed_at: view.temporal_evidence.observed_at,
                    valid_time: view.revision.valid_time.clone(),
                    formed_at: Some(view.revision.formed_at),
                    recorded_at: Some(view.revision.recorded_at),
                },
                provenance: ProvenanceSummary {
                    source_count: evidence.len(),
                    producer_signatures: Vec::new(),
                    note: None,
                },
                evidence,
                priority: 1.0,
                materialization: None,
            });
        }
        Ok((contributions, degradation))
    }

    /// Materialize Episode, Journal and CognitiveSchema through complete
    /// owner batches. Each family loader performs grouped primary and related
    /// reads, and this method associates every result with its input reference
    /// before applying the caller's ordered text budget.
    pub async fn contextual_revision_contributions(
        &self,
        context: &ReadContext,
        references: &[CognitiveRef],
        max_items: usize,
        max_text_bytes: usize,
    ) -> Result<(Vec<ContextContribution>, Vec<Degradation>)> {
        let selected = references
            .iter()
            .filter(|reference| {
                matches!(
                    reference,
                    CognitiveRef::Episode(_)
                        | CognitiveRef::EpisodeRevision(_)
                        | CognitiveRef::Journal(_)
                        | CognitiveRef::JournalRevision(_)
                        | CognitiveRef::CognitiveSchema(_)
                        | CognitiveRef::CognitiveSchemaRevision(_)
                )
            })
            .take(max_items.min(256))
            .cloned()
            .collect::<Vec<_>>();
        if selected.is_empty() {
            return Ok((Vec::new(), Vec::new()));
        }
        let episodes = selected
            .iter()
            .filter(|reference| {
                matches!(
                    reference,
                    CognitiveRef::Episode(_) | CognitiveRef::EpisodeRevision(_)
                )
            })
            .cloned()
            .collect::<Vec<_>>();
        let journals = selected
            .iter()
            .filter(|reference| {
                matches!(
                    reference,
                    CognitiveRef::Journal(_) | CognitiveRef::JournalRevision(_)
                )
            })
            .cloned()
            .collect::<Vec<_>>();
        let schemas = selected
            .iter()
            .filter(|reference| {
                matches!(
                    reference,
                    CognitiveRef::CognitiveSchema(_) | CognitiveRef::CognitiveSchemaRevision(_)
                )
            })
            .cloned()
            .collect::<Vec<_>>();
        let episodes = if episodes.is_empty() {
            HashMap::new()
        } else {
            self.episodes_by_references(context.subject, &episodes)
                .await?
        };
        let journals = if journals.is_empty() {
            HashMap::new()
        } else {
            self.journals_by_references(context.subject, &journals)
                .await?
        };
        let schemas = if schemas.is_empty() {
            HashMap::new()
        } else {
            self.schemas_by_references(context.subject, &schemas, context.historical_view())
                .await?
        };

        let mut remaining = max_text_bytes;
        let mut contributions = Vec::with_capacity(selected.len());
        let mut degradation = Vec::new();
        for reference in selected {
            let contribution = match &reference {
                CognitiveRef::Episode(_) | CognitiveRef::EpisodeRevision(_) => {
                    let Some(view) = episodes.get(&reference) else {
                        degradation.push(unavailable_context_source(&reference));
                        continue;
                    };
                    if !contextual_state_eligible(
                        view.object.acceptance_state,
                        view.object.integrity_state,
                        view.object.suppression_state,
                        view.object.purge_state,
                    ) {
                        degradation.push(unavailable_context_source(&reference));
                        continue;
                    }
                    let text = [
                        view.revision.title.as_deref(),
                        Some(&view.revision.boundary_explanation),
                    ]
                    .into_iter()
                    .flatten()
                    .filter(|value| !value.trim().is_empty())
                    .collect::<Vec<_>>()
                    .join("\n");
                    owner_contribution(
                        reference.clone(),
                        CognitiveRef::EpisodeRevision(view.revision.episode_revision_id),
                        "episode_summary",
                        text,
                        FreshnessDescriptor {
                            occurred: vec![view.revision.experience_time.clone()],
                            observed_at: None,
                            valid_time: TemporalExtent::Unknown,
                            formed_at: Some(view.revision.formed_at),
                            recorded_at: Some(view.revision.recorded_at),
                        },
                        basis_handles(&view.basis),
                        remaining,
                    )
                }
                CognitiveRef::Journal(_) | CognitiveRef::JournalRevision(_) => {
                    let Some(view) = journals.get(&reference) else {
                        degradation.push(unavailable_context_source(&reference));
                        continue;
                    };
                    if !contextual_state_eligible(
                        view.object.acceptance_state,
                        view.object.integrity_state,
                        view.object.suppression_state,
                        view.object.purge_state,
                    ) {
                        degradation.push(unavailable_context_source(&reference));
                        continue;
                    }
                    let text = std::iter::once(view.revision.narrative.as_str())
                        .filter(|text| !text.is_empty())
                        .chain(
                            view.points
                                .iter()
                                .map(|point| point.text.as_str())
                                .filter(|text| !text.trim().is_empty()),
                        )
                        .collect::<Vec<_>>()
                        .join("\n");
                    let mut evidence = view
                        .sources
                        .iter()
                        .cloned()
                        .map(|source| EvidenceHandle {
                            epistemic_relation: None,
                            reference: source,
                            basis_role: "journal_source".into(),
                        })
                        .collect::<Vec<_>>();
                    for point in &view.points {
                        evidence.extend(basis_handles(&point.basis));
                    }
                    owner_contribution(
                        reference.clone(),
                        CognitiveRef::JournalRevision(view.revision.journal_revision_id),
                        "journal_narrative",
                        text,
                        FreshnessDescriptor {
                            occurred: vec![view.revision.temporal_scope.clone()],
                            observed_at: None,
                            valid_time: TemporalExtent::Unknown,
                            formed_at: Some(view.revision.formed_at),
                            recorded_at: Some(view.revision.recorded_at),
                        },
                        evidence,
                        remaining,
                    )
                }
                CognitiveRef::CognitiveSchema(_) | CognitiveRef::CognitiveSchemaRevision(_) => {
                    let Some(view) = schemas.get(&reference) else {
                        degradation.push(unavailable_context_source(&reference));
                        continue;
                    };
                    if !contextual_state_eligible(
                        view.schema.acceptance_state,
                        view.schema.integrity_state,
                        view.schema.suppression_state,
                        view.schema.purge_state,
                    ) {
                        degradation.push(unavailable_context_source(&reference));
                        continue;
                    }
                    let evidence = view
                        .evidence_links
                        .iter()
                        .flat_map(|link| basis_handles(std::slice::from_ref(&link.basis)))
                        .collect();
                    owner_contribution(
                        reference.clone(),
                        CognitiveRef::CognitiveSchemaRevision(view.revision.schema_revision_id),
                        "schema_claim",
                        schema_context_text(&view.revision),
                        FreshnessDescriptor {
                            occurred: Vec::new(),
                            observed_at: None,
                            valid_time: view.revision.applicability_scope.valid_time.clone(),
                            formed_at: Some(view.revision.formed_at),
                            recorded_at: Some(view.revision.recorded_at),
                        },
                        evidence,
                        remaining,
                    )
                }
                _ => {
                    degradation.push(unavailable_context_source(&reference));
                    continue;
                }
            };
            let mut contribution = contribution;
            if let Some(text) = contribution.text.take() {
                let bounded = bounded_text(text, remaining);
                remaining = remaining.saturating_sub(bounded.len());
                contribution.estimated_tokens =
                    Some(((bounded.len().saturating_add(3)) / 4).min(u32::MAX as usize) as u32);
                contribution.text = Some(bounded);
            }
            contributions.push(contribution);
        }
        Ok((contributions, degradation))
    }
}

fn unavailable_context_source(reference: &CognitiveRef) -> Degradation {
    Degradation {
        code: "projection_source_unavailable".into(),
        detail: Some(reference.to_string()),
    }
}

fn contextual_state_eligible(
    acceptance: AcceptanceState,
    integrity: IntegrityState,
    suppression: SuppressionState,
    purge: PurgeState,
) -> bool {
    matches!(acceptance, AcceptanceState::Accepted)
        && matches!(integrity, IntegrityState::Valid)
        && matches!(suppression, SuppressionState::Normal)
        && matches!(purge, PurgeState::Normal)
}

fn basis_handles(basis: &[RevisionBasis]) -> Vec<EvidenceHandle> {
    basis
        .iter()
        .map(|basis| match basis {
            RevisionBasis::Evidence(value) => EvidenceHandle {
                epistemic_relation: value.epistemic_relation,
                reference: value.cognitive_ref(),
                basis_role: value.basis_role.as_str().into(),
            },
            RevisionBasis::CognitionDependency(value) => EvidenceHandle {
                epistemic_relation: value.epistemic_relation,
                reference: value.target_revision.clone(),
                basis_role: value.basis_role.as_str().into(),
            },
            RevisionBasis::Seed(value) => EvidenceHandle {
                epistemic_relation: None,
                reference: CognitiveRef::CognitiveSeedVersion(value.seed_version_id),
                basis_role: "seed".into(),
            },
        })
        .collect()
}

fn owner_contribution(
    reference: CognitiveRef,
    source_revision: CognitiveRef,
    semantic_role: &str,
    text: String,
    freshness: FreshnessDescriptor,
    evidence: Vec<EvidenceHandle>,
    max_text_bytes: usize,
) -> ContextContribution {
    let text = bounded_text(text, max_text_bytes);
    ContextContribution {
        source_revision: Some(source_revision),
        reference,
        semantic_role: semantic_role.into(),
        estimated_tokens: Some(((text.len().saturating_add(3)) / 4).min(u32::MAX as usize) as u32),
        text: Some(text),
        authority: AuthorityClass::SubjectCognition,
        freshness,
        provenance: ProvenanceSummary {
            source_count: evidence.len(),
            producer_signatures: Vec::new(),
            note: None,
        },
        evidence,
        priority: 1.0,
        materialization: None,
    }
}

fn schema_context_text(revision: &CognitiveSchemaRevision) -> String {
    [
        revision.title.as_deref(),
        Some(&revision.structural_claim),
        Some(&revision.applicability_scope.description),
        Some(&revision.boundary_definition),
    ]
    .into_iter()
    .flatten()
    .filter(|value| !value.trim().is_empty())
    .collect::<Vec<_>>()
    .join("\n")
}
