// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::unit;
use super::{
    ReferenceCurve, ReferenceMorphology, ReferencePathGeometry, ReferenceRelativeTopology,
    anchors::reference_cosine, geometry::normalized_field,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Debug, Deserialize)]
pub struct ReferenceObservableInput {
    pub curve: ReferenceCurve,
    pub query_vector: Vec<f32>,
    pub source_ids: Vec<i64>,
    pub local_field: Vec<(i64, f64)>,
    pub transfer_field: Vec<(i64, f64)>,
    pub local_domain: Vec<i64>,
    pub transfer_domain: Vec<i64>,
    pub geometry: ReferencePathGeometry,
    pub visible: bool,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReferenceObservables {
    pub direct: f64,
    pub structural: f64,
    pub thematic: f64,
    pub closure: f64,
    pub query_chunk_score: f64,
    pub semantic_boundary_score: f64,
    pub local_coverage: f64,
    pub transfer_coverage: f64,
    pub local_potential: f64,
    pub transfer_potential: f64,
    pub tail_only_ratio: f64,
}

pub fn reference_observables(input: &ReferenceObservableInput) -> ReferenceObservables {
    let local = normalized_field(&input.local_field);
    let transfer = normalized_field(&input.transfer_field);
    let source = input.source_ids.iter().copied().collect::<BTreeSet<_>>();
    let ldomain = input.local_domain.iter().copied().collect::<BTreeSet<_>>();
    let tdomain = input
        .transfer_domain
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    let mut sums = [0.0; 6]; // exact, local/transfer contacts, local/transfer potential, tail contacts
    let mut boundary: f64 = 0.0;
    for tag in &input.curve.tags {
        let l = local.get(&tag.id).copied().unwrap_or(0.0);
        let t = transfer.get(&tag.id).copied().unwrap_or(0.0);
        sums[0] += f64::from(source.contains(&tag.id));
        sums[1] += f64::from(ldomain.contains(&tag.id));
        sums[2] += f64::from(tdomain.contains(&tag.id));
        sums[3] += l;
        sums[4] += t;
        sums[5] += f64::from(
            (l > 0.0 || t > 0.0) && !ldomain.contains(&tag.id) && !tdomain.contains(&tag.id),
        );
        let contact = unit(reference_cosine(&input.query_vector, &tag.vector));
        let closure = unit(reference_cosine(&tag.vector, &input.curve.chunk_vector));
        boundary = boundary.max((contact * closure).sqrt());
    }
    let denominator = input.curve.tags.len().max(1) as f64;
    let local_coverage = sums[1] / denominator;
    let transfer_coverage = sums[2] / denominator;
    let local_potential = sums[3] / denominator;
    let transfer_potential = sums[4] / denominator;
    let tail_only_ratio = sums[5] / denominator;
    let agreement = 1.0 - (unit(local_potential) - unit(transfer_potential)).abs();
    let thematic = unit(
        0.25 * local_coverage
            + 0.2 * transfer_coverage
            + 0.2 * unit(local_potential)
            + 0.15 * unit(transfer_potential)
            + 0.2 * agreement,
    ) * (1.0 - 0.5 * unit(tail_only_ratio));
    let query_chunk_score = unit(reference_cosine(
        &input.query_vector,
        &input.curve.chunk_vector,
    ));
    let semantic_boundary_score = if boundary >= 0.55 { boundary } else { 0.0 };
    let direct = if input.visible {
        unit((0.75 * unit(sums[0] / source.len().max(1) as f64)).max(semantic_boundary_score))
    } else {
        0.0
    };
    ReferenceObservables {
        direct,
        structural: input.geometry.path_quality,
        thematic,
        closure: unit(0.65 * query_chunk_score + 0.35 * input.geometry.tag_closure),
        query_chunk_score,
        semantic_boundary_score,
        local_coverage,
        transfer_coverage,
        local_potential,
        transfer_potential,
        tail_only_ratio,
    }
}

#[derive(Debug, Deserialize, Clone, Serialize, schemars::JsonSchema)]
#[serde(default, rename_all = "camelCase")]
pub struct ReferencePureConfig {
    pub pure_query_weight: f64,
    pub pure_local_weight: f64,
    pub pure_transfer_weight: f64,
    pub topology_bonus_cap: f64,
    pub topology_path_saturation: f64,
}
impl Default for ReferencePureConfig {
    fn default() -> Self {
        Self {
            pure_query_weight: 0.25,
            pure_local_weight: 0.2,
            pure_transfer_weight: 0.15,
            topology_bonus_cap: 0.08,
            topology_path_saturation: 0.15,
        }
    }
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReferencePureScores {
    pub semantic_base: f64,
    pub path_reward: f64,
    pub pure_score: f64,
    pub graph_score: f64,
    pub direct_evidence: f64,
}
pub struct ReferencePureInput<'a> {
    pub query_score: f64,
    pub local_score: f64,
    pub transfer_score: f64,
    pub geometry: &'a ReferencePathGeometry,
    pub observables: &'a ReferenceObservables,
    pub topology: &'a ReferenceRelativeTopology,
    pub morphology: &'a ReferenceMorphology,
    pub config: &'a ReferencePureConfig,
}
pub fn reference_pure_scores(input: &ReferencePureInput<'_>) -> ReferencePureScores {
    let cfg = input.config;
    let o = input.observables;
    let g = input.geometry;
    let t = input.topology;
    let m = input.morphology;
    let weight =
        (cfg.pure_query_weight + cfg.pure_local_weight + cfg.pure_transfer_weight).max(1e-12);
    let semantic_base = unit(
        (cfg.pure_query_weight * unit(input.query_score)
            + cfg.pure_local_weight * unit(input.local_score)
            + cfg.pure_transfer_weight * unit(input.transfer_score))
            / weight,
    );
    let basis = unit(
        0.35 * o.local_coverage
            + 0.25 * o.transfer_coverage
            + 0.25 * unit(o.local_potential)
            + 0.15 * unit(o.transfer_potential),
    );
    let raw = unit(0.625 * g.path_quality + 0.375 * basis);
    let reliability = (unit(g.path_quality / cfg.topology_path_saturation.max(1e-6))
        * o.query_chunk_score)
        .sqrt();
    let path_reward = (cfg.topology_bonus_cap * raw * reliability).min(cfg.topology_bonus_cap);
    let mixtures = [
        (m.atomic_weight, 0.75),
        (m.propositional_weight, 0.25),
        (m.narrative_weight, 0.15),
    ];
    let graph_score = unit(
        mixtures
            .iter()
            .map(|(weight, node_share)| {
                weight * (node_share * t.node_graph_score + (1.0 - node_share) * t.edge_graph_score)
            })
            .sum(),
    );
    ReferencePureScores {
        semantic_base,
        path_reward,
        pure_score: unit(semantic_base + path_reward),
        graph_score,
        direct_evidence: o.semantic_boundary_score.max(o.direct),
    }
}
