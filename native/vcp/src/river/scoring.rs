//! Topology readout score assignment.
use super::*;

pub(super) fn assign_v3_scores(
    work: &mut [ScoredWork],
    mode: &str,
    omega: &OmegaOutput,
    config: &NativeConfig,
    cancelled: crate::CancellationCheck<'_>,
) -> Result<(), String> {
    let maximum_pure = work.iter().map(|item| item.pure_score).fold(0.0, f64::max);
    for item in work.iter_mut() {
        let near_frontier = item.pure_score >= maximum_pure - 0.03;
        let direct_answer = mode != "atomic"
            && item.observables.closure >= 0.55
            && (item.direct_evidence >= 0.55 || (near_frontier && item.curve.query_score >= 0.55));
        let structural = (item.topology.matched_edge_coverage
            * item.topology.reliability
            * item.observables.closure)
            .cbrt();
        item.role = if mode == "atomic" {
            "atomic_concept"
        } else if direct_answer {
            "direct_answer"
        } else if structural >= 0.35 {
            "structural_explanation"
        } else {
            "thematic_neighbor"
        }
        .to_string();
    }

    let direct_frontier = work
        .iter()
        .filter(|item| item.role == "direct_answer")
        .map(|item| item.pure_score)
        .fold(0.0, f64::max);

    for index in 0..work.len() {
        crate::check_cancelled(cancelled)?;
        let mut peers: Vec<(usize, f64)> = (0..work.len())
            .filter(|peer| *peer != index)
            .map(|peer| {
                let pure_delta = (work[peer].pure_score - work[index].pure_score)
                    / config.conditional_bandwidth.max(1e-4);
                let closure_delta = (work[peer].observables.closure
                    - work[index].observables.closure)
                    / config.conditional_closure_bandwidth.max(1e-4);
                let direct_delta = (work[peer].direct_evidence - work[index].direct_evidence)
                    / config.conditional_direct_bandwidth.max(1e-4);
                let role_weight = if work[peer].role == work[index].role {
                    1.0
                } else {
                    0.35
                };
                (
                    peer,
                    role_weight
                        * (-0.5
                            * (pure_delta * pure_delta
                                + closure_delta * closure_delta
                                + direct_delta * direct_delta))
                            .exp(),
                )
            })
            .collect();
        peers.sort_by(|left, right| right.1.partial_cmp(&left.1).unwrap_or(Ordering::Equal));
        peers.retain(|entry| entry.1 >= 1e-4);
        if peers.len() < config.minimum_peers {
            let existing: HashSet<usize> = peers.iter().map(|entry| entry.0).collect();
            let mut fallback: Vec<(usize, f64)> = (0..work.len())
                .filter(|peer| *peer != index && !existing.contains(peer))
                .map(|peer| (peer, 1e-4))
                .take(config.minimum_peers.saturating_sub(peers.len()))
                .collect();
            peers.append(&mut fallback);
        }
        let total_weight: f64 = peers.iter().map(|entry| entry.1).sum();
        let squared_weight: f64 = peers.iter().map(|entry| entry.1 * entry.1).sum();
        let expected = if total_weight > 1e-12 {
            peers
                .iter()
                .map(|entry| entry.1 * work[entry.0].graph_score)
                .sum::<f64>()
                / total_weight
        } else {
            work[index].graph_score
        };
        let variance = if total_weight > 1e-12 {
            peers
                .iter()
                .map(|entry| entry.1 * (work[entry.0].graph_score - expected).powi(2))
                .sum::<f64>()
                / total_weight
        } else {
            0.0
        };
        let effective_peers = if squared_weight > 1e-12 {
            total_weight * total_weight / squared_weight
        } else {
            0.0
        };
        let uncertainty = (variance * (1.0 + 1.0 / effective_peers.max(1.0))).sqrt();
        let innovation = positive(
            work[index].graph_score - expected - config.innovation_confidence_z * uncertainty,
        );
        let candidate_confidence = if mode == "atomic" {
            (work[index].observables.closure
                * clamp01(
                    0.55 * work[index].topology.matched_node_coverage
                        + 0.45 * work[index].topology.node_alignment_score,
                ))
            .sqrt()
        } else {
            (work[index].observables.closure
                * clamp01(
                    0.75 * work[index].topology.matched_edge_coverage
                        + 0.25 * work[index].topology.matched_node_coverage,
                )
                * clamp01(
                    0.7 * work[index].topology.reliability
                        + 0.3 * work[index].topology.node_alignment_score,
                ))
            .cbrt()
        };
        let statistical = clamp01(effective_peers / config.minimum_effective_peers);
        let combined_confidence = clamp01(candidate_confidence * statistical);
        let (role_cap, multiplier) = match work[index].role.as_str() {
            "atomic_concept" => (config.topology_bonus_cap, 1.0),
            "direct_answer" => (0.02, 0.35),
            "structural_explanation" => (0.045, 0.7),
            _ => (0.008, 0.15),
        };
        let requested = innovation * combined_confidence * config.innovation_scale * multiplier;
        let mut bonus = requested.min(role_cap).min(config.topology_bonus_cap);
        if direct_frontier > 0.0 && work[index].role != "direct_answer" && mode != "atomic" {
            bonus = bonus.min(positive(direct_frontier - 0.005 - work[index].pure_score));
        }
        work[index].v2_bonus = clamp01(bonus);
    }

    let anchor_mean = if work.is_empty() {
        0.0
    } else {
        work.iter().map(|item| item.anchor.strength).sum::<f64>() / work.len() as f64
    };
    let anchor_variance = if work.is_empty() {
        0.0
    } else {
        work.iter()
            .map(|item| (item.anchor.strength - anchor_mean).powi(2))
            .sum::<f64>()
            / work.len() as f64
    };
    let anchor_std = anchor_variance.sqrt();
    let threshold = clamp01(
        config
            .anchor_activation_floor
            .max(anchor_mean + config.anchor_activation_z * anchor_std),
    );
    let mut ranked_anchors: Vec<(usize, f64)> = work
        .iter()
        .enumerate()
        .map(|(index, item)| (index, item.anchor.strength))
        .collect();
    ranked_anchors.sort_by(|left, right| right.1.partial_cmp(&left.1).unwrap_or(Ordering::Equal));
    let strongest = ranked_anchors.first().copied();
    let second = ranked_anchors.get(1).map(|item| item.1).unwrap_or(0.0);
    let promote = strongest
        .map(|item| {
            item.1 >= config.anchor_frontier_abs_floor
                && item.1 >= config.anchor_frontier_contrast * second
        })
        .unwrap_or(false);
    let graph_gate = omega.omega.powf(config.omega_gamma.max(0.0));

    for (index, item) in work.iter_mut().enumerate() {
        let activation = if item.anchor.strength <= threshold {
            0.0
        } else if config.anchor_saturation - threshold > 1e-12 {
            let normalized = clamp01(
                (item.anchor.strength - threshold) / (config.anchor_saturation - threshold),
            );
            normalized * normalized * (3.0 - 2.0 * normalized)
        } else {
            1.0
        };
        item.anchor_bonus = config.anchor_bonus_cap * activation;
        if promote && strongest.map(|entry| entry.0) == Some(index) {
            item.role = "direct_answer".to_string();
        } else if omega.omega < config.struct_role_min_omega
            && item.role == "structural_explanation"
        {
            item.role = "thematic_neighbor".to_string();
        }
        item.gated_bonus = item.v2_bonus * graph_gate;
        item.final_score = clamp01(item.pure_score + item.gated_bonus + item.anchor_bonus);
    }
    Ok(())
}
