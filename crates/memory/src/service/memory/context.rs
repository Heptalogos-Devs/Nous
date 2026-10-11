// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;
use crate::service::accessibility_eligible;
use nous_runtime::{ReadContext, bounded_text};
use std::collections::HashSet;

impl MemoryService {
    /// Materialize all four Cognition families through one owner batch, then
    /// apply lifecycle, Memory accessibility, caller order and text budget in
    /// one pass. The exact selected revision comes from the loaded typed view.
    pub async fn context_contributions(
        &self,
        context: &ReadContext,
        references: &[CognitiveRef],
        explicit_references: &HashSet<CognitiveRef>,
        max_items: usize,
        max_text_bytes: usize,
    ) -> Result<(Vec<ContextContribution>, Vec<Degradation>)> {
        let mut seen = HashSet::new();
        let selected = references
            .iter()
            .filter(|reference| CognitionKind::from_reference(reference).is_ok())
            .filter(|reference| seen.insert((*reference).clone()))
            .take(max_items.min(256))
            .cloned()
            .collect::<Vec<_>>();
        if selected.is_empty() {
            return Ok((Vec::new(), Vec::new()));
        }

        let views = self.read_cognitions(context, &selected).await?;
        let mut remaining = max_text_bytes;
        let mut contributions = Vec::with_capacity(selected.len());
        let mut degradation = Vec::new();
        for reference in selected {
            let Some(view) = views.get(&reference) else {
                degradation.push(unavailable_context_source(&reference));
                continue;
            };
            let header = view.header();
            if !contextual_state_eligible(header.lifecycle) {
                degradation.push(unavailable_context_source(&reference));
                continue;
            }
            let revision_header = view.revision_header();

            let (semantic_role, text, freshness, evidence) = match view {
                CognitionView::Memory(view) => {
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
                    (
                        view.revision.semantic_role.clone(),
                        view.revision.representation_text.clone(),
                        FreshnessDescriptor {
                            occurred: view.temporal_evidence.occurred.clone(),
                            observed_at: view.temporal_evidence.observed_at,
                            valid_time: view.revision.valid_time.clone(),
                            formed_at: Some(revision_header.formed_at),
                            recorded_at: Some(revision_header.recorded_at),
                        },
                        basis_handles(&view.basis),
                    )
                }
                CognitionView::Episode(view) => {
                    let text = [
                        view.revision.title.as_deref(),
                        Some(&view.revision.boundary_explanation),
                    ]
                    .into_iter()
                    .flatten()
                    .filter(|value| !value.trim().is_empty())
                    .collect::<Vec<_>>()
                    .join("\n");
                    (
                        "episode_summary".into(),
                        text,
                        FreshnessDescriptor {
                            occurred: vec![view.revision.experience_time.clone()],
                            observed_at: None,
                            valid_time: TemporalExtent::Unknown,
                            formed_at: Some(revision_header.formed_at),
                            recorded_at: Some(revision_header.recorded_at),
                        },
                        basis_handles(&view.basis),
                    )
                }
                CognitionView::Journal(view) => {
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
                    (
                        "journal_narrative".into(),
                        text,
                        FreshnessDescriptor {
                            occurred: vec![view.revision.temporal_scope.clone()],
                            observed_at: None,
                            valid_time: TemporalExtent::Unknown,
                            formed_at: Some(revision_header.formed_at),
                            recorded_at: Some(revision_header.recorded_at),
                        },
                        evidence,
                    )
                }
                CognitionView::CognitiveSchema(view) => {
                    let evidence = view
                        .evidence_links
                        .iter()
                        .flat_map(|link| basis_handles(std::slice::from_ref(&link.basis)))
                        .collect();
                    (
                        "schema_claim".into(),
                        schema_context_text(&view.revision),
                        FreshnessDescriptor {
                            occurred: Vec::new(),
                            observed_at: None,
                            valid_time: view.revision.applicability_scope.valid_time.clone(),
                            formed_at: Some(revision_header.formed_at),
                            recorded_at: Some(revision_header.recorded_at),
                        },
                        evidence,
                    )
                }
            };

            let text = if remaining == 0 {
                None
            } else {
                let bounded = bounded_text(text, remaining);
                remaining = remaining.saturating_sub(bounded.len());
                Some(bounded)
            };
            let estimated_tokens = text
                .as_ref()
                .map(|text| ((text.len().saturating_add(3)) / 4).min(u32::MAX as usize) as u32);
            contributions.push(ContextContribution {
                source_revision: Some(revision_header.revision_ref.clone()),
                reference,
                semantic_role,
                estimated_tokens,
                text,
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
            });
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

fn contextual_state_eligible(lifecycle: CognitionLifecycle) -> bool {
    matches!(lifecycle.acceptance, AcceptanceState::Accepted)
        && matches!(lifecycle.integrity, IntegrityState::Valid)
        && matches!(lifecycle.suppression, SuppressionState::Normal)
        && matches!(lifecycle.purge, PurgeState::Normal)
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
