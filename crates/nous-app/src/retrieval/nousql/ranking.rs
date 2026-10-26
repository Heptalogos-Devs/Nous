use super::*;
use crate::retrieval::view::QueryView;

pub(in crate::retrieval) fn direct(
    view: &QueryView,
    request: &QueryRequest,
    leaf: &PlannedQuery,
) -> Vec<(String, f32)> {
    let targets = leaf
        .selectors
        .iter()
        .flat_map(|selector| &selector.targets)
        .collect::<BTreeSet<_>>();
    view.selected(request)
        .filter(|entry| targets.contains(&entry.reference()))
        .map(|entry| (entry.reference().to_string(), 1.0))
        .collect()
}
pub(in crate::retrieval) fn order(
    view: &QueryView,
    request: &QueryRequest,
    leaf: &PlannedQuery,
    mut ranks: Vec<(String, f32)>,
    constant: f32,
) -> Vec<(String, f32)> {
    let offered = ranks
        .iter()
        .map(|(id, _)| id.clone())
        .collect::<BTreeSet<_>>();
    let entries = view
        .selected(request)
        .filter(|entry| offered.contains(&entry.reference().to_string()))
        .map(|entry| (entry.reference().to_string(), entry))
        .collect::<BTreeMap<_, _>>();
    if !leaf.preferences.is_empty() {
        let mut lanes = vec![(ranks, 1.0)];
        for preference in &leaf.preferences {
            let mut matched = if let Some(text) = &preference.text {
                entries
                    .iter()
                    .filter_map(|(key, entry)| {
                        entry
                            .lexical_text
                            .as_ref()
                            .and_then(|body| nous_retrieval::match_score(text, body))
                            .map(|score| (key.clone(), score))
                    })
                    .collect::<Vec<_>>()
            } else {
                let axis = match preference.recent.as_deref() {
                    Some("occurred") => TimeAxis::Event,
                    Some("observed") => TimeAxis::Observation,
                    Some("formed") => TimeAxis::Formation,
                    Some("valid") => TimeAxis::Validity,
                    _ => TimeAxis::Recorded,
                };
                let mut local = request.clone();
                local.sort = Some(QuerySort {
                    axis: axis.clone(),
                    direction: SortDirection::Desc,
                    domain: Some(if matches!(axis, TimeAxis::Recorded) {
                        "unix_ms".into()
                    } else {
                        request
                            .clock
                            .as_ref()
                            .map(|clock| clock.domain.clone())
                            .unwrap_or_else(|| "unix_ms".into())
                    }),
                });
                let mut values = entries
                    .iter()
                    .filter_map(|(key, entry)| {
                        entry
                            .sort_time(&axis, &local)
                            .map(|time| (key.clone(), time.coordinate))
                    })
                    .collect::<Vec<_>>();
                values.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
                values.into_iter().map(|(key, _)| (key, 1.0)).collect()
            };
            if preference.text.is_some() {
                matched.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
            }
            lanes.push((matched, if preference.negative { -1.0 } else { 1.0 }));
        }
        ranks = crate::retrieval::query::rrf(lanes, usize::MAX, constant);
    }
    if let Some(sort) = &request.sort {
        ranks.sort_by(|(a, left_score), (b, right_score)| {
            let left = entries
                .get(a)
                .and_then(|entry| entry.sort_time(&sort.axis, request));
            let right = entries
                .get(b)
                .and_then(|entry| entry.sort_time(&sort.axis, request));
            sort.compare(left.as_ref(), right.as_ref())
                .then_with(|| right_score.total_cmp(left_score))
                .then_with(|| a.cmp(b))
        });
    }
    ranks
}
pub(in crate::retrieval) fn activations(leaf: &PlannedQuery) -> Vec<RetrievalTarget> {
    leaf.selectors
        .iter()
        .filter(|selector| selector.selector == SelectorKind::Tag)
        .flat_map(|selector| &selector.canonical)
        .filter_map(|value| {
            value
                .parse::<CognitiveRef>()
                .ok()
                .map(RetrievalTarget::Cognition)
        })
        .collect()
}
