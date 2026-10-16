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
            temporal_evidence: candidate.view.temporal_evidence.clone(),
            valid_time: DomainTemporalExtent {
                time_domain_ref: candidate.view.revision.valid_time_domain_ref.clone(),
                extent: candidate.view.revision.valid_time.clone(),
            },
            formed_at: Some(DomainStamp {
                time_domain_ref: candidate.view.revision.formed_time_domain_ref.clone(),
                at: candidate.view.revision.formed_at,
                recorded_at: candidate.view.revision.recorded_at,
            }),
            recorded_at: Some(candidate.view.revision.recorded_at),
        },
        entity_refs: candidate.view.aboutness.clone(),
        evidence: if query.result_need.need_evidence {
            candidate
                .view
                .basis
                .iter()
                .map(RevisionBasis::evidence_handle)
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
    query.temporal_predicate(TimeAxis::Valid).is_none_or(|p| {
        p.matches_extent(&DomainTemporalExtent {
            time_domain_ref: view.revision.valid_time_domain_ref.clone(),
            extent: view.revision.valid_time.clone(),
        }) == Some(true)
    }) && view.temporal_evidence.matches(
        query.temporal_predicate(TimeAxis::Occurred).as_ref(),
        query.temporal_predicate(TimeAxis::Observed).as_ref(),
    ) && query.temporal_predicate(TimeAxis::Formed).is_none_or(|p| {
        p.matches_stamp(&DomainStamp {
            time_domain_ref: view.revision.formed_time_domain_ref.clone(),
            at: view.revision.formed_at,
            recorded_at: view.revision.recorded_at,
        }) == Some(true)
    }) && query
        .expression
        .constraints
        .recorded
        .is_none_or(|p| p.contains(view.revision.recorded_at))
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
