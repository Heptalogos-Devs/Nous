//! Owner-direct lanes select bounded immutable refs before final owner materialization.
// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
use super::*;
use nous_runtime::{BoundQuery, LaneCandidate, LaneOutput, LaneStatus, QueryPlan};
use std::collections::{BTreeSet, HashSet};
mod candidates;

pub(super) async fn direct_lanes(
    service: &MemoryService,
    bound: &BoundQuery,
    plan: &QueryPlan,
) -> Result<Vec<LaneOutput>> {
    let mut outputs = vec![];
    for (family, variant) in [
        (EvidenceFamily::Entity, "entity:historical_aboutness"),
        (EvidenceFamily::Temporal, "temporal:historical_revision"),
        (EvidenceFamily::SchemaDirect, "schema:historical_support"),
    ] {
        if bound.lane_enabled(family) {
            let references =
                candidates::select(service, bound, family, plan.lane_budget(family) + 1).await?;
            outputs.push(lane(family, references, plan, variant));
        }
    }
    Ok(outputs)
}

fn lane(
    family: EvidenceFamily,
    references: Vec<CognitiveRef>,
    plan: &QueryPlan,
    variant: &str,
) -> LaneOutput {
    let budget = plan.lane_budget(family);
    let mut output = LaneOutput::empty(
        family,
        if references.len() > budget {
            LaneStatus::Truncated
        } else {
            LaneStatus::Ready
        },
    );
    output.candidates = references
        .into_iter()
        .take(budget)
        .enumerate()
        .map(|(rank, reference)| LaneCandidate {
            reference,
            rank: (rank + 1) as u32,
            variants: vec![variant.into()],
            provider_metadata: serde_json::Value::Null,
        })
        .collect();
    output
}
