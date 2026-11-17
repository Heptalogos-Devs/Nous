//! Topology readout score assignment.
use super::*;

pub(super) fn assign_v3_scores(
    work: &mut [ScoredWork],
    mode: &str,
    omega: &OmegaOutput,
    config: &NativeConfig,
) {
    for item in work.iter_mut() {
        let direct_answer = mode != "atomic"
            && item.observables.closure >= 0.55
            && item.anchor.explicit_exact_contacts > 0;
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

    for item in work.iter_mut() {
        // Candidate-self evidence, not exceptionalness among its peers. Roles
        // explain evidence and never allocate ranking privilege.
        let confidence = branch_confidence(&item.topology, mode);
        let direction = if item.topology.matched_edges > 0 {
            0.25 + 0.75 * item.topology.direction_score
        } else {
            0.25
        };
        let requested = positive(config.topology_bonus_cap)
            * item.graph_score
            * confidence
            * direction
            * precision_grounding(&item.topology);
        item.v2_bonus = clamp01(requested.min(positive(config.topology_bonus_cap)));
    }

    // Absolute activation: unrelated candidates cannot change eligibility.
    let threshold = clamp01(config.anchor_activation_floor);
    let graph_gate = omega.omega.powf(config.omega_gamma.max(0.0));

    for item in work.iter_mut() {
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
        if omega.omega < config.struct_role_min_omega && item.role == "structural_explanation" {
            item.role = "thematic_neighbor".to_string();
        }
        item.gated_bonus =
            (item.v2_bonus * graph_gate).min(positive(config.topology_bonus_cap - item.path_bonus));
        item.final_score =
            clamp01(item.pure_score + item.path_bonus + item.gated_bonus + item.anchor_bonus);
        item.ranking_score = item.final_score;
    }
}
