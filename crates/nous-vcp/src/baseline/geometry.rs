// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::{ReferenceCurve, anchors::reference_cosine};
use super::{positive, unit};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Deserialize)]
pub struct ReferencePathInput {
    pub curve: ReferenceCurve,
    pub local_field: Vec<(i64, f64)>,
    pub transfer_field: Vec<(i64, f64)>,
    pub local_domain: Vec<i64>,
    pub transfer_domain: Vec<i64>,
    pub edges: Vec<(i64, i64, f64)>,
    pub config: ReferencePathConfig,
}
#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(default, rename_all = "camelCase")]
pub struct ReferencePathConfig {
    pub local_weight: f64,
    pub transfer_weight: f64,
    pub direction_floor: f64,
    pub closure_floor: f64,
}
impl Default for ReferencePathConfig {
    fn default() -> Self {
        Self {
            local_weight: 0.6,
            transfer_weight: 0.4,
            direction_floor: 0.05,
            closure_floor: 0.0,
        }
    }
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReferencePathGeometry {
    pub path_quality: f64,
    pub path_core: f64,
    pub tag_closure: f64,
    pub support_coverage: f64,
    pub segment_count: usize,
    pub supported_segments: usize,
    pub transfer_segments: usize,
    pub mean_direction: f64,
    pub mean_continuity: f64,
    pub mean_local_potential: f64,
    pub mean_transfer_potential: f64,
}
pub(crate) fn normalized_field(entries: &[(i64, f64)]) -> BTreeMap<i64, f64> {
    let maximum = entries
        .iter()
        .map(|(_, v)| positive(*v))
        .fold(0.0, f64::max);
    entries
        .iter()
        .filter(|(id, v)| *id > 0 && positive(*v) > 0.0)
        .map(|(id, v)| {
            (
                *id,
                if maximum > 0.0 {
                    positive(*v) / maximum
                } else {
                    positive(*v)
                },
            )
        })
        .collect()
}

pub fn reference_path_geometry(input: &ReferencePathInput) -> ReferencePathGeometry {
    let config = &input.config;
    let local = normalized_field(&input.local_field);
    let transfer = normalized_field(&input.transfer_field);
    let ldomain = input.local_domain.iter().copied().collect::<BTreeSet<_>>();
    let tdomain = input
        .transfer_domain
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    let edges = input
        .edges
        .iter()
        .map(|(a, b, w)| ((*a, *b), positive(*w)))
        .collect::<BTreeMap<_, _>>();
    let at = |field: &BTreeMap<i64, f64>, id| field.get(&id).copied().unwrap_or(0.0);
    let edge = |a, b| edges.get(&(a, b)).copied().unwrap_or(0.0);
    let mut output = ReferencePathGeometry::default();
    let mut quality = 0.0;
    for pair in input.curve.tags.windows(2) {
        let a = &pair[0];
        let b = &pair[1];
        let lp = (at(&local, a.id) * at(&local, b.id)).sqrt();
        let tp = (at(&transfer, a.id) * at(&transfer, b.id)).sqrt();
        let forward = edge(a.id, b.id);
        let reverse = edge(b.id, a.id);
        let direction = if forward + reverse > 0.0 {
            unit(forward / (forward + reverse))
        } else {
            unit(config.direction_floor)
        };
        let semantic = unit((reference_cosine(&a.vector, &b.vector) + 1.0) / 2.0);
        let field = (lp.max(tp) * at(&local, b.id).max(at(&transfer, b.id))).sqrt();
        let continuity = unit(0.5 * semantic + 0.5 * field);
        let ls = ldomain.contains(&a.id) && ldomain.contains(&b.id);
        let ts = tdomain.contains(&a.id) && tdomain.contains(&b.id);
        let supported = (ls || ts) && (forward > 0.0 || reverse > 0.0);
        let potential = (config.local_weight * lp + config.transfer_weight * tp)
            / (config.local_weight + config.transfer_weight).max(1e-12);
        if supported {
            quality += unit(
                potential
                    * direction.max(config.direction_floor).sqrt()
                    * continuity.max(0.0).sqrt(),
            );
        }
        output.segment_count += 1;
        output.supported_segments += usize::from(supported);
        output.transfer_segments += usize::from(supported && ts && (!ls || tp > lp));
        output.mean_direction += direction;
        output.mean_continuity += continuity;
        output.mean_local_potential += lp;
        output.mean_transfer_potential += tp;
    }
    if output.segment_count > 0 {
        let count = output.segment_count as f64;
        output.path_core = unit(quality / count);
        output.support_coverage = output.supported_segments as f64 / count;
        output.mean_direction /= count;
        output.mean_continuity /= count;
        output.mean_local_potential /= count;
        output.mean_transfer_potential /= count;
    } else if let Some(tag) = input.curve.tags.first() {
        output.path_core = unit(at(&local, tag.id).max(at(&transfer, tag.id)) * 0.5);
    }
    if !input.curve.tags.is_empty() {
        output.tag_closure = input
            .curve
            .tags
            .iter()
            .map(|tag| {
                let cosine = unit(reference_cosine(&tag.vector, &input.curve.chunk_vector));
                unit((cosine - config.closure_floor) / (1.0 - config.closure_floor).max(1e-9))
            })
            .sum::<f64>()
            / input.curve.tags.len() as f64;
    }
    output.path_quality =
        unit(output.path_core * (0.5 + 0.25 * output.support_coverage + 0.25 * output.tag_closure));
    output
}
