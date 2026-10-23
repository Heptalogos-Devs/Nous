// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;
use crate::Result;
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
pub struct ReferencePipelineInput {
    pub epa: ReferenceEpaInput,
    pub epa_labels: Vec<String>,
    pub core_tags: Vec<String>,
    #[serde(default)]
    pub query_seeds: Vec<ReferenceSenseSeed>,
    pub ghosts: Vec<ReferenceFusionGhost>,
    pub tag_vectors: Vec<ReferenceFusionVector>,
    pub graph: ReferenceSenseGraph,
    pub transport: ReferenceTransport,
    pub pyramid_config: ReferencePyramidConfig,
    pub gating_config: ReferenceGateConfig,
    pub sense_config: ReferenceSenseConfig,
    pub fusion_config: ReferenceFusionConfig,
    pub field_config: ReferenceFieldConfig,
}
#[derive(Debug, Serialize)]
pub struct ReferencePipelineOutput {
    pub epa: ReferenceEpaAnalysis,
    pub pyramid: ReferencePyramid,
    pub gating: ReferenceGating,
    pub sense: ReferenceSenseOutput,
    pub fusion: ReferenceFusion,
    pub fields: ReferenceDualFields,
    pub local_vector: Vec<f32>,
    pub transfer_vector: Vec<f32>,
}

/// One numerical observation shared by both candidate readouts. ANN search is
/// supplied by the owning index adapter, and is never called during readout.
pub fn reference_query_pipeline(
    input: &ReferencePipelineInput,
    search: impl FnMut(&[f32], usize) -> Result<Vec<ReferenceResidualCandidate>>,
) -> Result<ReferencePipelineOutput> {
    let query = &input.epa.query;
    let epa = reference_epa_analysis(&input.epa)?;
    let pyramid = reference_pyramid(query, &input.pyramid_config, search)?;
    let primary = epa
        .axis_probabilities
        .iter()
        .enumerate()
        .filter(|(_, p)| **p > 0.05)
        .max_by(|(i, a), (j, b)| a.total_cmp(b).then_with(|| j.cmp(i)))
        .map(|(i, _)| i);
    let world = primary
        .and_then(|i| input.epa_labels.get(i))
        .cloned()
        .unwrap_or_else(|| "Unknown".into());
    let gating = reference_gate_tags(&ReferenceGateInput {
        levels: pyramid
            .levels
            .iter()
            .map(|l| {
                l.tags
                    .iter()
                    .map(|t| ReferenceGateTag {
                        id: t.id,
                        name: t.name.clone(),
                        contribution: t.contribution,
                        similarity: t.similarity,
                    })
                    .collect()
            })
            .collect(),
        logic_depth: epa.logic_depth,
        entropy: epa.entropy,
        resonance: epa.resonance,
        world,
        activation: pyramid.features.activation,
        coverage: pyramid.features.coverage,
        core_tags: input.core_tags.clone(),
        config: input.gating_config.clone(),
    });
    let seeds = sense_seeds(input, &gating);
    let sense = reference_sense(
        &input.graph,
        &ReferenceSenseInput {
            seeds,
            config: input.sense_config.clone(),
        },
    )?;
    let fusion = reference_fuse_observation(&ReferenceFusionInput {
        query: query.clone(),
        nodes: sense
            .nodes
            .iter()
            .map(|n| ReferenceFusionNode {
                id: n.id,
                energy: n.energy,
            })
            .collect(),
        gated_tags: gating.tags.clone(),
        core_tags: input.core_tags.clone(),
        tag_vectors: input.tag_vectors.clone(),
        ghosts: input.ghosts.clone(),
        dynamic_core_boost: gating.dynamic_core_boost,
        alpha: gating.effective_boost,
        config: input.fusion_config.clone(),
    });
    let fields = reference_dual_fields(&input.transport, &sense.source_field, &input.field_config)?;
    let vectors = input
        .tag_vectors
        .iter()
        .map(|t| (t.id, t.vector.clone()))
        .collect::<Vec<_>>();
    let local_vector = reference_field_projection(&ReferenceFieldProjectionInput {
        dimension: query.len(),
        field: fields.local_field.clone(),
        tag_vectors: vectors.clone(),
    });
    let transfer_vector = reference_field_projection(&ReferenceFieldProjectionInput {
        dimension: query.len(),
        field: fields.transfer_field.clone(),
        tag_vectors: vectors,
    });
    Ok(ReferencePipelineOutput {
        epa,
        pyramid,
        gating,
        sense,
        fusion,
        fields,
        local_vector,
        transfer_vector,
    })
}

fn sense_seeds(
    input: &ReferencePipelineInput,
    gating: &ReferenceGating,
) -> Vec<ReferenceSenseSeed> {
    let mut seeds = gating
        .tags
        .iter()
        .map(|t| ReferenceSenseSeed {
            id: t.id,
            energy: t.weight,
            source_type: if t.is_core { "core" } else { "seed" }.into(),
        })
        .collect::<Vec<_>>();
    if !input.query_seeds.is_empty() {
        let mut combined = seeds
            .into_iter()
            .map(|seed| (seed.id, seed))
            .collect::<std::collections::BTreeMap<_, _>>();
        for seed in &input.query_seeds {
            if !seed.energy.is_finite()
                || seed.energy <= 0.0
                || !input.graph.node_ids.contains(&seed.id)
            {
                continue;
            }
            combined
                .entry(seed.id)
                .and_modify(|current| {
                    if seed.energy > current.energy {
                        *current = seed.clone();
                    }
                })
                .or_insert_with(|| seed.clone());
        }
        seeds = combined.into_values().collect();
    }
    seeds
}
