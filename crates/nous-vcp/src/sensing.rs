//! Adapted from VCPToolBox rust-vexus-lite/src/memo_sensing.rs
//! Upstream commit d1b40c863273cbbba082fb7657671424768c4a53.
//! CC BY-NC-SA 4.0; see LICENSE-VCP and UPSTREAM.md.
//! Host/database access is replaced by immutable typed inputs.
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

#[derive(Clone, Serialize, Deserialize)]
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

#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DynamicWormholeDiagnostics {
    pub targets_evaluated: usize,
    pub source_vectors_loaded: usize,
    pub source_batches: usize,
    pub dot_products: usize,
    pub cache_hits: usize,
    pub budget_exhausted: bool,
    pub load_ms: f64,
    pub score_ms: f64,
    pub selection_ms: f64,
}

/// Request-local only: never mutates graph assets or persists residuals.
/// A source at layer L is supported only by residuals from layers > L.
pub struct DynamicWormholeContext<'a> {
    connection: &'a TagVectorPool,
    pool: &'a mut TagVectorPool,
    residuals: Vec<Vec<f32>>,
    residual_norms: Vec<f64>,
    known_scores: HashMap<(usize, i64), f64>,
    seed_levels: HashMap<i64, usize>,
    scores: HashMap<usize, Vec<f64>>,
    pub diagnostics: DynamicWormholeDiagnostics,
}

impl<'a> DynamicWormholeContext<'a> {
    pub fn try_new(
        connection: &'a TagVectorPool,
        pool: &'a mut TagVectorPool,
        residuals: Vec<Vec<f32>>,
        seed_levels: HashMap<i64, usize>,
    ) -> Result<Self, String> {
        if residuals.first().is_some_and(|first| {
            first.is_empty()
                || residuals.iter().any(|row| {
                    row.len() != first.len() || row.iter().any(|value| !value.is_finite())
                })
        }) || seed_levels.keys().any(|id| *id <= 0)
        {
            return Err("MEMO_RESIDUAL_CONTEXT_INVALID: residual dimensions, values or seed IDs are invalid".into());
        }
        let residual_norms = residuals
            .iter()
            .map(|r| r.iter().map(|&v| (v as f64).powi(2)).sum::<f64>().sqrt())
            .collect();
        Ok(Self {
            connection,
            pool,
            residuals,
            residual_norms,
            seed_levels,
            known_scores: HashMap::new(),
            scores: HashMap::new(),
            diagnostics: DynamicWormholeDiagnostics::default(),
        })
    }

    pub fn reuse_score(&mut self, layer: usize, id: i64, score: f64) {
        if score.is_finite() {
            self.known_scores
                .insert((layer, id), score.clamp(-1.0, 1.0));
        }
    }

    fn prepare(
        &mut self,
        artifact: &NativeArtifact,
        active: &HashMap<(Option<usize>, usize), SpikeState>,
        config: &SenseConfig,
    ) -> std::result::Result<(), String> {
        if self.residuals.len() < 2 {
            return Ok(());
        }
        let mut targets = Vec::new();
        for spike in active.values().filter(|s| {
            s.energy >= config.firing_threshold
                && s.momentum >= 0.0
                && s.residual_level + 1 < self.residuals.len()
        }) {
            for cursor in
                artifact.row_offsets[spike.node_index]..artifact.row_offsets[spike.node_index + 1]
            {
                if positive(artifact.weights[cursor]) <= 0.0 {
                    break;
                }
                let target = artifact.targets[cursor];
                if !self.scores.contains_key(&target) {
                    targets.push(target);
                }
            }
        }
        // Deterministic request budget, independent of HashMap iteration order.
        targets.sort_unstable();
        targets.dedup();
        let remaining = config
            .dynamic_wormhole_max_targets
            .saturating_sub(self.scores.len());
        if targets.len() > remaining {
            self.diagnostics.budget_exhausted = true;
        }
        targets.truncate(remaining);
        if targets.is_empty() {
            return Ok(());
        }
        let dimension = self.residuals[0].len();
        let ids: Vec<i64> = targets.iter().map(|&i| artifact.node_ids[i]).collect();
        let began = Instant::now();
        let stats = ensure_tag_vectors_loaded(self.connection, self.pool, &ids, dimension)?;
        self.diagnostics.load_ms += began.elapsed().as_secs_f64() * 1000.0;
        self.diagnostics.source_vectors_loaded += stats.fetched_nodes;
        self.diagnostics.source_batches += stats.source_batches;
        let began = Instant::now();
        for target in targets {
            // Cache negative/missing results too: never retry a dirty SQL row per hop.
            let mut scores = vec![0.0; self.residuals.len()];
            if let Some(vector) = self.pool.get(&artifact.node_ids[target]) {
                let norm = vector
                    .iter()
                    .map(|&v| (v as f64).powi(2))
                    .sum::<f64>()
                    .sqrt();
                if vector.len() == dimension && norm.is_finite() && norm > 1e-12 {
                    for (layer, residual) in self.residuals.iter().enumerate().skip(1) {
                        let residual_norm = self.residual_norms[layer];
                        if residual.len() != dimension {
                            continue;
                        }
                        if residual_norm.is_finite() && residual_norm > 1e-12 {
                            if let Some(&score) =
                                self.known_scores.get(&(layer, artifact.node_ids[target]))
                            {
                                scores[layer] = positive(score);
                                self.diagnostics.cache_hits += 1;
                            } else {
                                let dot = residual
                                    .iter()
                                    .zip(vector.iter())
                                    .map(|(&a, &b)| a as f64 * b as f64)
                                    .sum::<f64>();
                                scores[layer] = positive(dot / (norm * residual_norm)).min(1.0);
                                self.diagnostics.dot_products += 1;
                            }
                        }
                    }
                }
            }
            self.scores.insert(target, scores);
        }
        self.diagnostics.targets_evaluated = self.scores.len();
        self.diagnostics.score_ms += began.elapsed().as_secs_f64() * 1000.0;
        Ok(())
    }

    fn resonance(&mut self, target: usize, source_level: usize) -> f64 {
        let Some(scores) = self.scores.get(&target) else {
            return 0.0;
        };
        self.diagnostics.cache_hits += 1;
        scores
            .iter()
            .skip(source_level.saturating_add(1))
            .copied()
            .fold(0.0, f64::max)
    }
}

pub fn sense_typed(
    artifact: &NativeArtifact,
    artifact_sig: &str,
    input: SenseInput,
) -> std::result::Result<SenseOutput, String> {
    sense_with_context(artifact, artifact_sig, input, None)
}

pub fn sense_with_context(
    artifact: &NativeArtifact,
    artifact_sig: &str,
    input: SenseInput,
    context: Option<&mut DynamicWormholeContext<'_>>,
) -> std::result::Result<SenseOutput, String> {
    artifact.validate()?;
    sense_prevalidated(artifact, artifact_sig, input, context)
}

/// MemoAssets has already validated this immutable graph once at construction.
pub(crate) fn sense_prevalidated(
    artifact: &NativeArtifact,
    artifact_sig: &str,
    input: SenseInput,
    mut context: Option<&mut DynamicWormholeContext<'_>>,
) -> std::result::Result<SenseOutput, String> {
    crate::assets::validate_finite_config(&input.config)?;
    if artifact_sig.trim().is_empty()
        || input
            .seeds
            .iter()
            .any(|seed| seed.id <= 0 || !seed.energy.is_finite())
    {
        return Err(
            "MEMO_SENSE_INPUT_INVALID: signature, seed IDs and energies are required".into(),
        );
    }
    let started_at = Instant::now();
    let config = input.config;
    if !config.dynamic_wormholes
        || config.dynamic_wormhole_slots == 0
        || config.dynamic_wormhole_max_targets == 0
    {
        context = None;
    }
    if !config.dynamic_wormhole_threshold.is_finite()
        || !(0.0..=1.0).contains(&config.dynamic_wormhole_threshold)
    {
        return Err("dynamicWormholeThreshold must be finite and in [0,1]".into());
    }
    let weights = fir_weights(&config);
    let mut active: HashMap<(Option<usize>, usize), SpikeState> = HashMap::new();
    let mut accumulated: HashMap<usize, f64> = HashMap::new();
    let mut provenance: HashMap<usize, Provenance> = HashMap::new();
    let mut edge_flows: HashMap<(usize, usize), EdgeFlow> = HashMap::new();
    let mut parents: HashMap<usize, Parent> = HashMap::new();

    for seed in input
        .seeds
        .iter()
        .filter(|seed| seed.id > 0 && seed.energy > 0.0)
    {
        let Some(&node_index) = artifact.node_index.get(&seed.id) else {
            continue;
        };
        let energy = positive(seed.energy);
        let source_type = if seed.source_type.is_empty() {
            "seed".to_string()
        } else {
            seed.source_type.clone()
        };
        active
            .entry((None, node_index))
            .and_modify(|state| state.energy += energy)
            .or_insert(SpikeState {
                node_index,
                previous_index: None,
                energy,
                momentum: config.base_momentum,
                source_type: source_type.clone(),
                hop: 0,
                seed_id: seed.id,
                residual_level: context
                    .as_ref()
                    .and_then(|c| c.seed_levels.get(&seed.id).copied())
                    .unwrap_or(usize::MAX / 2),
            });
        *accumulated.entry(node_index).or_default() += energy * weights[0];
        provenance.entry(node_index).or_insert(Provenance {
            source_type,
            origin_type: None,
            hop: 0,
            seed_id: Some(seed.id),
        });
    }

    // 空种子是合法的请求级观测：JavaScript SOTA 在 Pyramid 未召回 Tag 时
    // 跳过 Spike，但仍允许传播后补入字符串 Core/幽灵节点参与最终融合；
    // 若后续也没有可融合节点，则返回原查询向量而不是把请求升级为异常。
    let mut return_flow_suppressed_mass = 0.0;
    let mut state_truncations = 0usize;
    let mut hop_in_flight_mass = Vec::new();
    let transition_limit = config.max_transition_records.min(16_000);
    let mut transitions = Vec::new();
    let mut transitions_truncated = false;

    let mut wormhole_edges_fired = 0;
    let mut dynamic_wormhole_edges_fired = 0;
    let mut promoted_wormhole_transitions = 0;
    let mut dynamic_wormhole_candidates = 0;
    let mut dynamic_wormhole_selected = 0;
    let mut neighbor_scratch: Vec<(usize, f64, bool)> = Vec::new();
    let mut selected_scratch: Vec<(usize, bool)> = Vec::new();

    for hop in 0..config.max_safe_hops {
        if let Some(ctx) = context.as_deref_mut() {
            ctx.prepare(artifact, &active, &config)?;
        }
        let mut next: HashMap<(Option<usize>, usize), SpikeState> = HashMap::new();
        let mut hop_transitions = Vec::new();

        for spike in active.values() {
            if spike.energy < config.firing_threshold || spike.momentum < 0.0 {
                continue;
            }
            let start = artifact.row_offsets[spike.node_index];
            let end = artifact.row_offsets[spike.node_index + 1];
            let limit = end.min(start.saturating_add(config.max_neighbors_per_node.max(1)));
            selected_scratch.clear();
            neighbor_scratch.clear();
            if let Some(ctx) = context.as_deref_mut() {
                let began = Instant::now();
                for cursor in start..end {
                    let conductance = positive(artifact.weights[cursor]);
                    if conductance <= 0.0 {
                        break;
                    }
                    let target = artifact.targets[cursor];
                    if target == spike.node_index {
                        continue;
                    }
                    let resonance = ctx.resonance(target, spike.residual_level);
                    if resonance <= 0.0 || resonance < config.dynamic_wormhole_threshold {
                        continue;
                    }
                    dynamic_wormhole_candidates += 1;
                    // Do not spend a slot on an edge rejected by return-current gating.
                    let return_factor = if spike.previous_index == Some(target) {
                        config.return_flow_factor.clamp(0.0, 1.0)
                    } else {
                        1.0
                    };
                    if spike.energy * conductance * config.wormhole_decay * return_factor
                        < config.minimum_injected_current
                    {
                        continue;
                    }
                    neighbor_scratch.push((cursor, conductance * resonance.powi(4), true));
                }
                neighbor_scratch.sort_unstable_by(|a, b| {
                    b.1.total_cmp(&a.1)
                        .then_with(|| artifact.weights[b.0].total_cmp(&artifact.weights[a.0]))
                        .then_with(|| {
                            artifact.node_ids[artifact.targets[a.0]]
                                .cmp(&artifact.node_ids[artifact.targets[b.0]])
                        })
                });
                for &(cursor, _, _) in neighbor_scratch.iter().take(
                    config
                        .dynamic_wormhole_slots
                        .min(config.max_neighbors_per_node.max(1)),
                ) {
                    selected_scratch.push((cursor, true));
                }
                // Reserve within K, fill remaining seats with unchanged ranked history.
                for cursor in start..limit {
                    if selected_scratch.len() >= config.max_neighbors_per_node.max(1) {
                        break;
                    }
                    if !selected_scratch.iter().any(|&(c, _)| c == cursor) {
                        selected_scratch.push((cursor, false));
                    }
                }
                ctx.diagnostics.selection_ms += began.elapsed().as_secs_f64() * 1000.0;
            } else {
                selected_scratch.extend((start..limit).map(|c| (c, false)));
            }
            for &(cursor, dynamic_wormhole) in &selected_scratch {
                dynamic_wormhole_selected += usize::from(dynamic_wormhole);
                let target_index = artifact.targets[cursor];
                let conductance = positive(artifact.weights[cursor]);
                if conductance <= 0.0 {
                    break;
                }
                let source_id = artifact.node_ids[spike.node_index];
                let target_id = artifact.node_ids[target_index];
                let residual = artifact.node_anchor_gain[target_index];
                let wormhole = dynamic_wormhole
                    || artifact.edge_is_wormhole[cursor]
                    || conductance * residual >= config.tension_threshold;
                let decay = if wormhole {
                    config.wormhole_decay
                } else {
                    config.base_decay
                };
                let immediate_return = spike.previous_index == Some(target_index);
                let unpenalized = spike.energy * conductance * decay;
                let injected = unpenalized
                    * if immediate_return {
                        config.return_flow_factor.clamp(0.0, 1.0)
                    } else {
                        1.0
                    };
                if immediate_return {
                    return_flow_suppressed_mass += positive(unpenalized - injected);
                }
                if injected < config.minimum_injected_current {
                    continue;
                }

                wormhole_edges_fired += usize::from(wormhole);
                dynamic_wormhole_edges_fired += usize::from(dynamic_wormhole);
                promoted_wormhole_transitions += usize::from(dynamic_wormhole && cursor >= limit);
                let edge = edge_flows
                    .entry((spike.node_index, target_index))
                    .or_insert(EdgeFlow {
                        source_id,
                        target_id,
                        flow: 0.0,
                        max_flow: 0.0,
                        conductance,
                        min_hop: spike.hop + 1,
                        wormhole,
                        immediate_return,
                    });
                edge.flow += injected;
                edge.max_flow = edge.max_flow.max(injected);
                edge.min_hop = edge.min_hop.min(spike.hop + 1);
                edge.wormhole |= wormhole;
                edge.immediate_return |= immediate_return;

                let parent = parents.entry(target_index).or_insert(Parent {
                    parent_id: source_id,
                    flow: injected,
                    hop: spike.hop + 1,
                    wormhole,
                });
                if injected > parent.flow || (injected == parent.flow && spike.hop + 1 < parent.hop)
                {
                    *parent = Parent {
                        parent_id: source_id,
                        flow: injected,
                        hop: spike.hop + 1,
                        wormhole,
                    };
                }

                let next_momentum = spike.momentum - if wormhole { 0.0 } else { 1.0 };
                if next_momentum < 0.0 && !wormhole {
                    continue;
                }
                if transition_limit > 0 {
                    // Bound temporary storage as well as the retained observation.
                    if hop_transitions.len() < transition_limit {
                        hop_transitions.push(ObservationTransition {
                            source_id,
                            target_id,
                            previous_id: spike.previous_index.map(|i| artifact.node_ids[i]),
                            hop: spike.hop + 1,
                            flow: injected,
                            wormhole,
                            immediate_return,
                        });
                    } else {
                        transitions_truncated = true;
                    }
                }
                next.entry((Some(spike.node_index), target_index))
                    .and_modify(|state| {
                        state.energy += injected;
                        state.momentum = state.momentum.max(next_momentum);
                        state.residual_level = state.residual_level.min(spike.residual_level);
                    })
                    .or_insert(SpikeState {
                        node_index: target_index,
                        previous_index: Some(spike.node_index),
                        energy: injected,
                        momentum: next_momentum,
                        source_type: spike.source_type.clone(),
                        hop: spike.hop + 1,
                        seed_id: spike.seed_id,
                        residual_level: spike.residual_level,
                    });
            }
        }

        if next.len() > config.max_propagation_states.max(100) {
            let mut retained: Vec<SpikeState> = next.into_values().collect();
            retained.sort_by(|left, right| {
                right
                    .energy
                    .partial_cmp(&left.energy)
                    .unwrap_or(Ordering::Equal)
            });
            let limit = config.max_propagation_states.max(100);
            state_truncations += retained.len().saturating_sub(limit);
            retained.truncate(limit);
            next = retained
                .into_iter()
                .map(|state| ((state.previous_index, state.node_index), state))
                .collect();
        }

        // Reject transfers whose destination state was discarded by the state cap.
        hop_transitions.retain(|edge| {
            let source = artifact.node_index.get(&edge.source_id).copied();
            let target = artifact.node_index.get(&edge.target_id).copied();
            match (source, target) {
                (Some(source), Some(target)) => next.contains_key(&(Some(source), target)),
                _ => false,
            }
        });
        hop_transitions.sort_by(|left, right| {
            right
                .flow
                .total_cmp(&left.flow)
                .then_with(|| left.source_id.cmp(&right.source_id))
                .then_with(|| left.target_id.cmp(&right.target_id))
                .then_with(|| left.previous_id.cmp(&right.previous_id))
        });
        let remaining = transition_limit.saturating_sub(transitions.len());
        if hop_transitions.len() > remaining {
            transitions_truncated = true;
            hop_transitions.truncate(remaining);
        }
        transitions.extend(hop_transitions);

        let mut node_energy: HashMap<usize, f64> = HashMap::new();
        let mut in_flight = 0.0;
        for spike in next.values() {
            *node_energy.entry(spike.node_index).or_default() += spike.energy;
            in_flight += spike.energy;
            provenance
                .entry(spike.node_index)
                .and_modify(|value| {
                    if spike.hop < value.hop {
                        *value = Provenance {
                            source_type: "emergent".to_string(),
                            origin_type: Some(spike.source_type.clone()),
                            hop: spike.hop,
                            seed_id: Some(spike.seed_id),
                        };
                    }
                })
                .or_insert(Provenance {
                    source_type: "emergent".to_string(),
                    origin_type: Some(spike.source_type.clone()),
                    hop: spike.hop,
                    seed_id: Some(spike.seed_id),
                });
        }
        hop_in_flight_mass.push(in_flight);

        let field_weight = weights[hop + 1];
        let mut propagated = false;
        for (node_index, energy) in node_energy {
            *accumulated.entry(node_index).or_default() += energy * field_weight;
            propagated |= energy > config.minimum_injected_current;
        }
        if !propagated {
            break;
        }
        active = next;
    }

    let maximum_node_energy = accumulated.values().copied().fold(0.0, f64::max);
    let maximum_edge_flow = edge_flows
        .values()
        .map(|edge| edge.flow)
        .fold(0.0, f64::max);

    let mut nodes: Vec<ObservationNode> = accumulated
        .iter()
        .map(|(&index, &energy)| {
            let meta = provenance.get(&index);
            ObservationNode {
                id: artifact.node_ids[index],
                energy,
                normalized_energy: if maximum_node_energy > 0.0 {
                    energy / maximum_node_energy
                } else {
                    0.0
                },
                source_type: meta
                    .map(|value| value.source_type.clone())
                    .unwrap_or_else(|| "unknown".to_string()),
                origin_type: meta.and_then(|value| value.origin_type.clone()),
                hop: meta.map(|value| value.hop).unwrap_or(0),
                seed_id: meta.and_then(|value| value.seed_id),
                strongest_parent: parents.get(&index).map(|parent| ObservationParent {
                    parent_id: parent.parent_id,
                    flow: parent.flow,
                    hop: parent.hop,
                    wormhole: parent.wormhole,
                }),
            }
        })
        .collect();
    nodes.sort_by(|left, right| {
        right
            .energy
            .partial_cmp(&left.energy)
            .unwrap_or(Ordering::Equal)
            .then_with(|| left.id.cmp(&right.id))
    });
    if config.max_output_nodes > 0 {
        nodes.truncate(config.max_output_nodes);
    }

    let mut edges: Vec<ObservationEdge> = edge_flows
        .into_values()
        .map(|edge| ObservationEdge {
            source_id: edge.source_id,
            target_id: edge.target_id,
            flow: edge.flow,
            max_flow: edge.max_flow,
            normalized_flow: if maximum_edge_flow > 0.0 {
                edge.flow / maximum_edge_flow
            } else {
                0.0
            },
            conductance: edge.conductance,
            min_hop: edge.min_hop,
            wormhole: edge.wormhole,
            immediate_return: edge.immediate_return,
        })
        .collect();
    edges.sort_by(|left, right| {
        right
            .flow
            .partial_cmp(&left.flow)
            .unwrap_or(Ordering::Equal)
            .then_with(|| left.source_id.cmp(&right.source_id))
            .then_with(|| left.target_id.cmp(&right.target_id))
    });
    if config.max_output_edges > 0 {
        edges.truncate(config.max_output_edges);
    }

    let total_mass: f64 = nodes.iter().map(|node| node.energy).sum();
    let source_field = nodes
        .iter()
        .filter(|node| node.id > 0 && node.energy > 0.0)
        .map(|node| {
            (
                node.id,
                if total_mass > 0.0 {
                    node.energy / total_mass
                } else {
                    0.0
                },
            )
        })
        .collect();

    Ok(SenseOutput {
        schema: OBSERVATION_SCHEMA.to_string(),
        algorithm_version: ALGORITHM_VERSION.to_string(),
        artifact_sig: artifact_sig.to_string(),
        query_id: input.query_id,
        source_field,
        diagnostics: SenseDiagnostics {
            backend: "rust-shared-memo-runtime".to_string(),
            seed_nodes: input.seeds.len(),
            reached_nodes: nodes.len(),
            active_edges: edges.len(),
            maximum_node_energy,
            maximum_edge_flow,
            return_flow_suppressed_mass,
            state_truncations,
            hop_in_flight_mass,
            artifact_nodes: artifact.node_ids.len(),
            artifact_edges: artifact.targets.len(),
            elapsed_ms: started_at.elapsed().as_secs_f64() * 1000.0,
            wormhole_edges_fired,
            dynamic_wormhole_edges_fired,
            promoted_wormhole_transitions,
            dynamic_wormhole_candidates,
            dynamic_wormhole_selected,
            dynamic_wormhole_work: context
                .as_ref()
                .map(|ctx| ctx.diagnostics.clone())
                .unwrap_or_default(),
        },
        nodes,
        edges,
        transitions,
        transitions_truncated,
    })
}
