//! Spike propagation, source transitions and observation output.
use super::*;

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
