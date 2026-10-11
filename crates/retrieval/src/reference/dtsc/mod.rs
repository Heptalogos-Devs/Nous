// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

mod config;
mod curve;
pub(crate) mod field;
pub use config::*;
pub use curve::*;
pub use field::*;

use curve::{DtscCurveContext, dtsc_curve};
use nous_core::{Error, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
pub struct ReferenceDtscInput {
    pub dimension: usize,
    pub nodes: Vec<ReferenceDtscNode>,
    pub source_field: Vec<(i64, f64)>,
    pub tag_vectors: Vec<(i64, Vec<f32>)>,
    pub inbound: Vec<(i64, f64)>,
    pub anchor_gain: Vec<(i64, f64)>,
    pub edges: Vec<(i64, i64, f64)>,
    pub candidates: Vec<ReferenceDtscCandidate>,
    pub original_vector: Vec<f32>,
    pub enhanced_vector: Vec<f32>,
    pub query_geometry_state: ReferenceDtscGeometryState,
    pub config: ReferenceDtscConfig,
    pub top_k: usize,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReferenceDtscOutput {
    pub results: Vec<ReferenceDtscScore>,
    pub diagnostics: ReferenceDtscDiagnostics,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReferenceDtscDiagnostics {
    pub offered_candidates: usize,
    pub projected_candidates: usize,
    pub field_nodes: usize,
    pub contributing_candidates: usize,
    pub returned_candidates: usize,
    pub field_trusted: bool,
    pub field_entropy: f64,
    pub fallback_used: bool,
    pub fallback_reason: Option<String>,
}
fn fallback(
    scores: &[ReferenceDtscScore],
    field: &ReferenceDtscField,
    config: &ReferenceDtscConfig,
) -> Option<&'static str> {
    if !field.field_trusted {
        return Some("query-energy-field-low-trust");
    }
    let contributors = scores
        .iter()
        .filter(|s| s.geo_score > 0.0)
        .collect::<Vec<_>>();
    if contributors.is_empty() {
        return Some("no-candidate-curve-contacted-query-field");
    }
    if !config.field.fallback_to_knn_on_low_trust {
        return None;
    }
    let maximum = contributors.iter().map(|s| s.geo_score).fold(0.0, f64::max);
    let minimum = contributors
        .iter()
        .map(|s| s.geo_score)
        .fold(f64::INFINITY, f64::min);
    let evidence: f64 = contributors
        .iter()
        .map(|s| s.geo_strong_hits as f64 + 0.75 * s.geo_exact_hits as f64)
        .sum();
    let low = maximum < config.trust.min_geo_score.max(0.0)
        && evidence < config.trust.min_strong_evidence.max(0.0);
    if low
        && contributors.len() as f64 / (scores.len().max(1) as f64)
            < config.trust.min_candidate_coverage.clamp(0.0, 1.0)
    {
        Some("candidate-curve-readout-jointly-low-trust")
    } else if low
        && contributors.len() > 1
        && maximum - minimum < config.trust.min_geo_spread.max(0.0)
    {
        Some("candidate-curve-scores-lack-discrimination")
    } else {
        None
    }
}

pub fn reference_dtsc(input: &ReferenceDtscInput) -> Result<ReferenceDtscOutput> {
    if input.dimension == 0 {
        return Err(Error::Invalid("DTSC dimension must be positive".into()));
    }
    let config = input.config.normalized();
    let field = reference_dtsc_field(&ReferenceDtscFieldInput {
        dimension: input.dimension,
        nodes: input.nodes.clone(),
        source_field: input.source_field.clone(),
        tag_vectors: input.tag_vectors.clone(),
        inbound: input.inbound.clone(),
        sample_tags: Vec::new(),
        config: config.field.clone(),
    });
    let ctx = DtscCurveContext {
        config: &config,
        field: &field,
        inbound: &input.inbound,
        edges: input.edges.iter().map(|(a, b, w)| ((*a, *b), *w)).collect(),
        anchor: input.anchor_gain.iter().copied().collect(),
        original: &input.original_vector,
        enhanced: &input.enhanced_vector,
        geometry: &input.query_geometry_state,
    };
    let mut scores = input
        .candidates
        .iter()
        .map(|c| {
            if c.curve.chunk_vector.len() == input.dimension
                && c.curve
                    .tags
                    .iter()
                    .all(|t| t.vector.len() == input.dimension)
            {
                dtsc_curve(c, &ctx)
            } else {
                let mut curve = c.curve.clone();
                if curve.chunk_vector.len() != input.dimension {
                    curve.chunk_vector.clear();
                    curve.tags.clear();
                } else {
                    curve.tags.retain(|t| t.vector.len() == input.dimension);
                }
                dtsc_curve(
                    &ReferenceDtscCandidate {
                        curve,
                        score: c.score,
                    },
                    &ctx,
                )
            }
        })
        .collect::<Vec<_>>();
    let reason = fallback(&scores, &field, &config);
    let contributors = if reason.is_some() {
        0
    } else {
        scores.iter().filter(|s| s.geo_score > 0.0).count()
    };
    if reason.is_some() {
        for s in &mut scores {
            s.geo_bonus = 0.0;
            s.geo_base_bonus = 0.0;
            s.geo_aux_bonus = 0.0;
            s.score = s.original_knn_score;
            s.geo_effect = "neutral".into();
        }
    } else {
        scores.sort_by(|a, b| {
            b.score
                .total_cmp(&a.score)
                .then_with(|| b.original_knn_score.total_cmp(&a.original_knn_score))
        });
    }
    let limit = if input.top_k == 0 {
        scores.len()
    } else {
        input.top_k
    };
    scores.truncate(limit.max(1));
    Ok(ReferenceDtscOutput {
        diagnostics: ReferenceDtscDiagnostics {
            offered_candidates: input.candidates.len(),
            projected_candidates: input.candidates.len(),
            field_nodes: field.field_nodes.len(),
            contributing_candidates: contributors,
            returned_candidates: scores.len(),
            field_trusted: field.field_trusted,
            field_entropy: field.field_entropy,
            fallback_used: reason.is_some(),
            fallback_reason: reason.map(Into::into),
        },
        results: scores,
    })
}
