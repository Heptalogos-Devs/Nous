//! Complete RiverMemo topology V3 readout copied and adapted from VCPToolBox.
//! Commit d1b40c863273cbbba082fb7657671424768c4a53, CC BY-NC-SA 4.0.
//! Host observation handles and SQLite reads are immutable typed parameters.
// Retain the upstream numerical decomposition and floating-point accumulation order.
#![allow(clippy::type_complexity)]

use crate::assets::{MemoAssets, TagVectorPool, ensure_tag_vectors_loaded};
use crate::graph::NativeArtifact;
use crate::pipeline::PipelineOutput;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Instant;
const RESULT_SCHEMA: &str = "rivermemo-topology-v3-native-result-v1";
const ALGORITHM_VERSION: &str = "rivermemo.topology-v3.1-rust";
const MAX_TAG_PAIR_COSINES: usize = 1 << 22;

fn clamp01(value: f64) -> f64 {
    if value.is_finite() {
        value.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

fn positive(value: f64) -> f64 {
    if value.is_finite() {
        value.max(0.0)
    } else {
        0.0
    }
}

fn cosine(left: &[f32], right: &[f32]) -> f64 {
    if left.len() != right.len() || left.is_empty() {
        return 0.0;
    }
    let mut dot = 0.0;
    let mut left_norm = 0.0;
    let mut right_norm = 0.0;
    for index in 0..left.len() {
        let a = left[index] as f64;
        let b = right[index] as f64;
        dot += a * b;
        left_norm += a * a;
        right_norm += b * b;
    }
    cosine_from_parts(dot, left_norm, right_norm)
}

fn cosine_from_parts(dot: f64, left_norm: f64, right_norm: f64) -> f64 {
    if left_norm > 1e-15 && right_norm > 1e-15 {
        dot / (left_norm.sqrt() * right_norm.sqrt())
    } else {
        0.0
    }
}

fn squared_norm(vector: &[f32]) -> f64 {
    let mut total = 0.0;
    for value in vector {
        let value = *value as f64;
        total += value * value;
    }
    total
}

fn cosine_with_norms(left: &[f32], left_norm: f64, right: &[f32], right_norm: f64) -> f64 {
    if left.len() != right.len() || left.is_empty() {
        return 0.0;
    }
    let mut dot = 0.0;
    for index in 0..left.len() {
        dot += left[index] as f64 * right[index] as f64;
    }
    cosine_from_parts(dot, left_norm, right_norm)
}

struct CosineCache {
    columns: HashMap<i64, usize>,
    column_norms: Vec<f64>,
    rows: HashMap<i64, usize>,
    row_norms: Vec<f64>,
    /// `cosine(river or seed Tag, candidate Tag)`, absent above the size bound.
    tag_pairs: Option<Vec<Vec<f64>>>,
    /// `cosine(query vector, candidate Tag)`; the query is not a Tag.
    query_tags: Vec<f64>,
}

impl CosineCache {
    fn build(
        curves: &[Curve],
        query: &[f32],
        row_ids: impl IntoIterator<Item = i64>,
        vectors: &TagVectorPool,
    ) -> Self {
        let mut columns = HashMap::new();
        let mut column_vectors: Vec<Arc<[f32]>> = Vec::new();
        for tag in curves.iter().flat_map(|curve| &curve.tags) {
            if let std::collections::hash_map::Entry::Vacant(entry) = columns.entry(tag.id) {
                entry.insert(column_vectors.len());
                column_vectors.push(tag.vector.clone());
            }
        }
        let mut rows = HashMap::new();
        let mut row_vectors: Vec<Arc<[f32]>> = Vec::new();
        for id in row_ids {
            let Some(vector) = vectors.get(&id) else {
                continue;
            };
            if let std::collections::hash_map::Entry::Vacant(entry) = rows.entry(id) {
                entry.insert(row_vectors.len());
                row_vectors.push(vector.clone());
            }
        }
        let column_norms: Vec<f64> = column_vectors.par_iter().map(|v| squared_norm(v)).collect();
        let row_norms: Vec<f64> = row_vectors.iter().map(|v| squared_norm(v)).collect();
        let query_norm = squared_norm(query);
        let query_tags = column_vectors
            .par_iter()
            .zip(column_norms.par_iter())
            .map(|(column, norm)| cosine_with_norms(query, query_norm, column, *norm))
            .collect::<Vec<f64>>();
        let tag_pairs = (row_vectors.len().saturating_mul(column_vectors.len())
            <= MAX_TAG_PAIR_COSINES)
            .then(|| {
                row_vectors
                    .par_iter()
                    .zip(row_norms.par_iter())
                    .map(|(row, row_norm)| {
                        column_vectors
                            .iter()
                            .zip(&column_norms)
                            .map(|(column, norm)| cosine_with_norms(row, *row_norm, column, *norm))
                            .collect::<Vec<f64>>()
                    })
                    .collect::<Vec<Vec<f64>>>()
            });
        Self {
            columns,
            column_norms,
            rows,
            row_norms,
            tag_pairs,
            query_tags,
        }
    }

    /// Same value as `cosine(left_vector, &tag.vector)`, where `left_vector`
    /// is the pool vector of `left_id`.
    fn tag_pair(&self, left_id: i64, left_vector: &[f32], tag: &TagData) -> f64 {
        match (self.rows.get(&left_id), self.columns.get(&tag.id)) {
            (Some(&row), Some(&column)) => match &self.tag_pairs {
                Some(table) => table[row][column],
                None => cosine_with_norms(
                    left_vector,
                    self.row_norms[row],
                    &tag.vector,
                    self.column_norms[column],
                ),
            },
            _ => cosine(left_vector, &tag.vector),
        }
    }

    /// Same value as `cosine(query, &tag.vector)` for the request query vector.
    fn query_tag(&self, query: &[f32], tag: &TagData) -> f64 {
        match self.columns.get(&tag.id) {
            Some(&column) => self.query_tags[column],
            None => cosine(query, &tag.vector),
        }
    }
}

mod types;
use types::{
    Alignment, AnchorOutput, Curve, CurveLoadOutput, FieldWorkspace, NativeInput, QueryInput,
    QueryStateInput, RiverEdge, RiverNode, ScoredWork, SourceProvenance, TagData, default_top_k,
};
pub use types::{
    CandidateInput, GeometryOutput, NativeConfig, NativeDiagnostics, NativeOutput,
    NativeResultItem, ObservableOutput, OmegaOutput, QueryMorphology, TopologyOutput,
};
mod curves;
use curves::{
    FileTagRows, attach_tags, compute_anchor_scores, load_curves, normalize_field, select_superset,
};
mod topology;
use topology::{evaluate_path, evaluate_topology};
mod anchors;
use anchors::{compute_anchors, evaluate_observables};
mod morphology;
use morphology::{compute_omega, compute_query_morphology};
mod scoring;
use scoring::assign_v3_scores;
mod readout;
pub use readout::{ReadoutRequest, readout};
