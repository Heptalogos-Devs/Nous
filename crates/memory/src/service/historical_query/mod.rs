//! Owner-direct lanes select bounded immutable refs before final owner materialization.
// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
use super::*;
use nous_runtime::{BoundQuery, LaneCandidate, LaneOutput, LaneStatus};
use std::collections::{BTreeSet, HashSet};
mod candidates;

pub(super) async fn direct_lanes(
    service: &MemoryService,
    bound: &BoundQuery,
) -> Result<Vec<LaneOutput>> {
    let mut outputs = vec![];
    for (family, variant) in [
        (EvidenceFamily::Entity, "entity:historical_aboutness"),
        (EvidenceFamily::Temporal, "temporal:historical_revision"),
        (EvidenceFamily::SchemaDirect, "schema:historical_support"),
    ] {
        if bound.plan.lane_enabled(family) {
            let references =
                candidates::select(service, bound, family, bound.plan.lane_budget(family) + 1)
                    .await?;
            outputs.push(lane(family, references, bound, variant));
        }
    }
    Ok(outputs)
}

fn lane(
    family: EvidenceFamily,
    references: Vec<CognitiveRef>,
    bound: &BoundQuery,
    variant: &str,
) -> LaneOutput {
    let budget = bound.plan.lane_budget(family);
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
