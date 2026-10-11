// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use crate::{reference::*, *};
use nous_runtime::{BoundQuery, QueryPlan};

/// Request-local numerical observation. It owns one pipeline output and has
/// no ranking, mutation, provider invocation, or process cache API.
#[derive(Debug, Serialize)]
pub struct VcpQueryObservation {
    query_id: String,
    profile: nous_runtime::CognitiveProfile,
    generation_id: ServingGenerationId,
    config_subset_digest: String,
    bound_at: chrono::DateTime<chrono::Utc>,
    temporal_context: QueryTemporalContext,
    original_vector: Vec<f32>,
    policy: VcpQueryPolicy,
    core_tag_ids: Vec<i64>,
    query_seeds: Vec<ReferenceSenseSeed>,
    numerical: ReferencePipelineOutput,
}
impl VcpQueryObservation {
    pub fn prepare(
        generation: &VcpServingGeneration,
        bound: &BoundQuery,
        plan: &QueryPlan,
        embedding: &TextEmbeddingOutput,
        signals: Option<&crate::PreparedQuerySignals>,
    ) -> Result<Self> {
        if bound.source_query.capabilities.text_embedding == RequirementStrength::Forbidden {
            return Err(Error::Unavailable(
                "bound query forbids VCP embedding observation".into(),
            ));
        }
        if !matches!(
            plan.cognitive_profile,
            nous_runtime::CognitiveProfile::VcpDtsc | nous_runtime::CognitiveProfile::VcpRiverMemo
        ) || bound.retrieval_policy.cognitive_profile != plan.cognitive_profile
            || crate::assets::files::digest(&generation.policy)?
                != crate::assets::files::digest(&bound.config_snapshot.get(VCP_ASSETS)?)?
        {
            return Err(Error::Unavailable(
                "VCP generation disagrees with frozen bound profile/policy".into(),
            ));
        }
        if !embedding.space.compatible_with(&generation.space)
            || embedding.producer.signature_hash != generation.producer.signature_hash
            || bound
                .selected_embedding_space
                .as_ref()
                .is_some_and(|space| !space.compatible_with(&generation.space))
        {
            return Err(Error::Unavailable(
                "VCP query embedding space/producer mismatch".into(),
            ));
        }
        if embedding.vector.len() != generation.space.dimension as usize
            || embedding.vector.iter().any(|v| !v.is_finite())
        {
            return Err(Error::Invalid(
                "VCP query embedding has invalid dimension/values".into(),
            ));
        }
        let mut policy = bound.config_snapshot.get(VCP_QUERY)?;
        policy.validate()?;
        if !plan.expand_topology || plan.topology_nodes < 100 {
            return Err(Error::Unavailable(
                "bound VCP work budget cannot admit frozen Sense minimum".into(),
            ));
        }
        policy.sense.max_safe_hops = policy.sense.max_safe_hops.min(plan.topology_rounds);
        policy.sense.max_propagation_states =
            policy.sense.max_propagation_states.min(plan.topology_nodes);
        let core_tag_ids = bound
            .activation
            .tags()
            .filter_map(|tag| generation.identities.id(&CognitiveRef::Tag(tag.tag)).ok())
            .filter(|id| {
                generation
                    .labels
                    .binary_search_by_key(id, |(id, _)| *id)
                    .is_ok()
            })
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let query_seeds = activation_seeds(generation, bound, signals);
        let mut input = pipeline_input(generation, embedding, &policy, &core_tag_ids);
        input.query_seeds = query_seeds.clone();
        let numerical = reference_query_pipeline(&input, |vector, limit| {
            generation.search_residual_tags(vector, limit)
        })?;
        let constraints = &bound.source_query.expression.constraints;
        let config_subset_digest = crate::assets::files::digest(&serde_json::json!({
            "profile": plan.cognitive_profile, "assets": generation.policy, "query": policy,
            "space": generation.space, "producer": generation.producer.signature_hash,
        }))?;
        Ok(Self {
            query_seeds,
            query_id: bound.query_id.to_string(),
            profile: plan.cognitive_profile,
            generation_id: generation.generation_id,
            config_subset_digest,
            bound_at: bound.bound_at,
            temporal_context: QueryTemporalContext {
                occurred: constraints.occurred,
                observed: constraints.observed,
                valid: constraints.valid,
                formed: constraints.formed,
                recorded: constraints.recorded,
            },
            original_vector: embedding.vector.clone(),
            policy,
            core_tag_ids,
            numerical,
        })
    }
    pub fn numerical(&self) -> &ReferencePipelineOutput {
        &self.numerical
    }
    pub fn original_vector(&self) -> &[f32] {
        &self.original_vector
    }
    pub fn generation_id(&self) -> ServingGenerationId {
        self.generation_id
    }
    pub fn profile_id(&self) -> &str {
        self.profile.id()
    }
    pub fn config_subset_digest(&self) -> &str {
        &self.config_subset_digest
    }
    pub fn policy(&self) -> &VcpQueryPolicy {
        &self.policy
    }
    pub fn core_tag_ids(&self) -> &[i64] {
        &self.core_tag_ids
    }
    pub fn route_seed(&self, id: i64) -> Option<serde_json::Value> {
        self.query_seeds.iter().find(|seed| seed.id == id).map(|seed| serde_json::json!({"id":seed.id,"energy":seed.energy,"source":"query_activation"}))
            .or_else(|| self.numerical.gating.tags.iter().find(|seed| seed.id == id).map(|seed| serde_json::json!(seed)))
    }
    pub fn seed_ids(&self) -> std::collections::BTreeSet<i64> {
        self.numerical
            .gating
            .tags
            .iter()
            .map(|tag| tag.id)
            .chain(self.query_seeds.iter().map(|seed| seed.id))
            .collect()
    }
    pub fn temporal_context(&self) -> &QueryTemporalContext {
        &self.temporal_context
    }
}

fn pipeline_input(
    generation: &VcpServingGeneration,
    embedding: &TextEmbeddingOutput,
    policy: &VcpQueryPolicy,
    core_tag_ids: &[i64],
) -> ReferencePipelineInput {
    let labels = generation
        .labels
        .iter()
        .cloned()
        .collect::<BTreeMap<_, _>>();
    let vectors = generation
        .vectors
        .iter()
        .map(|(id, vector)| (*id, vector))
        .collect::<BTreeMap<_, _>>();
    let transport = &generation.graph.graph.transport;
    let mut edges = Vec::new();
    for (row, source) in transport.node_ids.iter().enumerate() {
        for offset in transport.row_offsets[row]..transport.row_offsets[row + 1] {
            edges.push((
                *source,
                transport.node_ids[transport.targets[offset]],
                transport.weights[offset],
            ));
        }
    }
    ReferencePipelineInput {
        epa: ReferenceEpaInput {
            query: embedding.vector.clone(),
            mean: if generation.epa.success {
                generation.epa.mean.clone()
            } else {
                vec![0.0; embedding.vector.len()]
            },
            basis: generation.epa.basis.clone(),
        },
        epa_labels: generation.epa.labels.clone(),
        core_tags: core_tag_ids.iter().map(|id| labels[id].clone()).collect(),
        query_seeds: Vec::new(),
        ghosts: Vec::new(),
        tag_vectors: labels
            .iter()
            .map(|(id, name)| ReferenceFusionVector {
                id: *id,
                name: name.clone(),
                vector: vectors[id].clone(),
            })
            .collect(),
        graph: ReferenceSenseGraph {
            node_ids: transport.node_ids.clone(),
            edges,
            anchor_gain: generation
                .intrinsic
                .iter()
                .filter_map(|r| r.anchor_gain.map(|gain| (r.id, gain)))
                .collect(),
            wormholes: generation.graph.graph.wormholes.clone(),
        },
        transport: transport.clone(),
        pyramid_config: policy.pyramid.clone(),
        gating_config: policy.gating.clone(),
        sense_config: policy.sense.clone(),
        fusion_config: policy.fusion.clone(),
        field_config: policy.fields.clone(),
    }
}

fn activation_seeds(
    generation: &VcpServingGeneration,
    bound: &BoundQuery,
    signals: Option<&crate::PreparedQuerySignals>,
) -> Vec<ReferenceSenseSeed> {
    let present = |reference: &CognitiveRef| {
        generation
            .identities
            .id(reference)
            .ok()
            .filter(|id| generation.graph.graph.transport.node_ids.contains(id))
    };
    let mut seeds = bound
        .activation
        .seeds
        .iter()
        .filter_map(|seed| {
            present(&seed.reference).map(|id| ReferenceSenseSeed {
                id,
                energy: seed.strength,
                source_type: seed.origin.clone(),
            })
        })
        .collect::<Vec<_>>();
    if seeds.is_empty()
        && let Some(signals) = signals
    {
        for (lane, source) in [
            (signals.lexical(), "lexical_promoted"),
            (signals.dense(), "dense_promoted"),
        ] {
            if let Some(lane) = lane.filter(|lane| {
                matches!(
                    lane.status,
                    nous_runtime::LaneStatus::Ready | nous_runtime::LaneStatus::Truncated
                )
            }) {
                seeds.extend(
                    lane.candidates
                        .iter()
                        .filter(|c| c.rank > 0)
                        .filter_map(|c| {
                            present(&c.reference).map(|id| ReferenceSenseSeed {
                                id,
                                energy: 0.6 / f64::from(c.rank),
                                source_type: source.into(),
                            })
                        })
                        .take(4),
                );
            }
        }
    }
    seeds
}
