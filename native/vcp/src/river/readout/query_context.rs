//! Prepare the authorized query field and its anchor seeds.
use super::*;

pub(super) struct QueryContext {
    pub field: FieldWorkspace,
    pub seeds: Vec<(i64, f64)>,
    pub core_ids: HashSet<i64>,
    pub fallback_anchor: bool,
}

pub(super) fn query_context(input: &NativeInput, artifact: &NativeArtifact) -> QueryContext {
    let local_domain: HashSet<i64> = input.query_state.local_domain_ids.iter().copied().collect();
    let workspace = FieldWorkspace {
        local: normalize_field(&input.query_state.local_field),
        transfer: normalize_field(&input.query_state.transfer_field),
        local_domain,
        transfer_domain: input
            .query_state
            .transfer_domain_ids
            .iter()
            .copied()
            .collect(),
        source_ids: input
            .query_state
            .source_field
            .iter()
            .map(|entry| entry.0)
            .collect(),
        active_edges: input
            .query_state
            .river_edges
            .iter()
            .filter_map(|edge| {
                // Query support is conditioned on an observed, real transport edge.
                let flow = clamp01(if edge.normalized_flow != 0.0 {
                    edge.normalized_flow
                } else {
                    edge.flow
                });
                (flow >= input.config.minimum_river_edge_flow
                    && artifact.edge_weight(edge.source_id, edge.target_id) > 0.0)
                    .then_some(((edge.source_id, edge.target_id), flow))
            })
            .collect(),
    };
    let direct_ids: Vec<i64> = input
        .query_state
        .field_provenance
        .iter()
        .filter(|entry| {
            entry.hop == 0 && matches!(entry.source_type.as_str(), "core" | "lexical_seed" | "seed")
        })
        .map(|entry| entry.id)
        .collect();
    let fallback_anchor = direct_ids.is_empty();
    let anchor_ids: HashSet<i64> = if fallback_anchor {
        input
            .query_state
            .source_field
            .iter()
            .map(|entry| entry.0)
            .collect()
    } else {
        direct_ids.into_iter().collect()
    };
    let mut seeds: Vec<(i64, f64)> = input
        .query_state
        .source_field
        .iter()
        .filter(|entry| anchor_ids.contains(&entry.0) && entry.1 > 0.0)
        .copied()
        .collect();

    let maximum_seed = seeds.iter().map(|s| s.1).fold(0.0, f64::max);
    let core_ids: HashSet<i64> = input
        .query_state
        .field_provenance
        .iter()
        .filter(|p| p.hop == 0 && matches!(p.source_type.as_str(), "core" | "lexical_seed"))
        .map(|p| p.id)
        .collect();
    seeds.retain(|s| s.1 >= maximum_seed * 0.2 || core_ids.contains(&s.0));

    QueryContext {
        field: workspace,
        seeds,
        core_ids,
        fallback_anchor,
    }
}
