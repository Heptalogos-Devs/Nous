// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::query::{Candidate, FinalMemoryState};
use super::*;
use std::collections::BTreeMap;

pub(super) fn final_state_filter(
    query: &CognitiveQuery,
    candidate: &Candidate,
    state: &FinalMemoryState,
    exact: bool,
    historical: bool,
) -> bool {
    if !matches!(state.purge_state, PurgeState::Normal) {
        return false;
    }
    if !historical
        && (!matches!(state.acceptance_state, AcceptanceState::Accepted)
            || !matches!(state.integrity_state, IntegrityState::Valid)
            || !matches!(state.suppression_state, SuppressionState::Normal))
    {
        return false;
    }
    if !hard_filter(query, &candidate.view, exact, historical) {
        return false;
    }
    let level = match state.accessibility_mode {
        AccessibilityMode::Normal => AccessibilityLevel::Normal,
        AccessibilityMode::Deep => AccessibilityLevel::Deep,
        AccessibilityMode::Explicit => AccessibilityLevel::Explicit,
        AccessibilityMode::Auto => candidate.view.accessibility_level,
    };
    accessibility_eligible(level, query.effort, exact || historical)
}

pub(super) fn to_hit(
    query: &CognitiveQuery,
    candidate: &Candidate,
    reference: CognitiveRef,
) -> CognitiveHit {
    CognitiveHit {
        authority_epoch: Some(candidate.view.object.object_epoch),
        preference_refs: candidate
            .view
            .tags
            .iter()
            .copied()
            .map(CognitiveRef::Tag)
            .collect(),
        reference,
        revision: Some(CognitiveRef::MemoryRevision(
            candidate.view.revision.memory_revision_id,
        )),
        semantic_role: Some(candidate.view.revision.semantic_role.clone()),
        cognitive_role: Some(candidate.view.object.cognitive_role.as_str().into()),
        formation_mode: Some(candidate.view.revision.formation_mode.as_str().into()),
        representation: Some(candidate.view.revision.representation_text.clone()),
        authority: AuthorityClass::SubjectCognition,
        freshness: FreshnessDescriptor {
            occurred: candidate.view.temporal_evidence.occurred.clone(),
            observed_at: candidate.view.temporal_evidence.observed_at,
            valid_time: candidate.view.revision.valid_time.clone(),
            formed_at: Some(candidate.view.revision.formed_at),
            recorded_at: Some(candidate.view.revision.recorded_at),
        },
        entity_refs: candidate.view.aboutness.clone(),
        evidence: if query.result_need.need_evidence {
            candidate
                .view
                .basis
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
        || (matches!(view.object.acceptance_state, AcceptanceState::Accepted)
            && matches!(view.object.integrity_state, IntegrityState::Valid)
            && matches!(view.object.suppression_state, SuppressionState::Normal)))
        && matches!(view.object.purge_state, PurgeState::Normal)
        && accessibility_eligible(view.accessibility_level, query.effort, exact || historical)
        && query.expression.constraints.matches_common(QueryFacts {
            authority: AuthorityClass::SubjectCognition,
            entities: &view.aboutness,
            source_classes: &view.source_classes,
            modality: Modality::Text,
            cognitive_role: Some(view.object.cognitive_role.as_str()),
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
            .is_none_or(|interval| interval.contains(view.revision.formed_at))
        && query
            .expression
            .constraints
            .recorded
            .is_none_or(|interval| interval.contains(view.revision.recorded_at))
}

pub(super) fn increment_drop(counts: &mut BTreeMap<String, usize>, reason: &str) {
    *counts.entry(reason.into()).or_default() += 1;
}

pub(super) fn lifecycle_drop_reason(state: &FinalMemoryState) -> String {
    if !matches!(state.acceptance_state, AcceptanceState::Accepted) {
        "withdrawn".into()
    } else if !matches!(state.integrity_state, IntegrityState::Valid) {
        "revalidation_required".into()
    } else if !matches!(state.suppression_state, SuppressionState::Normal) {
        "suppressed".into()
    } else if !matches!(state.purge_state, PurgeState::Normal) {
        "purging".into()
    } else {
        "accessibility".into()
    }
}

pub(super) fn apply_historical_header(
    view: &mut MemoryView,
    bound: &nous_runtime::BoundQuery,
) -> Result<Option<&'static str>> {
    let Some(snapshot) = bound.historical_authority.as_deref() else {
        return Ok(None);
    };
    let Some(state) = snapshot.cognition_for(&CognitiveRef::MemoryRevision(
        view.revision.memory_revision_id,
    )) else {
        return Ok(Some("outside_historical_view"));
    };
    if view.object.purge_state != PurgeState::Normal {
        return Ok(Some("purged"));
    }
    view.object = serde_json::from_value(state.state.clone())
        .map_err(|e| Error::Infrastructure(format!("historical Memory header: {e}")))?;
    if view.object.acceptance_state != AcceptanceState::Accepted
        || view.object.integrity_state != IntegrityState::Valid
        || (view.object.suppression_state != SuppressionState::Normal
            && !bound.source_query.expression.constraints.include_suppressed)
    {
        return Ok(Some("historical_lifecycle"));
    }
    Ok(None)
}
