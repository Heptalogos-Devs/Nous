// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::ReferenceTransport;
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReferenceFileTags {
    pub file_id: i64,
    pub tags: Vec<(i64, i64)>,
}
#[derive(Debug, Deserialize)]
pub struct ReferenceGraphInput {
    pub files: Vec<ReferenceFileTags>,
    pub pairwise: Vec<(i64, i64, f64)>,
    pub anchor_gain: Vec<(i64, f64)>,
    pub config: ReferenceGraphConfig,
}
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ReferenceGraphConfig {
    pub forward_gain: f64,
    pub reverse_gain: f64,
    pub min_reverse_gain: f64,
    pub max_reverse_gain: f64,
    pub distance_decay: f64,
    pub reverse_inversion_guard: f64,
    pub reverse_anchor_boost: bool,
    pub reverse_anchor_max: f64,
    pub semantic_enabled: bool,
    pub semantic_peak: f64,
    pub semantic_sigma: f64,
    pub semantic_low_fallback: f64,
    pub outbound_mass: f64,
    pub association_reserve_mass: f64,
    pub evidence_compression: f64,
    pub wormhole_gain: f64,
    pub tension_threshold: f64,
    pub hub_exponent: f64,
    pub hub_floor: f64,
    pub hub_ceiling: f64,
    pub smoothing_ratio: f64,
}
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ReferenceProvenanceEdge {
    pub source_id: i64,
    pub target_id: i64,
    pub file_contributions: Vec<(i64, f64)>,
}
#[derive(Debug, Serialize, Deserialize)]
pub struct ReferenceGraphOutput {
    pub fact_matrix: Vec<(i64, i64, f64)>,
    pub transport: ReferenceTransport,
    pub wormholes: Vec<(i64, i64)>,
    pub inbound: Vec<(i64, f64)>,
    pub provenance: Vec<ReferenceProvenanceEdge>,
}
fn add(map: &mut BTreeMap<(i64, i64), f64>, from: i64, to: i64, value: f64) {
    if from != to && value.is_finite() && value > 0.0 {
        *map.entry((from, to)).or_default() += value;
    }
}
type DirectedFacts = BTreeMap<(i64, i64), f64>;
fn file_facts(input: &ReferenceGraphInput) -> Vec<(i64, DirectedFacts)> {
    let config = &input.config;
    let pairwise = input
        .pairwise
        .iter()
        .map(|(a, b, value)| (((*a).min(*b), (*a).max(*b)), *value))
        .collect::<BTreeMap<_, _>>();
    let anchors = input
        .anchor_gain
        .iter()
        .copied()
        .collect::<BTreeMap<_, _>>();
    let mut output = Vec::new();
    for file in &input.files {
        let mut fact = BTreeMap::new();
        if !(2..=100).contains(&file.tags.len()) {
            continue;
        }
        let mut tags = file.tags.clone();
        tags.sort_by_key(|(id, position)| (*position, *id));
        for (index, (left, lpos)) in tags.iter().enumerate() {
            for (right, rpos) in tags.iter().skip(index + 1) {
                if left == right {
                    continue;
                }
                let sim = pairwise
                    .get(&((*left).min(*right), (*left).max(*right)))
                    .copied()
                    .unwrap_or(config.semantic_low_fallback);
                let semantic = if !config.semantic_enabled {
                    1.0
                } else if sim < 0.15 {
                    0.4 + sim
                } else {
                    0.5 + 0.8
                        * (-(sim - config.semantic_peak).powi(2)
                            / (2.0 * config.semantic_sigma.max(0.001).powi(2)))
                        .exp()
                };
                if *lpos <= 0 || *rpos <= 0 {
                    add(&mut fact, *left, *right, 0.49 * semantic);
                    add(&mut fact, *right, *left, 0.49 * semantic);
                    continue;
                }
                let span = (tags.len() as f64 - 1.0).max(1.0);
                let position = |pos: i64| 0.9 - 0.4 * (pos - 1).max(0) as f64 / span;
                let delta = (rpos - lpos).max(1) as f64;
                let base = position(*lpos)
                    * position(*rpos)
                    * (-config.distance_decay.max(0.0) * (delta - 1.0)).exp();
                let forward = base * config.forward_gain.max(0.0) * semantic;
                let mut reverse = config
                    .reverse_gain
                    .clamp(config.min_reverse_gain, config.max_reverse_gain);
                if config.reverse_anchor_boost {
                    reverse *= anchors
                        .get(left)
                        .copied()
                        .unwrap_or(1.0)
                        .min(config.reverse_anchor_max.max(1.0));
                }
                let backward = (base * reverse.clamp(0.25, 0.70) * semantic)
                    .min(forward * config.reverse_inversion_guard.clamp(0.0, 1.0));
                add(&mut fact, *left, *right, forward);
                add(&mut fact, *right, *left, backward);
            }
        }
        output.push((file.file_id, fact));
    }
    output
}

/// Prepare pairwise and anchor lookup once for the entire file collection.
pub fn reference_graph_file_facts(input: &ReferenceGraphInput) -> Result<Vec<ReferenceFileFacts>> {
    if input.config.min_reverse_gain > input.config.max_reverse_gain {
        return Err(Error::Invalid(
            "invalid reference reverse gain range".into(),
        ));
    }
    Ok(file_facts(input)
        .into_iter()
        .map(|(file_id, facts)| ReferenceFileFacts {
            file_id,
            facts: facts
                .into_iter()
                .map(|((a, b), mass)| (a, b, mass))
                .collect(),
        })
        .collect())
}

#[derive(Debug)]
pub struct ReferenceFileFacts {
    pub file_id: i64,
    pub facts: Vec<(i64, i64, f64)>,
}

pub fn reference_graph_facts(input: &ReferenceGraphInput) -> Result<Vec<(i64, i64, f64)>> {
    let mut facts = BTreeMap::new();
    for file in reference_graph_file_facts(input)? {
        for (from, to, mass) in file.facts {
            add(&mut facts, from, to, mass);
        }
    }
    Ok(facts
        .into_iter()
        .map(|((a, b), mass)| (a, b, mass))
        .collect())
}

pub fn reference_graph(input: &ReferenceGraphInput) -> Result<ReferenceGraphOutput> {
    reference_graph_from_facts(
        &reference_graph_facts(input)?,
        &input.anchor_gain,
        reference_provenance(input),
        &input.config,
    )
}

/// Build the frozen transport contract from owner-supplied directed facts.
/// Evidence roots are supplied separately, so adapters need not invent files
/// or turn unordered membership into a narrative sequence.
pub fn reference_graph_from_facts(
    facts: &[(i64, i64, f64)],
    anchor_gain: &[(i64, f64)],
    provenance: Vec<ReferenceProvenanceEdge>,
    config: &ReferenceGraphConfig,
) -> Result<ReferenceGraphOutput> {
    let mut fact = BTreeMap::new();
    for &(from, to, mass) in facts {
        if !mass.is_finite() || mass < 0.0 {
            return Err(Error::Invalid(
                "graph fact mass must be finite and nonnegative".into(),
            ));
        }
        add(&mut fact, from, to, mass);
    }
    let anchors = anchor_gain.iter().copied().collect::<BTreeMap<_, _>>();
    let mut rows = BTreeMap::<i64, Vec<(i64, f64, bool)>>::new();
    let mut inflow = BTreeMap::<i64, f64>::new();
    for ((from, to), basis) in &fact {
        let evidence = (1.0 + basis.max(0.0) * config.evidence_compression.max(0.01)).ln();
        let wormhole =
            evidence * anchors.get(to).copied().unwrap_or(1.0) >= config.tension_threshold.max(0.0);
        let weight = evidence
            * if wormhole {
                config.wormhole_gain.max(1.0)
            } else {
                1.0
            };
        if weight > 0.0 {
            rows.entry(*from).or_default().push((*to, weight, wormhole));
            *inflow.entry(*to).or_default() += weight;
        }
    }
    let mut values = inflow
        .values()
        .copied()
        .filter(|v| *v > 0.0)
        .collect::<Vec<_>>();
    values.sort_by(f64::total_cmp);
    let median = values.get(values.len() / 2).copied().unwrap_or(1.0);
    let smoothing = (median * config.smoothing_ratio.clamp(0.01, 2.0)).max(1e-9);
    let outbound = config.outbound_mass.clamp(0.01, 0.999999);
    let reserve = config.association_reserve_mass.clamp(0.0, outbound);
    let mut edges = BTreeMap::new();
    let mut wormholes = Vec::new();
    for (source, row) in rows {
        let adjusted = row
            .into_iter()
            .map(|(to, value, wormhole)| {
                let relative = (inflow[&to] / (median + smoothing)).max(1e-9);
                let penalty = relative.powf(-config.hub_exponent.clamp(0.0, 1.0)).clamp(
                    config.hub_floor.clamp(0.05, 1.0),
                    config.hub_ceiling.clamp(1.0, 4.0),
                );
                (to, value * penalty, wormhole)
            })
            .collect::<Vec<_>>();
        let total: f64 = adjusted.iter().map(|e| e.1).sum();
        let wormhole_total: f64 = adjusted.iter().filter(|e| e.2).map(|e| e.1).sum();
        if total <= 0.0 {
            continue;
        }
        let reserved = if wormhole_total > 0.0 { reserve } else { 0.0 };
        for (target, weight, wormhole) in adjusted {
            let value = (outbound - reserved) * weight / total
                + if wormhole && wormhole_total > 0.0 {
                    reserved * weight / wormhole_total
                } else {
                    0.0
                };
            edges.insert((source, target), value);
            if wormhole {
                wormholes.push((source, target));
            }
        }
    }
    let nodes = edges
        .keys()
        .flat_map(|(a, b)| [*a, *b])
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let indices = nodes
        .iter()
        .enumerate()
        .map(|(i, id)| (*id, i))
        .collect::<BTreeMap<_, _>>();
    let mut offsets = vec![0];
    let mut targets = Vec::new();
    let mut weights = Vec::new();
    let mut inbound = BTreeMap::<i64, f64>::new();
    for source in &nodes {
        for ((_, target), weight) in edges.range((*source, i64::MIN)..=(*source, i64::MAX)) {
            targets.push(indices[target]);
            weights.push(*weight);
            *inbound.entry(*target).or_default() += weight;
        }
        offsets.push(targets.len());
    }
    Ok(ReferenceGraphOutput {
        fact_matrix: fact.into_iter().map(|((a, b), v)| (a, b, v)).collect(),
        transport: ReferenceTransport {
            node_ids: nodes,
            row_offsets: offsets,
            targets,
            weights,
        },
        wormholes,
        inbound: inbound.into_iter().collect(),
        provenance,
    })
}

fn reference_provenance(input: &ReferenceGraphInput) -> Vec<ReferenceProvenanceEdge> {
    let config = &input.config;
    let mut provenance = BTreeMap::<(i64, i64), BTreeMap<i64, f64>>::new();
    for file in &input.files {
        if !(2..=100).contains(&file.tags.len()) {
            continue;
        }
        let mut tags = file.tags.clone();
        tags.sort_by_key(|(id, position)| (*position, *id));
        for (i, (a, apos)) in tags.iter().enumerate() {
            for (b, bpos) in tags.iter().skip(i + 1) {
                if a == b {
                    continue;
                }
                let distance =
                    (-config.distance_decay.max(0.0) * ((*bpos - *apos).max(1) as f64 - 1.0)).exp();
                for (source, target, mass) in [
                    (*a, *b, config.forward_gain.max(0.0) * distance),
                    (*b, *a, config.reverse_gain.max(0.0) * distance),
                ] {
                    *provenance
                        .entry((source, target))
                        .or_default()
                        .entry(file.file_id)
                        .or_default() += mass;
                }
            }
        }
    }
    provenance
        .into_iter()
        .map(|((source_id, target_id), values)| ReferenceProvenanceEdge {
            source_id,
            target_id,
            file_contributions: values.into_iter().collect(),
        })
        .collect()
}
