mod concept_model;
mod historical_context;
pub use concept_model::*;
// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

mod activation;
pub use activation::*;
mod representation;
pub use representation::{
    QUERY_REPRESENTATION, QueryContextSnapshot, QueryRepresentation, QueryRepresentationLimits,
    build_query_representation,
};
mod closure;
mod cognitive_profile;
pub use cognitive_profile::{COGNITIVE_PROFILE, CognitiveProfile, CognitiveProfileRequirements};
mod bind;
mod lane;
mod orchestrate;
mod plan;
mod preferences;
mod ranking;
mod tree;
mod types;
pub use bind::planned_lanes;
pub use lane::{LaneCandidate, LaneOutput, LaneStatus, TopologyWorkSummary};
pub use orchestrate::{CognitiveContributor, CognitiveContributors, SharedLaneProvider};
pub use plan::{QueryPlan, WorkCycle};
pub use ranking::{
    CandidateRankInput, DEFAULT_RESULT_LIMIT, RetrievalPolicy, rank_candidates_with_policy,
    register_retrieval_configuration, resolve_retrieval_policy,
};
pub use types::{
    AccessibilityQueryPolicy, AuthorityReadView, BoundQuery, QueryActivationView, QueryExecution,
    QueryReadLease, ReadContext,
};
pub(super) mod prepared;
pub use prepared::{
    QueryHostPlan, QueryHostRole, QueryLease, QueryRun, QueryRunHandle, QueryRunHostPolicy,
    QueryRunStage,
};
