//! Model-independent Session continuity, consumer context and query orchestration.

// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

mod planning;
mod policy;
pub use planning::{ExperienceMetadata, UseSummary};
pub use policy::execution::QUERY_SLOTS;
mod clock;
mod subject_clock;
pub use subject_clock::{ClockChange, ClockSnapshot, SubjectClockService};
mod experience;
mod history;
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
mod projection;
mod work_contexts;
pub use clock::{CognitiveClock, ManualCognitiveClock, SystemCognitiveClock};
pub use experience::ExperienceInput;
pub use maintenance::*;
pub use policy::episode::SETTLE_DELAY_KEY;
pub use policy::episode::{EpisodePolicy, ExperienceContext};
pub use policy::maintenance::*;
pub use projection::*;
pub use query::{
    ActivationSeed, ActivationSource, BoundQuery, COGNITIVE_PROFILE, CONCEPT_ENRICHMENT,
    CognitiveContributor, CognitiveContributors, CognitiveProfile, CognitiveProfileRequirements,
    ConceptActivationPolicy, ConceptEnrichment, DEFAULT_RESULT_LIMIT, LaneCandidate, LaneOutput,
    LaneStatus, NovelConceptHypothesis, QUERY_REPRESENTATION, QueryActivation, QueryActivationView,
    QueryConceptCandidate, QueryConceptOutput, QueryConceptSelection, QueryContextSnapshot,
    QueryExecution, QueryHostCapabilities, QueryLease, QueryPlan, QueryRepresentation,
    QueryRepresentationLimits, QueryReservation, QuerySemanticEmbedding, QuerySource,
    SharedLaneProvider, TagActivation, TopologyWorkSummary, build_query_representation,
    register_retrieval_configuration,
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
                Err(Error::Invalid("resident_limit must be positive".into()))
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
    semantic_clock: std::sync::Arc<dyn CognitiveClock>,
    pending_queries: std::sync::Arc<
        std::sync::Mutex<std::collections::HashMap<Uuid, query::prepared::PendingQuery>>,
    >,
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
            return Err(Error::Invalid("resident_limit must be positive".into()));
        }
        Ok(Self {
            store,
            resident_limit,
            configuration,
            semantic_clock: clock.clone(),
            clock,
            pending_queries: Default::default(),
        })
    }

    pub fn with_semantic_clock(mut self, clock: std::sync::Arc<dyn CognitiveClock>) -> Self {
        self.semantic_clock = clock;
        self
    }

    pub fn now(&self, subject: SubjectId) -> DateTime<Utc> {
        self.clock.now(subject)
    }

    /// UTC infrastructure/record time is independent of the Subject semantic clock.
    pub fn recorded_now(&self) -> DateTime<Utc> {
        Utc::now()
    }
    pub async fn read_clock(&self, subject: SubjectId) -> Result<DomainStamp> {
        self.semantic_clock.read_stamp(subject).await
    }
    /// One semantic instant per committing owner transaction; no second pool checkout.
    pub async fn capture_semantic_in(
        &self,
        tx: &mut nous_persistence::AuthorityTransaction<'_>,
        subject: SubjectId,
    ) -> Result<DomainStamp> {
        let (prior, prior_subject):(Option<String>,Option<String>)=sqlx::query_as("SELECT NULLIF(current_setting('nous.semantic_stamp',true),''),NULLIF(current_setting('nous.semantic_subject',true),'')")
            .fetch_one(&mut **tx).await.map_err(nous_persistence::database_error)?;
        if let Some(prior) = prior {
            if prior_subject.as_deref() != Some(subject.0.to_string().as_str()) {
                return Err(Error::Invalid(
                    "owner transaction has another semantic Subject".into(),
                ));
            }
            return serde_json::from_str(&prior).map_err(|e| Error::Infrastructure(e.to_string()));
        }
        let stamp = self.semantic_clock.capture_in(tx, subject).await?;
        sqlx::query("SELECT set_config('nous.semantic_stamp',$1,true),set_config('nous.authority_time',$2,true),set_config('nous.semantic_subject',$3,true),set_config('nous.formed_stamp',$1,true)")
            .bind(serde_json::to_string(&stamp).map_err(|e|Error::Infrastructure(e.to_string()))?).bind(stamp.recorded_at.to_rfc3339()).bind(subject.0.to_string()).execute(&mut **tx).await.map_err(nous_persistence::database_error)?;
        Ok(stamp)
    }
    /// Recovered paid formation retains its original semantic domain. New owner
    /// formation uses the single stamp captured with this committing mutation.
    pub async fn formation_stamp_in(
        &self,
        tx: &mut nous_persistence::AuthorityTransaction<'_>,
        subject: SubjectId,
        owner: &str,
        operation: nous_core::OperationId,
    ) -> Result<DomainStamp> {
        let captured = self.capture_semantic_in(tx, subject).await?;
        let stamp = nous_persistence::AuthorityStore::model_result_cognitive_stamp_in(
            tx,
            subject,
            owner,
            &operation.0.to_string(),
        )
        .await?
        .unwrap_or(captured);
        sqlx::query("SELECT set_config('nous.formed_stamp',$1,true)")
            .bind(serde_json::to_string(&stamp).map_err(|e| Error::Infrastructure(e.to_string()))?)
            .execute(&mut **tx)
            .await
            .map_err(nous_persistence::database_error)?;
        Ok(stamp)
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

mod source_read;
pub use source_read::*;

mod reflection;
pub use reflection::*;
