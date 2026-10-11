// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use crate::{QueryRiver, SourceSeed, WaveGraphGeneration, propagate_with_budget};
use nous_core::{Result, ServingGenerationId, TimePredicate};
use serde::Serialize;

pub const NATIVE_MECHANISM_ID: &str = "experimental-node-potential-v1";

#[derive(Debug, Clone, Default, Serialize)]
pub struct QueryTemporalContext {
    pub occurred: Option<TimePredicate>,
    pub observed: Option<TimePredicate>,
    pub valid: Option<TimePredicate>,
    pub formed: Option<TimePredicate>,
    pub recorded: Option<TimePredicate>,
}

/// One frozen propagation observation. Candidate ordering belongs to readout,
/// never to this value. The river is owned here, without a second copy of its
/// node, edge or provenance state. No mutation API is exposed.
#[derive(Debug, Serialize)]
pub struct QueryObservation {
    query_id: String,
    profile_id: nous_runtime::CognitiveProfile,
    profile_digest: String,
    topology_generation_id: ServingGenerationId,
    config_subset_digest: String,
    bound_at: chrono::DateTime<chrono::Utc>,
    source_seeds: Vec<SourceSeed>,
    temporal_context: QueryTemporalContext,
    river: QueryRiver,
}

impl QueryObservation {
    pub(crate) fn native(
        graph: &WaveGraphGeneration,
        bound: &nous_runtime::BoundQuery,
        plan: &nous_runtime::QueryPlan,
        source_seeds: Vec<SourceSeed>,
    ) -> Result<Self> {
        let constraints = &bound.source_query.expression.constraints;
        let config_subset_digest = crate::assets::files::digest(&serde_json::json!({
            "wave": graph.config,
            "max_hops": plan.topology_rounds,
            "max_states": plan.topology_nodes,
        }))?;
        let river = propagate_with_budget(
            graph,
            &source_seeds,
            plan.topology_rounds,
            plan.topology_nodes,
        );
        Ok(Self {
            query_id: bound.query_id.to_string(),
            profile_id: plan.cognitive_profile,
            profile_digest: plan.cognitive_profile.digest(),
            topology_generation_id: graph.generation_id,
            config_subset_digest,
            bound_at: bound.bound_at,
            source_seeds,
            temporal_context: QueryTemporalContext {
                occurred: constraints.occurred,
                observed: constraints.observed,
                valid: constraints.valid,
                formed: constraints.formed,
                recorded: constraints.recorded,
            },
            river,
        })
    }

    pub fn river(&self) -> &QueryRiver {
        &self.river
    }
    pub fn source_seeds(&self) -> &[SourceSeed] {
        &self.source_seeds
    }
    pub fn profile_id(&self) -> &str {
        self.profile_id.id()
    }
    pub fn profile_digest(&self) -> &str {
        &self.profile_digest
    }
    pub fn topology_generation_id(&self) -> ServingGenerationId {
        self.topology_generation_id
    }
    pub fn config_subset_digest(&self) -> &str {
        &self.config_subset_digest
    }
    pub fn temporal_context(&self) -> &QueryTemporalContext {
        &self.temporal_context
    }
}
