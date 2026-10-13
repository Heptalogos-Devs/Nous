//! Future Authority references cannot enter a historical query's current context.
// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
use crate::WorkContextView;
use nous_core::*;
pub(super) fn fence(
    query: &mut CognitiveQuery,
    context: &mut Option<WorkContextView>,
    sources: &mut Vec<super::QuerySource>,
    view: Option<&HistoricalAuthoritySnapshot>,
) -> Vec<Degradation> {
    let Some(view) = view else {
        return Vec::new();
    };
    let visible = |reference: &CognitiveRef| {
        matches!(
            reference,
            CognitiveRef::Entity(_) | CognitiveRef::Resource(_) | CognitiveRef::ExternalObject(_)
        ) || view.contains(reference)
    };
    let excluded = sources
        .iter()
        .filter(|source| !visible(&source.reference))
        .map(|source| source.reference.to_string())
        .collect::<std::collections::BTreeSet<_>>();
    sources.retain(|source| visible(&source.reference));
    query.situation.current_refs.retain(visible);
    if let Some(context) = context {
        context.cognition_anchors.retain(visible);
        context
            .tag_anchors
            .retain(|tag| visible(&CognitiveRef::Tag(*tag)));
    }
    excluded
        .into_iter()
        .map(|reference| Degradation {
            code: "future_context_ref_excluded".into(),
            detail: Some(reference),
        })
        .collect()
}

pub(super) fn validate_view(
    query: &CognitiveQuery,
    historical_authority: Option<&HistoricalAuthoritySnapshot>,
) -> Result<()> {
    let required = query.temporal_frame.authority_view != AuthorityView::Current
        || query.temporal_frame.revision_view != RevisionView::Current;
    if required != historical_authority.is_some() {
        return Err(Error::Unavailable(
            "historical query requires its owner-projected Authority and Serving view".into(),
        ));
    }
    if let Some(view) = historical_authority {
        let cutoff = match query.temporal_frame.authority_view {
            AuthorityView::AsOf(at) => at,
            AuthorityView::Current => query.temporal_frame.clock_now,
        };
        if view.subject != query.subject
            || view.as_of != cutoff
            || view.revision_view != query.temporal_frame.revision_view
        {
            return Err(Error::Invalid(
                "historical snapshot disagrees with frozen query frame".into(),
            ));
        }
    }
    Ok(())
}
