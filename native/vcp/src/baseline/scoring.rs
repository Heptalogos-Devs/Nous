use super::{positive, unit};
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
pub struct ReferenceScoreInput {
    pub candidates: Vec<ReferenceScoreCandidate>,
    pub mode: String,
    pub omega: f64,
    pub config: ReferenceScoreConfig,
}
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReferenceScoreCandidate {
    pub id: i64,
    pub pure_score: f64,
    pub graph_score: f64,
    pub query_score: f64,
    pub closure: f64,
    pub direct_evidence: f64,
    pub matched_edge_coverage: f64,
    pub matched_node_coverage: f64,
    pub node_alignment_score: f64,
    pub topology_reliability: f64,
    pub anchor_strength: f64,
}
#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(default, rename_all = "camelCase")]
pub struct ReferenceScoreConfig {
    pub conditional_bandwidth: f64,
    pub conditional_closure_bandwidth: f64,
    pub conditional_direct_bandwidth: f64,
    pub minimum_peers: usize,
    pub minimum_effective_peers: f64,
    pub innovation_confidence_z: f64,
    pub innovation_scale: f64,
    pub topology_bonus_cap: f64,
    pub omega_gamma: f64,
    pub struct_role_min_omega: f64,
    pub anchor_bonus_cap: f64,
    pub anchor_activation_z: f64,
    pub anchor_activation_floor: f64,
    pub anchor_saturation: f64,
    pub anchor_frontier_contrast: f64,
    pub anchor_frontier_abs_floor: f64,
}
impl Default for ReferenceScoreConfig {
    fn default() -> Self {
        Self {
            conditional_bandwidth: 0.04,
            conditional_closure_bandwidth: 0.1,
            conditional_direct_bandwidth: 0.12,
            minimum_peers: 3,
            minimum_effective_peers: 2.5,
            innovation_confidence_z: 1.0,
            innovation_scale: 0.5,
            topology_bonus_cap: 0.08,
            omega_gamma: 1.0,
            struct_role_min_omega: 0.12,
            anchor_bonus_cap: 0.1,
            anchor_activation_z: 2.0,
            anchor_activation_floor: 0.05,
            anchor_saturation: 0.2,
            anchor_frontier_contrast: 2.0,
            anchor_frontier_abs_floor: 0.1,
        }
    }
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReferenceScoreOutput {
    pub id: i64,
    pub role: String,
    pub v2_bonus: f64,
    pub gated_bonus: f64,
    pub anchor_bonus: f64,
    pub final_score: f64,
    pub diagnostics: ReferenceInnovation,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReferenceInnovation {
    pub expected: f64,
    pub variance: f64,
    pub effective_peers: f64,
    pub uncertainty: f64,
    pub innovation: f64,
    pub candidate_confidence: f64,
    pub combined_confidence: f64,
    pub requested_bonus: f64,
    pub peer_count: usize,
    pub initial_role: String,
    pub anchor_threshold: f64,
    pub anchor_activation: f64,
    pub graph_gate: f64,
}
fn role(c: &ReferenceScoreCandidate, atomic: bool, frontier: f64) -> &'static str {
    if atomic {
        "atomic_concept"
    } else if c.closure >= 0.55
        && (c.direct_evidence >= 0.55 || (c.pure_score >= frontier - 0.03 && c.query_score >= 0.55))
    {
        "direct_answer"
    } else if (c.matched_edge_coverage * c.topology_reliability * c.closure).cbrt() >= 0.35 {
        "structural_explanation"
    } else {
        "thematic_neighbor"
    }
}

fn peer_distribution(
    input: &ReferenceScoreInput,
    roles: &[&str],
    index: usize,
) -> Vec<(usize, f64)> {
    let target = &input.candidates[index];
    let cfg = &input.config;
    let mut peers = input
        .candidates
        .iter()
        .enumerate()
        .filter(|(j, _)| *j != index)
        .map(|(j, c)| {
            let delta = [
                (c.pure_score - target.pure_score) / cfg.conditional_bandwidth.max(1e-4),
                (c.closure - target.closure) / cfg.conditional_closure_bandwidth.max(1e-4),
                (c.direct_evidence - target.direct_evidence)
                    / cfg.conditional_direct_bandwidth.max(1e-4),
            ];
            let distance: f64 = delta.iter().map(|x| x * x).sum();
            let weight =
                (-0.5 * distance).exp() * if roles[j] == roles[index] { 1.0 } else { 0.35 };
            (j, weight)
        })
        .collect::<Vec<_>>();
    peers.sort_by(|a, b| b.1.total_cmp(&a.1));
    peers.retain(|(_, w)| *w >= 1e-4);
    for j in 0..input.candidates.len() {
        if peers.len() >= cfg.minimum_peers {
            break;
        }
        if j != index && !peers.iter().any(|(p, _)| *p == j) {
            peers.push((j, 1e-4));
        }
    }
    peers
}

fn innovation(input: &ReferenceScoreInput, roles: &[&str], index: usize) -> ReferenceInnovation {
    let c = &input.candidates[index];
    let cfg = &input.config;
    let peers = peer_distribution(input, roles, index);
    let mass: f64 = peers.iter().map(|(_, w)| w).sum();
    let square_mass: f64 = peers.iter().map(|(_, w)| w * w).sum();
    let expected = if mass > 1e-12 {
        peers
            .iter()
            .map(|(j, w)| w * input.candidates[*j].graph_score)
            .sum::<f64>()
            / mass
    } else {
        c.graph_score
    };
    let variance = if mass > 1e-12 {
        peers
            .iter()
            .map(|(j, w)| w * (input.candidates[*j].graph_score - expected).powi(2))
            .sum::<f64>()
            / mass
    } else {
        0.0
    };
    let effective_peers = if square_mass > 1e-12 {
        mass * mass / square_mass
    } else {
        0.0
    };
    let uncertainty = (variance * (1.0 + effective_peers.max(1.0).recip())).sqrt();
    let innovation = positive(c.graph_score - expected - cfg.innovation_confidence_z * uncertainty);
    let candidate_confidence = if input.mode == "atomic" {
        (c.closure * unit(0.55 * c.matched_node_coverage + 0.45 * c.node_alignment_score)).sqrt()
    } else {
        (c.closure
            * unit(0.75 * c.matched_edge_coverage + 0.25 * c.matched_node_coverage)
            * unit(0.7 * c.topology_reliability + 0.3 * c.node_alignment_score))
        .cbrt()
    };
    let combined_confidence =
        unit(candidate_confidence * unit(effective_peers / cfg.minimum_effective_peers));
    let multiplier = match roles[index] {
        "atomic_concept" => 1.0,
        "direct_answer" => 0.35,
        "structural_explanation" => 0.7,
        _ => 0.15,
    };
    ReferenceInnovation {
        expected,
        variance,
        effective_peers,
        uncertainty,
        innovation,
        candidate_confidence,
        combined_confidence,
        requested_bonus: innovation * combined_confidence * cfg.innovation_scale * multiplier,
        peer_count: peers.len(),
        initial_role: roles[index].into(),
        anchor_threshold: 0.0,
        anchor_activation: 0.0,
        graph_gate: 0.0,
    }
}

struct AnchorBatch {
    threshold: f64,
    promoted: Option<usize>,
}
fn anchor_batch(input: &ReferenceScoreInput) -> AnchorBatch {
    let n = input.candidates.len();
    let mean = if n == 0 {
        0.0
    } else {
        input
            .candidates
            .iter()
            .map(|c| c.anchor_strength)
            .sum::<f64>()
            / n as f64
    };
    let variance = if n == 0 {
        0.0
    } else {
        input
            .candidates
            .iter()
            .map(|c| (c.anchor_strength - mean).powi(2))
            .sum::<f64>()
            / n as f64
    };
    let cfg = &input.config;
    let threshold = unit(
        cfg.anchor_activation_floor
            .max(mean + cfg.anchor_activation_z * variance.sqrt()),
    );
    let mut anchors = input
        .candidates
        .iter()
        .enumerate()
        .map(|(i, c)| (i, c.anchor_strength))
        .collect::<Vec<_>>();
    anchors.sort_by(|a, b| b.1.total_cmp(&a.1));
    let promoted = anchors
        .first()
        .filter(|(_, strength)| {
            *strength >= cfg.anchor_frontier_abs_floor
                && *strength
                    >= cfg.anchor_frontier_contrast * anchors.get(1).map_or(0.0, |(_, s)| *s)
        })
        .map(|(i, _)| *i);
    AnchorBatch {
        threshold,
        promoted,
    }
}

pub fn reference_v3_scores(input: &ReferenceScoreInput) -> Vec<ReferenceScoreOutput> {
    let frontier = input
        .candidates
        .iter()
        .map(|c| c.pure_score)
        .fold(0.0, f64::max);
    let atomic = input.mode == "atomic";
    let roles = input
        .candidates
        .iter()
        .map(|c| role(c, atomic, frontier))
        .collect::<Vec<_>>();
    let direct_frontier = input
        .candidates
        .iter()
        .zip(&roles)
        .filter(|(_, r)| **r == "direct_answer")
        .map(|(c, _)| c.pure_score)
        .fold(0.0, f64::max);
    let anchor = anchor_batch(input);
    let cfg = &input.config;
    let gate = input.omega.powf(cfg.omega_gamma.max(0.0));
    input
        .candidates
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let mut diagnostics = innovation(input, &roles, i);
            let cap = match roles[i] {
                "direct_answer" => 0.02,
                "structural_explanation" => 0.045,
                "thematic_neighbor" => 0.008,
                _ => cfg.topology_bonus_cap,
            };
            let mut bonus = diagnostics
                .requested_bonus
                .min(cap)
                .min(cfg.topology_bonus_cap);
            if direct_frontier > 0.0 && roles[i] != "direct_answer" && !atomic {
                bonus = bonus.min(positive(direct_frontier - 0.005 - c.pure_score));
            }
            let v2_bonus = unit(bonus);
            let activation = if c.anchor_strength <= anchor.threshold {
                0.0
            } else if cfg.anchor_saturation - anchor.threshold <= 1e-12 {
                1.0
            } else {
                let x = unit(
                    (c.anchor_strength - anchor.threshold)
                        / (cfg.anchor_saturation - anchor.threshold),
                );
                x * x * (3.0 - 2.0 * x)
            };
            let role = if anchor.promoted == Some(i) {
                "direct_answer"
            } else if input.omega < cfg.struct_role_min_omega
                && roles[i] == "structural_explanation"
            {
                "thematic_neighbor"
            } else {
                roles[i]
            };
            diagnostics.anchor_threshold = anchor.threshold;
            diagnostics.anchor_activation = activation;
            diagnostics.graph_gate = gate;
            let gated_bonus = v2_bonus * gate;
            let anchor_bonus = cfg.anchor_bonus_cap * activation;
            ReferenceScoreOutput {
                id: c.id,
                role: role.into(),
                v2_bonus,
                gated_bonus,
                anchor_bonus,
                final_score: unit(c.pure_score + gated_bonus + anchor_bonus),
                diagnostics,
            }
        })
        .collect()
}
