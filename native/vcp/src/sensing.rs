//! Consumes immutable typed inputs.
use crate::assets::{TagVectorPool, ensure_tag_vectors_loaded};
use crate::graph::NativeArtifact;

use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::collections::HashMap;
use std::time::Instant;

const OBSERVATION_SCHEMA: &str = "vexus-unified-memo-observation-v1";
const ALGORITHM_VERSION: &str = "tagmemo.spike-v9.1-rust-shared";

fn positive(value: f64) -> f64 {
    if value.is_finite() {
        value.max(0.0)
    } else {
        0.0
    }
}

fn default_energy() -> f64 {
    1.0
}

fn default_source_type() -> String {
    "seed".to_string()
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SenseInput {
    #[serde(default)]
    pub query_id: Option<String>,
    #[serde(default)]
    pub seeds: Vec<SenseSeed>,
    #[serde(default)]
    pub config: SenseConfig,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SenseSeed {
    pub id: i64,
    #[serde(default = "default_energy")]
    pub energy: f64,
    #[serde(default = "default_source_type")]
    pub source_type: String,
}

#[derive(Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(default, rename_all = "camelCase")]
pub struct SenseConfig {
    pub max_safe_hops: usize,
    pub base_momentum: f64,
    pub firing_threshold: f64,
    pub base_decay: f64,
    pub wormhole_decay: f64,
    pub tension_threshold: f64,
    pub max_neighbors_per_node: usize,
    pub return_flow_factor: f64,
    pub fir_gamma: f64,
    pub max_propagation_states: usize,
    pub minimum_injected_current: f64,
    pub max_output_nodes: usize,
    pub max_output_edges: usize,
    /// 0 disables route recording. Recording never changes propagation.
    pub max_transition_records: usize,
    /// Only the pipeline supplies residual context; standalone sensing stays legacy.
    pub dynamic_wormholes: bool,
    pub dynamic_wormhole_threshold: f64,
    pub dynamic_wormhole_slots: usize,
    /// Request-wide unique target budget. Zero disables local vector work.
    pub dynamic_wormhole_max_targets: usize,
}

impl Default for SenseConfig {
    fn default() -> Self {
        Self {
            max_safe_hops: 4,
            base_momentum: 2.0,
            firing_threshold: 0.10,
            base_decay: 0.25,
            wormhole_decay: 0.70,
            tension_threshold: 1.0,
            max_neighbors_per_node: 20,
            return_flow_factor: 0.15,
            fir_gamma: 0.6,
            max_propagation_states: 2000,
            minimum_injected_current: 0.01,
            // 0 表示完整输出；旧 JS SOTA 不在河网观测层截断，
            // 只在后续融合层限制 emergent candidates。
            max_output_nodes: 0,
            max_output_edges: 0,
            max_transition_records: 0,
            dynamic_wormholes: true,
            dynamic_wormhole_threshold: 0.2,
            dynamic_wormhole_slots: 2,
            dynamic_wormhole_max_targets: 4096,
        }
    }
}

#[derive(Clone)]
struct SpikeState {
    node_index: usize,
    previous_index: Option<usize>,
    energy: f64,
    momentum: f64,
    source_type: String,
    hop: usize,
    seed_id: i64,
    residual_level: usize,
}

#[derive(Clone)]
struct Provenance {
    source_type: String,
    origin_type: Option<String>,
    hop: usize,
    seed_id: Option<i64>,
}

#[derive(Clone)]
struct Parent {
    parent_id: i64,
    flow: f64,
    hop: usize,
    wormhole: bool,
}

#[derive(Clone)]
struct EdgeFlow {
    source_id: i64,
    target_id: i64,
    flow: f64,
    max_flow: f64,
    conductance: f64,
    min_hop: usize,
    wormhole: bool,
    immediate_return: bool,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ObservationNode {
    pub id: i64,
    pub energy: f64,
    pub normalized_energy: f64,
    pub source_type: String,
    pub origin_type: Option<String>,
    pub hop: usize,
    pub seed_id: Option<i64>,
    pub strongest_parent: Option<ObservationParent>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ObservationParent {
    pub parent_id: i64,
    pub flow: f64,
    pub hop: usize,
    pub wormhole: bool,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ObservationEdge {
    pub source_id: i64,
    pub target_id: i64,
    pub flow: f64,
    pub max_flow: f64,
    pub normalized_flow: f64,
    pub conductance: f64,
    pub min_hop: usize,
    pub wormhole: bool,
    pub immediate_return: bool,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SenseDiagnostics {
    pub backend: String,
    pub seed_nodes: usize,
    pub reached_nodes: usize,
    pub active_edges: usize,
    pub maximum_node_energy: f64,
    pub maximum_edge_flow: f64,
    pub return_flow_suppressed_mass: f64,
    pub state_truncations: usize,
    pub hop_in_flight_mass: Vec<f64>,
    pub artifact_nodes: usize,
    pub artifact_edges: usize,
    pub elapsed_ms: f64,
    /// Counts accepted-current events before destination state-cap pruning.
    pub wormhole_edges_fired: usize,
    pub dynamic_wormhole_edges_fired: usize,
    pub promoted_wormhole_transitions: usize,
    pub dynamic_wormhole_candidates: usize,
    pub dynamic_wormhole_selected: usize,
    pub dynamic_wormhole_work: DynamicWormholeDiagnostics,
}

/// A time-expanded transition between accepted propagation states.
/// State identity is (hop, previous_id, node_id), not just a Tag ID.
/// Multiple seed paths may merge into one state; no unique seed lineage is claimed.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ObservationTransition {
    pub source_id: i64,
    pub target_id: i64,
    pub previous_id: Option<i64>,
    pub hop: usize,
    pub flow: f64,
    pub wormhole: bool,
    pub immediate_return: bool,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SenseOutput {
    pub schema: String,
    pub algorithm_version: String,
    pub artifact_sig: String,
    pub query_id: Option<String>,
    pub source_field: Vec<(i64, f64)>,
    pub nodes: Vec<ObservationNode>,
    pub edges: Vec<ObservationEdge>,
    pub diagnostics: SenseDiagnostics,
    pub transitions: Vec<ObservationTransition>,
    pub transitions_truncated: bool,
}

fn fir_weights(config: &SenseConfig) -> Vec<f64> {
    let gamma = config.fir_gamma.clamp(0.05, 0.95);
    let mut weights: Vec<f64> = (0..=config.max_safe_hops)
        .map(|hop| gamma.powi(hop as i32))
        .collect();
    let total: f64 = weights.iter().sum();
    if total > 0.0 {
        for weight in &mut weights {
            *weight /= total;
        }
    }
    weights
}

mod wormholes;
pub use wormholes::{DynamicWormholeContext, DynamicWormholeDiagnostics};
mod propagation;
pub(crate) use propagation::sense_prevalidated;
pub use propagation::{sense_typed, sense_with_context};
