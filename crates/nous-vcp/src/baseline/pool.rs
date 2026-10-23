// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::unit;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Deserialize)]
pub struct ReferencePoolSignals {
    pub id: i64,
    pub query: f64,
    pub denoised: f64,
    pub local: f64,
    pub transfer: f64,
    pub bm25: f64,
    pub time: f64,
    pub anchor: f64,
}
#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(default, rename_all = "camelCase")]
pub struct ReferencePoolConfig {
    pub query_k: usize,
    pub denoised_k: usize,
    pub local_field_k: usize,
    pub transfer_field_k: usize,
    pub bm25_k: usize,
    pub anchor_k: usize,
    pub max_union_candidates: usize,
}
impl Default for ReferencePoolConfig {
    fn default() -> Self {
        Self {
            query_k: 100,
            denoised_k: 100,
            local_field_k: 100,
            transfer_field_k: 100,
            bm25_k: 50,
            anchor_k: 50,
            max_union_candidates: 300,
        }
    }
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReferenceSelectedCandidate {
    pub id: i64,
    pub union_score: f64,
    pub union_rank: usize,
    pub sources: Vec<String>,
}
pub fn reference_candidate_pool(
    signals: &[ReferencePoolSignals],
    config: &ReferencePoolConfig,
) -> Vec<ReferenceSelectedCandidate> {
    let channels = [
        ("query_knn", config.query_k),
        ("denoised_field_knn", config.denoised_k),
        ("local_field_knn", config.local_field_k),
        ("transfer_field_knn", config.transfer_field_k),
        ("bm25", config.bm25_k),
        ("time", config.query_k),
        ("anchor_direct", config.anchor_k),
    ];
    let mut support: BTreeMap<i64, Vec<(&str, f64, usize)>> = BTreeMap::new();
    for (channel, (name, limit)) in channels.into_iter().enumerate() {
        let mut ranked = signals
            .iter()
            .map(|s| {
                (
                    s.id,
                    [
                        s.query, s.denoised, s.local, s.transfer, s.bm25, s.time, s.anchor,
                    ][channel],
                )
            })
            .filter(|(_, score)| score.is_finite() && *score > 0.0)
            .collect::<Vec<_>>();
        ranked.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        ranked.truncate(limit.max(1));
        let minimum = ranked.iter().map(|(_, s)| *s).fold(f64::INFINITY, f64::min);
        let maximum = ranked
            .iter()
            .map(|(_, s)| *s)
            .fold(f64::NEG_INFINITY, f64::max);
        for (index, (id, score)) in ranked.into_iter().enumerate() {
            let rank = index + 1;
            let scaled = if maximum - minimum > 1e-12 {
                unit((score - minimum) / (maximum - minimum))
            } else {
                unit(1.0 / rank as f64)
            };
            support.entry(id).or_default().push((name, scaled, rank));
        }
    }
    let mut selected = support
        .into_iter()
        .map(|(id, entries)| {
            let maximum = entries.iter().map(|(_, s, _)| *s).fold(0.0, f64::max);
            let mean = entries.iter().map(|(_, s, _)| *s).sum::<f64>() / entries.len() as f64;
            let reciprocal = entries
                .iter()
                .map(|(_, _, rank)| 1.0 / (60.0 + *rank as f64))
                .sum::<f64>();
            let multi = (0.05 * entries.len().saturating_sub(1) as f64).min(0.2);
            ReferenceSelectedCandidate {
                id,
                union_score: unit(
                    0.5 * maximum + 0.25 * mean + 0.25 * unit(20.0 * reciprocal) + multi,
                ),
                union_rank: 0,
                sources: entries.iter().map(|(name, _, _)| (*name).into()).collect(),
            }
        })
        .collect::<Vec<_>>();
    selected.sort_by(|a, b| {
        b.sources
            .len()
            .cmp(&a.sources.len())
            .then_with(|| b.union_score.total_cmp(&a.union_score))
            .then_with(|| a.id.cmp(&b.id))
    });
    selected.truncate(config.max_union_candidates.max(1));
    for (i, c) in selected.iter_mut().enumerate() {
        c.union_rank = i + 1;
    }
    selected
}
