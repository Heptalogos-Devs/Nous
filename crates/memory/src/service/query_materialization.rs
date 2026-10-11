// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;
use std::collections::BTreeMap;

pub(super) fn to_hit(
    query: &CognitiveQuery,
    view: &MemoryView,
    reference: CognitiveRef,
) -> CognitiveHit {
    CognitiveHit {
        authority_epoch: Some(view.header.object_epoch),
        preference_refs: view.tags.iter().copied().map(CognitiveRef::Tag).collect(),
        reference,
        revision: Some(CognitiveRef::MemoryRevision(MemoryRevisionId(
            view.revision.header.revision_id(),
        ))),
        semantic_role: Some(view.revision.semantic_role.clone()),
        cognitive_role: Some(view.header.details.memory_role().as_str().into()),
        formation_mode: Some(view.revision.formation_mode.as_str().into()),
        representation: Some(view.revision.representation_text.clone()),
        authority: AuthorityClass::SubjectCognition,
        freshness: FreshnessDescriptor {
            occurred: view.temporal_evidence.occurred.clone(),
            observed_at: view.temporal_evidence.observed_at,
            valid_time: view.revision.valid_time.clone(),
            formed_at: Some(view.revision.header.formed_at),
            recorded_at: Some(view.revision.header.recorded_at),
        },
        entity_refs: view.aboutness.clone(),
        evidence: if query.result_need.need_evidence {
            view.basis
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
        } else {
            Vec::new()
        },
        match_evidence: MatchEvidence::default(),
        materialization: Vec::new(),
    }
}

pub(super) fn hard_filter(
    query: &CognitiveQuery,
    view: &MemoryView,
    exact: bool,
    historical: bool,
) -> bool {
    (historical
        || (matches!(view.header.lifecycle.acceptance, AcceptanceState::Accepted)
            && matches!(view.header.lifecycle.integrity, IntegrityState::Valid)
            && matches!(view.header.lifecycle.suppression, SuppressionState::Normal)))
        && matches!(view.header.lifecycle.purge, PurgeState::Normal)
        && accessibility_eligible(view.accessibility_level, query.effort, exact || historical)
        && query.expression.constraints.matches_common(QueryFacts {
            authority: AuthorityClass::SubjectCognition,
            entities: &view.aboutness,
            source_classes: &view.source_classes,
            modality: Modality::Text,
            cognitive_role: Some(view.header.details.memory_role().as_str()),
            formation_mode: Some(view.revision.formation_mode.as_str()),
            epistemic_class: Some(view.revision.epistemic_class),
        })
        && temporal_match(query, view)
}

pub(super) fn temporal_match(query: &CognitiveQuery, view: &MemoryView) -> bool {
    query
        .expression
        .constraints
        .valid
        .is_none_or(|interval| interval.matches_extent(&view.revision.valid_time))
        && query
            .expression
            .constraints
            .occurred
            .is_none_or(|interval| {
                view.temporal_evidence
                    .occurred
                    .iter()
                    .any(|value| interval.matches_extent(value))
            })
        && query
            .expression
            .constraints
            .observed
            .is_none_or(|interval| {
                view.temporal_evidence
                    .observed_at
                    .is_some_and(|value| interval.contains(value))
            })
        && query
            .expression
            .constraints
            .formed
            .is_none_or(|interval| interval.contains(view.revision.header.formed_at))
        && query
            .expression
            .constraints
            .recorded
            .is_none_or(|interval| interval.contains(view.revision.header.recorded_at))
}

pub(super) fn increment_drop(counts: &mut BTreeMap<String, usize>, reason: &str) {
    *counts.entry(reason.into()).or_default() += 1;
}

pub(super) fn lifecycle_drop_reason(state: &CognitionLifecycle) -> &'static str {
    if !matches!(state.acceptance, AcceptanceState::Accepted) {
        "withdrawn"
    } else if !matches!(state.integrity, IntegrityState::Valid) {
        "revalidation_required"
    } else if !matches!(state.suppression, SuppressionState::Normal) {
        "suppressed"
    } else if !matches!(state.purge, PurgeState::Normal) {
        "purging"
    } else {
        "accessibility"
    }
}

pub(super) fn historical_drop_reason(
    view: &MemoryView,
    bound: &nous_runtime::BoundQuery,
) -> Option<&'static str> {
    let snapshot = bound.read_context.historical_view()?;
    if snapshot
        .cognition_for(&CognitiveRef::MemoryRevision(MemoryRevisionId(
            view.revision.header.revision_id(),
        )))
        .is_none()
    {
        return Some("outside_historical_view");
    }
    if view.header.lifecycle.purge != PurgeState::Normal {
        return Some("purged");
    }
    if view.header.lifecycle.acceptance != AcceptanceState::Accepted
        || view.header.lifecycle.integrity != IntegrityState::Valid
        || (view.header.lifecycle.suppression != SuppressionState::Normal
            && !bound.source_query.expression.constraints.include_suppressed)
    {
        return Some("historical_lifecycle");
    }
    None
}
