use super::anchors::reference_cosine;
use super::*;
use crate::Result;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Deserialize)]
pub struct ReferenceReadoutObservation {
    pub original_vector: Vec<f32>,
    pub enhanced_vector: Vec<f32>,
    pub local_vector: Vec<f32>,
    pub transfer_vector: Vec<f32>,
    pub source_field: Vec<(i64, f64)>,
    pub local_field: Vec<(i64, f64)>,
    pub transfer_field: Vec<(i64, f64)>,
    pub local_domain: Vec<i64>,
    pub transfer_domain: Vec<i64>,
    pub river: ReferenceRiverShapeInput,
    pub provenance: Vec<ReferenceSourceProvenance>,
}
#[derive(Debug, Deserialize)]
pub struct ReferenceSourceProvenance {
    pub id: i64,
    pub hop: i64,
    pub source_type: String,
}
#[derive(Debug, Deserialize)]
pub struct ReferenceCandidateCurve {
    pub curve: ReferenceCurve,
    pub file_id: i64,
    pub bm25_score: f64,
    pub time_score: f64,
    pub anchor_score: f64,
}
#[derive(Debug, Default, Deserialize, Clone, Serialize, schemars::JsonSchema)]
#[serde(default)]
pub struct ReferenceReadoutConfig {
    pub pool: ReferencePoolConfig,
    pub path: ReferencePathConfig,
    pub topology: ReferenceTopologyConfig,
    pub anchor: ReferenceAnchorConfig,
    pub omega: ReferenceOmegaConfig,
    pub pure: ReferencePureConfig,
    pub scoring: ReferenceScoreConfig,
}
#[derive(Debug, Deserialize)]
pub struct ReferenceReadoutInput {
    pub observation: ReferenceReadoutObservation,
    pub candidates: Vec<ReferenceCandidateCurve>,
    pub transport: ReferenceTransport,
    pub edge_provenance: Vec<ReferenceProvenanceEdge>,
    /// Optional owner-supplied per-candidate evidence view. Frozen file-owned
    /// provenance remains the default numerical contract.
    #[serde(default)]
    pub candidate_provenance: Vec<(i64, Vec<ReferenceProvenanceEdge>)>,
    pub tag_vectors: Vec<(i64, Vec<f32>)>,
    pub inbound: Vec<(i64, f64)>,
    pub allowed_file_ids: Vec<i64>,
    pub config: ReferenceReadoutConfig,
    pub top_k: usize,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReferenceReadoutOutput {
    pub morphology: ReferenceMorphology,
    pub omega: ReferenceOmega,
    pub selected: Vec<ReferenceSelectedCandidate>,
    pub results: Vec<ReferenceReadoutResult>,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReferenceReadoutResult {
    pub id: i64,
    pub rank: usize,
    pub score: f64,
    pub base_score: f64,
    pub geometry: ReferencePathGeometry,
    pub relative_topology: ReferenceRelativeTopology,
    pub observables: ReferenceObservables,
    pub pure: ReferencePureScores,
    pub anchor: ReferenceAnchor,
    pub scoring: ReferenceScoreOutput,
}
fn signals(input: &ReferenceReadoutInput) -> Vec<ReferencePoolSignals> {
    let o = &input.observation;
    let domain = o.local_domain.iter().copied().collect::<BTreeSet<_>>();
    let hits = input
        .candidates
        .iter()
        .map(|c| {
            c.curve
                .tags
                .iter()
                .filter(|t| domain.contains(&t.id))
                .count()
        })
        .collect::<Vec<_>>();
    let maximum = hits.iter().copied().max().unwrap_or(0);
    input
        .candidates
        .iter()
        .zip(hits)
        .map(|(c, hits)| {
            let v = &c.curve.chunk_vector;
            ReferencePoolSignals {
                id: c.curve.id,
                query: reference_cosine(&o.original_vector, v),
                denoised: reference_cosine(&o.enhanced_vector, v),
                local: reference_cosine(&o.local_vector, v),
                transfer: reference_cosine(&o.transfer_vector, v),
                bm25: c.bm25_score,
                time: c.time_score,
                anchor: if maximum == 0 {
                    c.anchor_score
                } else {
                    c.anchor_score.max(hits as f64 / maximum as f64)
                },
            }
        })
        .collect()
}
fn anchors(
    input: &ReferenceReadoutInput,
    selected: &[ReferenceSelectedCandidate],
) -> Vec<ReferenceAnchor> {
    let o = &input.observation;
    let direct = o
        .provenance
        .iter()
        .filter(|p| p.hop == 0 && matches!(p.source_type.as_str(), "core" | "seed"))
        .map(|p| p.id)
        .collect::<BTreeSet<_>>();
    let fallback = direct.is_empty();
    let curves = input
        .candidates
        .iter()
        .map(|c| (c.curve.id, &c.curve))
        .collect::<BTreeMap<_, _>>();
    reference_anchors(&ReferenceAnchorInput {
        curves: selected.iter().map(|s| curves[&s.id].clone()).collect(),
        seeds: o
            .source_field
            .iter()
            .filter(|(id, mass)| *mass > 0.0 && (fallback || direct.contains(id)))
            .copied()
            .collect(),
        seed_vectors: input.tag_vectors.clone(),
        inbound: input.inbound.clone(),
        fallback,
        config: input.config.anchor.clone(),
    })
}
fn geometry(
    input: &ReferenceReadoutInput,
    curve: &ReferenceCurve,
    edges: &[(i64, i64, f64)],
) -> ReferencePathGeometry {
    let o = &input.observation;
    reference_path_geometry(&ReferencePathInput {
        curve: curve.clone(),
        local_field: o.local_field.clone(),
        transfer_field: o.transfer_field.clone(),
        local_domain: o.local_domain.clone(),
        transfer_domain: o.transfer_domain.clone(),
        edges: edges.to_vec(),
        config: input.config.path.clone(),
    })
}
struct CandidateMeasurement {
    id: i64,
    geometry: ReferencePathGeometry,
    relative_topology: ReferenceRelativeTopology,
    observables: ReferenceObservables,
    pure: ReferencePureScores,
    anchor: ReferenceAnchor,
}
fn candidate(
    input: &ReferenceReadoutInput,
    curve: &ReferenceCandidateCurve,
    signals: &ReferencePoolSignals,
    edges: &[(i64, i64, f64)],
    morphology: &ReferenceMorphology,
    anchor: ReferenceAnchor,
) -> CandidateMeasurement {
    let o = &input.observation;
    let geometry = geometry(input, &curve.curve, edges);
    let topology = reference_relative_topology(&ReferenceTopologyInput {
        curve: curve.curve.clone(),
        file_id: curve.file_id,
        river: o.river.clone(),
        query_tag_vectors: input.tag_vectors.clone(),
        provenance: input
            .candidate_provenance
            .iter()
            .find(|(id, _)| *id == curve.curve.id)
            .map(|(_, edges)| edges.clone())
            .unwrap_or_else(|| input.edge_provenance.clone()),
        config: input.config.topology.clone(),
    });
    let observables = reference_observables(&ReferenceObservableInput {
        curve: curve.curve.clone(),
        query_vector: o.original_vector.clone(),
        source_ids: o.source_field.iter().map(|(id, _)| *id).collect(),
        local_field: o.local_field.clone(),
        transfer_field: o.transfer_field.clone(),
        local_domain: o.local_domain.clone(),
        transfer_domain: o.transfer_domain.clone(),
        geometry: geometry.clone(),
        visible: input.allowed_file_ids.is_empty()
            || input.allowed_file_ids.contains(&curve.file_id),
    });
    let pure = reference_pure_scores(&ReferencePureInput {
        query_score: signals.query,
        local_score: signals.local,
        transfer_score: signals.transfer,
        geometry: &geometry,
        observables: &observables,
        topology: &topology,
        morphology,
        config: &input.config.pure,
    });
    CandidateMeasurement {
        id: curve.curve.id,
        geometry,
        relative_topology: topology,
        observables,
        pure,
        anchor,
    }
}

pub fn reference_v3_readout(input: &ReferenceReadoutInput) -> Result<ReferenceReadoutOutput> {
    input.transport.validate()?;
    let edges = input
        .transport
        .node_ids
        .iter()
        .enumerate()
        .flat_map(|(i, id)| {
            (input.transport.row_offsets[i]..input.transport.row_offsets[i + 1]).map(move |edge| {
                (
                    *id,
                    input.transport.node_ids[input.transport.targets[edge]],
                    input.transport.weights[edge],
                )
            })
        })
        .collect::<Vec<_>>();
    let signals = signals(input);
    let selected = reference_candidate_pool(&signals, &input.config.pool);
    let anchors = anchors(input, &selected);
    let morphology = reference_morphology(&input.observation.river);
    let omega = reference_omega(&input.observation.river, &input.config.omega);
    let curves = input
        .candidates
        .iter()
        .map(|c| (c.curve.id, c))
        .collect::<BTreeMap<_, _>>();
    let signals = signals
        .iter()
        .map(|s| (s.id, s))
        .collect::<BTreeMap<_, _>>();
    let measurements = selected
        .iter()
        .zip(anchors)
        .map(|(s, anchor)| {
            candidate(
                input,
                curves[&s.id],
                signals[&s.id],
                &edges,
                &morphology,
                anchor,
            )
        })
        .collect::<Vec<_>>();
    let scoring = reference_v3_scores(&ReferenceScoreInput {
        mode: morphology.dominant_mode.clone(),
        omega: omega.omega,
        config: input.config.scoring.clone(),
        candidates: measurements
            .iter()
            .map(|r| ReferenceScoreCandidate {
                id: r.id,
                pure_score: r.pure.pure_score,
                graph_score: r.pure.graph_score,
                query_score: signals[&r.id].query,
                closure: r.observables.closure,
                direct_evidence: r.pure.direct_evidence,
                matched_edge_coverage: r.relative_topology.matched_edge_coverage,
                matched_node_coverage: r.relative_topology.matched_node_coverage,
                node_alignment_score: r.relative_topology.node_alignment_score,
                topology_reliability: r.relative_topology.reliability,
                anchor_strength: r.anchor.strength,
            })
            .collect(),
    });
    let mut results = measurements
        .into_iter()
        .zip(scoring)
        .map(|(m, scoring)| ReferenceReadoutResult {
            id: m.id,
            rank: 0,
            score: scoring.final_score,
            base_score: m.pure.pure_score,
            geometry: m.geometry,
            relative_topology: m.relative_topology,
            observables: m.observables,
            pure: m.pure,
            anchor: m.anchor,
            scoring,
        })
        .collect::<Vec<_>>();
    let pool = selected
        .iter()
        .map(|s| (s.id, s))
        .collect::<BTreeMap<_, _>>();
    results.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then_with(|| pool[&b.id].union_score.total_cmp(&pool[&a.id].union_score))
            .then_with(|| pool[&a.id].union_rank.cmp(&pool[&b.id].union_rank))
    });
    results.truncate(input.top_k.max(1));
    for (i, r) in results.iter_mut().enumerate() {
        r.rank = i + 1;
    }
    Ok(ReferenceReadoutOutput {
        morphology,
        omega,
        selected,
        results,
    })
}
