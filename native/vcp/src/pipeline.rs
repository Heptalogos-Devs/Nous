//! Complete query pipeline.
//! Query execution consumes immutable typed snapshots.
// Preserve numerical decomposition and floating-point accumulation order.
#![allow(
    clippy::type_complexity,
    clippy::too_many_arguments,
    clippy::needless_range_loop
)]

use crate::assets::{MemoAssets, TagSearch, TagVectorPool, ensure_tag_vectors_loaded};
use crate::sensing::{
    DynamicWormholeContext, SenseConfig, SenseDiagnostics, SenseInput, SenseOutput, SenseSeed,
    sense_prevalidated,
};
use base64::Engine;
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Instant;
const PIPELINE_SCHEMA: &str = "vexus-unified-memo-pipeline-v1";
const PIPELINE_ALGORITHM: &str = "tagmemo.epa-pyramid-spike-fusion-v9.4-calibrated-rust";

fn positive(value: f64) -> f64 {
    if value.is_finite() {
        value.max(0.0)
    } else {
        0.0
    }
}

fn clamp01(value: f64) -> f64 {
    if value.is_finite() {
        value.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

fn magnitude(vector: &[f32]) -> f64 {
    vector
        .iter()
        .map(|value| (*value as f64) * (*value as f64))
        .sum::<f64>()
        .sqrt()
}

fn dot(left: &[f32], right: &[f32]) -> f64 {
    left.iter()
        .zip(right.iter())
        .map(|(a, b)| (*a as f64) * (*b as f64))
        .sum()
}

fn cosine(left: &[f32], right: &[f32]) -> f64 {
    if left.len() != right.len() || left.is_empty() {
        return 0.0;
    }
    let denominator = magnitude(left) * magnitude(right);
    if denominator > 1e-12 {
        dot(left, right) / denominator
    } else {
        0.0
    }
}

fn normalize(vector: &mut [f64]) -> bool {
    let norm = vector.iter().map(|value| value * value).sum::<f64>().sqrt();
    if norm <= 1e-12 {
        return false;
    }
    for value in vector {
        *value /= norm;
    }
    true
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PipelineInput {
    #[serde(default)]
    pub query_id: Option<String>,
    #[serde(default, rename = "queryText")]
    pub _query_text: String,
    #[serde(default)]
    pub query_vector: Vec<f32>,
    #[serde(default)]
    pub core_tags: Vec<String>,
    #[serde(default)]
    pub ghost_tags: Vec<GhostTagInput>,
    #[serde(default)]
    pub config: PipelineConfig,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GhostTagInput {
    pub name: String,
    #[serde(default)]
    pub vector: Vec<f32>,
    #[serde(default)]
    pub is_core: bool,
}

#[derive(Serialize, Deserialize, schemars::JsonSchema)]
#[serde(default, rename_all = "camelCase")]
pub struct PipelineConfig {
    pub base_tag_boost: f64,
    pub core_boost_factor: f64,
    pub local_alpha: f64,
    pub transfer_alpha: f64,
    pub field_max_iterations: usize,
    pub local_tolerance: f64,
    pub transfer_tolerance: f64,
    pub local_mass_ratio: f64,
    pub transfer_mass_ratio: f64,
    pub max_field_projection_nodes: usize,
    pub max_levels: usize,
    pub pyramid_top_k: usize,
    pub min_energy_ratio: f64,
    pub layer_decay: f64,
    pub activation_multiplier: Vec<f64>,
    pub dynamic_boost_range: Vec<f64>,
    pub core_boost_range: Vec<f64>,
    pub lang_confidence_enabled: bool,
    pub lang_penalty_unknown: f64,
    pub lang_penalty_cross_domain: f64,
    pub deduplication_threshold: f64,
    pub max_fusion_tags: usize,
    pub max_emergent_nodes: usize,
    pub tech_tag_threshold: f64,
    pub normal_tag_threshold: f64,
    pub epa_gate_mode: String,
    #[serde(default)]
    pub spike_routing: SenseConfig,
}

impl Default for PipelineConfig {
    fn default() -> Self {
        Self {
            base_tag_boost: 0.6,
            core_boost_factor: 1.33,
            local_alpha: 0.15,
            transfer_alpha: 0.55,
            field_max_iterations: 80,
            local_tolerance: 1e-9,
            transfer_tolerance: 1e-9,
            local_mass_ratio: 0.8,
            transfer_mass_ratio: 0.9,
            max_field_projection_nodes: 256,
            max_levels: 3,
            pyramid_top_k: 10,
            min_energy_ratio: 0.1,
            layer_decay: 0.7,
            activation_multiplier: vec![0.5, 1.5],
            dynamic_boost_range: vec![0.3, 2.0],
            core_boost_range: vec![1.2, 1.4],
            lang_confidence_enabled: true,
            lang_penalty_unknown: 0.05,
            lang_penalty_cross_domain: 0.2,
            deduplication_threshold: 0.88,
            max_fusion_tags: 128,
            max_emergent_nodes: 50,
            tech_tag_threshold: 0.08,
            normal_tag_threshold: 0.015,
            epa_gate_mode: "backgroundRelative".into(),
            spike_routing: SenseConfig::default(),
        }
    }
}

mod epa;
pub use epa::{EpaBasis, decode_epa_basis};
use epa::{EpaOutput, analyze_epa_with_mode};
mod pyramid;
use pyramid::{PyramidOutput, PyramidTagWork, analyze_pyramid_with_pool};
mod gating;
use gating::{GatedTag, gate_tags, technical_name};
mod fusion;
use fusion::{FusionDiagnostics, fuse_observation_with_pool};
mod fields;
use fields::{DualFieldDiagnostics, solve_dual_fields_with_pool};
mod output;
pub use output::PipelineOutput;
use output::{PipelineDiagnostics, PipelineMetadataOutput};
mod run;
pub use run::run_pipeline;
