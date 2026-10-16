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
            clock,
            pending_queries: Default::default(),
        })
    }

    /// UTC infrastructure/record time is independent of the Subject semantic clock.
    pub fn recorded_now(&self) -> DateTime<Utc> {
        Utc::now()
    }
    pub async fn read_clock(&self, subject: SubjectId) -> Result<DomainStamp> {
        self.clock.read_stamp(subject).await
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
        let stamp = self.clock.capture_in(tx, subject).await?;
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

    /// Mapping only chooses a real scheduler wake-up. It does not make a
    /// semantic deadline true or alter network leases/backoff.
    pub fn semantic_deadline(
        &self,
        subject: SubjectId,
        domain: &str,
        at: DateTime<Utc>,
    ) -> Option<DateTime<Utc>> {
        self.clock.real_deadline(subject, domain, at)
    }

    /// Cognitive settling uses a same-domain semantic coordinate. The returned
    /// UTC time is merely a wake-up; paused/unmapped time never becomes settled
    /// because a real polling interval elapsed.
    pub fn semantic_settlement(
        &self,
        subject: SubjectId,
        source: &DomainStamp,
        delay: chrono::Duration,
        frame: &DomainStamp,
    ) -> Result<(bool, DateTime<Utc>)> {
        let due = source
            .at
            .checked_add_signed(delay)
            .ok_or_else(|| Error::Invalid("semantic settling timestamp overflow".into()))?;
        let current = if source.time_domain_ref == "time:real" {
            Some(frame.recorded_at)
        } else if source.time_domain_ref == frame.time_domain_ref {
            Some(frame.at)
        } else {
            None
        };
        let settled = current.is_some_and(|at| at >= due);
        let wake = self
            .semantic_deadline(subject, &source.time_domain_ref, due)
            .unwrap_or_else(|| frame.recorded_at + delay.max(chrono::Duration::seconds(1)));
        Ok((settled, wake))
    }

    /// Explicit clock control makes existing cognitive obligations eligible for
    /// reevaluation. It does not grant execution/model budget or clear retries.
    pub async fn clock_changed(&self, subject: SubjectId) -> Result<()> {
        self.require_subject(subject).await?;
        let now = self.recorded_now();
        let mut tx = self.store.begin().await?;
        sqlx::query("UPDATE maintenance_needs SET due_at=LEAST(due_at,$2),updated_at=$2,trigger_revision=trigger_revision+1 WHERE subject_id=$1 AND state='pending' AND kind IN ('episode_segment','episode_resegment','journal_review','journal_revalidate','memory_consolidate')")
            .bind(subject.0).bind(now).execute(&mut *tx).await.map_err(nous_persistence::database_error)?;
        let seq: i64 = sqlx::query_scalar("SELECT authority_seq FROM subjects WHERE subject_id=$1")
            .bind(subject.0)
            .fetch_one(&mut *tx)
            .await
            .map_err(nous_persistence::database_error)?;
        sqlx::query("INSERT INTO maintenance_needs(need_id,subject_id,kind,scope_kind,scope_ref,trigger_authority_seq,due_at,priority,state,created_at,updated_at) VALUES($1,$2,'episode_segment','track','interaction',$3,$4,50,'pending',$4,$4) ON CONFLICT(subject_id,kind,scope_kind,scope_ref) WHERE state IN ('pending','blocked') DO NOTHING")
            .bind(Uuid::now_v7()).bind(subject.0).bind(seq).bind(now).execute(&mut *tx).await.map_err(nous_persistence::database_error)?;
        tx.commit()
            .await
            .map_err(nous_persistence::database_error)?;
        Ok(())
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
