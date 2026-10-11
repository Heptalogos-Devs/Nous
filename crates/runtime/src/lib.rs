//! Model-independent Session continuity, consumer context and query orchestration.

// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

mod policy;
pub use policy::execution::QUERY_SLOTS;
mod clock;
mod context;
mod experience;
mod maintenance;
mod query;
mod query_feedback;
mod resources;
mod segmentation;
mod sessions;
mod types;
mod use_feedback;
pub use query_feedback::{
    LinkedQueryFeedback, QUERY_FEEDBACK_RETENTION, QueryFeedbackSignals, linked_query_feedback,
};
mod work_contexts;
pub use clock::{CognitiveClock, ManualCognitiveClock, SystemCognitiveClock};
pub use context::*;
pub use experience::ExperienceInput;
pub use maintenance::*;
pub use policy::episode::SETTLE_DELAY_KEY;
pub use policy::episode::{EpisodePolicy, ExperienceContext};
pub use policy::maintenance::*;
pub use query::{
    AccessibilityQueryPolicy, ActivationSeed, ActivationSource, AuthorityReadView, BoundQuery,
    COGNITIVE_PROFILE, CONCEPT_ENRICHMENT, CognitiveContributor, CognitiveContributors,
    CognitiveProfile, CognitiveProfileRequirements, ConceptActivationPolicy, ConceptEnrichment,
    DEFAULT_RESULT_LIMIT, LaneCandidate, LaneOutput, LaneStatus, NovelConceptHypothesis,
    QUERY_REPRESENTATION, QueryActivation, QueryActivationView, QueryConceptCandidate,
    QueryConceptOutput, QueryConceptSelection, QueryContextSnapshot, QueryExecution, QueryHostPlan,
    QueryHostRole, QueryOwner, QueryPlan, QueryReadLease, QueryRepresentation,
    QueryRepresentationLimits, QueryRunHostPolicy, QuerySemanticEmbedding, ReadContext,
    RetrievalPolicy, SharedLaneProvider, TagActivation, TopologyWorkSummary, WorkCycle,
    build_query_representation, query_owner, register_retrieval_configuration,
};
pub use segmentation::{EpisodeDraft, SegmentationProgress};
pub use work_contexts::*;

pub use resources::*;
pub use types::*;

use chrono::{DateTime, Utc};
use nous_core::*;
use nous_persistence::AuthorityStore;
use std::collections::HashSet;
use uuid::Uuid;

pub const RESIDENT_LIMIT_KEY: nous_configuration::ConfigKey<usize> =
    nous_configuration::ConfigKey::new("runtime.resident_limit");

pub fn register_configuration(
    registry: &mut nous_configuration::ConfigRegistryBuilder,
) -> Result<()> {
    policy::execution::register_configuration(registry)?;
    query_feedback::register(registry)?;
    registry.register(
        RESIDENT_LIMIT_KEY,
        "cognitive-runtime",
        "Maximum resident references per session.",
        256,
        nous_configuration::ConfigExposure::Developer,
        nous_configuration::ConfigScopePolicy::SystemOnly,
        nous_configuration::ConfigApplyMode::RestartProcess,
        nous_configuration::ConfigSemanticEffect::Operational,
        |value| {
            if *value > 0 {
                Ok(())
            } else {
                Err(Error::new(
                    nous_core::ErrorOwner::Runtime,
                    nous_core::ErrorCode::InvalidRequest,
                    "resident_limit must be positive",
                ))
            }
        },
    )?;
    policy::episode::register_episode_configuration(registry)?;
    policy::maintenance::register_maintenance_configuration(registry)?;
    register_retrieval_configuration(registry)
}

#[derive(Clone)]
pub struct CognitiveRuntimeService {
    pub store: AuthorityStore,
    pub resident_limit: usize,
    pub configuration: nous_configuration::ConfigurationService,
    clock: std::sync::Arc<dyn CognitiveClock>,
    query_slots: std::sync::Arc<tokio::sync::Semaphore>,
    query_slot_admission: std::sync::Arc<std::sync::Mutex<()>>,
}

impl CognitiveRuntimeService {
    pub fn new(
        store: AuthorityStore,
        resident_limit: usize,
        configuration: nous_configuration::ConfigurationService,
    ) -> Result<Self> {
        Self::with_clock(
            store,
            resident_limit,
            configuration,
            std::sync::Arc::new(SystemCognitiveClock),
        )
    }

    pub fn with_clock(
        store: AuthorityStore,
        resident_limit: usize,
        configuration: nous_configuration::ConfigurationService,
        clock: std::sync::Arc<dyn CognitiveClock>,
    ) -> Result<Self> {
        if resident_limit == 0 {
            return Err(Error::new(
                nous_core::ErrorOwner::Runtime,
                nous_core::ErrorCode::InvalidRequest,
                "resident_limit must be positive",
            ));
        }
        Ok(Self {
            store,
            resident_limit,
            configuration,
            clock,
            query_slots: std::sync::Arc::new(tokio::sync::Semaphore::new(64)),
            query_slot_admission: Default::default(),
        })
    }

    pub fn now(&self, subject: SubjectId) -> DateTime<Utc> {
        self.clock.now(subject)
    }

    pub async fn require_subject(&self, subject: SubjectId) -> Result<()> {
        self.store.require_subject(subject).await
    }

    pub async fn reference_in_subject(
        &self,
        subject: SubjectId,
        reference: &CognitiveRef,
    ) -> Result<bool> {
        self.store.reference_in_subject(subject, reference).await
    }
}
