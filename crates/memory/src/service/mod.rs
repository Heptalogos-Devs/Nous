//! Memory Authority orchestration for the R1 reference profile.

// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

mod accessibility;
mod accretion;
mod batch;
mod concept_maintenance;
mod consolidation_context;
mod dependencies;
mod episode;
mod historical;
mod historical_query;
mod journal;
mod lane;
mod longitudinal_policy;
mod longitudinal_query;
mod maintenance_planning;
mod memory;
mod provenance;
mod query;
mod query_materialization;
mod query_support;
pub mod schema;
mod schema_lane;
mod source_classes;
mod source_facts;
mod state;
mod tag;
mod topology;

use crate::*;
use chrono::{DateTime, Utc};
use nous_core::*;
use nous_object_store::ObjectStore;
use nous_persistence::{
    AuthorityStore, MutationEnvelope, MutationStart, OwnerLock, ProjectionInvalidation,
    database_error as db,
};
pub use nous_runtime::{ResidentView, ResourceUpsert, ResourceView, SessionView};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sqlx::Row;
use state::{fence_epoch, require_transition};
use std::sync::Arc;
use uuid::Uuid;

pub use crate::{CognitiveRole, FormationMode};
pub use accessibility::{
    AccessibilityPolicy, eligible as accessibility_eligible, register_configuration,
    resolve_accessibility_policy,
};
pub use accretion::{ACCRETION, AccretionPolicy, AccretionSignal};
pub use concept_maintenance::*;
pub use episode::*;
pub use journal::*;
pub use longitudinal_policy::{
    CONSOLIDATION_CONTEXT, CONSOLIDATION_MAX_ACTIONS, ConsolidationContextPolicy, EPISODE_SYNOPSIS,
};
pub use maintenance_planning::MaintenanceScope;
pub use nous_core::TopologyRelation;
pub use nous_core::{CognitiveQuery, CognitiveQueryResult};
pub use tag::*;

#[derive(Clone)]
pub struct MemoryService {
    pub configuration: nous_configuration::ConfigurationService,
    pub store: AuthorityStore,
    pub objects: ObjectStore,
    pub material: nous_material::MaterialService,
    pub cognition: nous_runtime::CognitiveRuntimeService,
    capabilities: Arc<Vec<CapabilityDescriptor>>,
}

impl MemoryService {
    async fn start_mutation(
        &self,
        subject: SubjectId,
        operation: OperationId,
        kind: &str,
        digest: &str,
    ) -> Result<MutationStart<'_>> {
        let mut start = MutationEnvelope::begin(
            &self.store,
            subject,
            operation,
            kind,
            digest,
            Some(OwnerLock("memory-authority")),
        )
        .await?;
        if let MutationStart::Active(mutation) = &mut start {
            mutation
                .capture_authority_time(self.cognition.now(subject))
                .await?;
        }
        Ok(start)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeStatus {
    pub ready: bool,
    pub authority: String,
    pub capabilities: Vec<CapabilityStatus>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryView {
    pub object: MemoryObject,
    pub revision: MemoryRevision,
    pub basis: Vec<RevisionBasis>,
    pub aboutness: Vec<EntityRef>,
    pub tags: Vec<TagId>,
    pub relations: Vec<MemoryRevisionRelation>,
    pub relations_truncated: bool,
    pub accessibility_level: AccessibilityLevel,
    pub temporal_evidence: TemporalEvidence,
    #[serde(default)]
    pub source_classes: Vec<SourceClass>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviseMemoryInput {
    #[serde(default)]
    pub producer: Option<ProducerSignature>,
    pub operation_id: OperationId,
    pub subject: SubjectId,
    pub memory_id: MemoryId,
    pub expected_object_epoch: i64,
    pub intent: RevisionIntent,
    pub formation_mode: FormationMode,
    pub grounding_occurrence_id: Option<OccurrenceId>,
    pub semantic_role: String,
    pub representation_text: String,
    pub title: Option<String>,
    pub basis: Vec<RevisionBasis>,
    pub aboutness: Vec<EntityRef>,
    pub valid_time: TemporalExtent,

    pub epistemic_class: EpistemicClass,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateTagRequest {
    pub producer: Option<ProducerSignature>,
    pub operation_id: OperationId,
    pub label: String,
    pub description: Option<String>,
    pub kind_hint: Option<String>,
    #[serde(default = "default_explicit_origin")]
    pub origin: String,
}

fn default_explicit_origin() -> String {
    "explicit".into()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateAssociationRequest {
    pub producer: Option<ProducerSignature>,
    pub operation_id: OperationId,
    pub from: CognitiveRef,
    pub to: CognitiveRef,
    pub relation_kind: String,
    pub polarity: AssociationPolarity,
    pub basis_class: AssociationBasisClass,
    pub basis: Vec<AssociationBasis>,
    pub producer_signature_id: Option<Uuid>,
    pub valid_time: TemporalExtent,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RebindEntityRequest {
    pub mention_id: Uuid,
    pub entity_ref: Option<EntityRef>,
    pub binding_state: String,
    pub host_resolution_ref: Option<String>,
    pub reason: Option<String>,
}

fn parse_enum<T: DeserializeOwned>(value: String, name: &str) -> Result<T> {
    serde_json::from_value(serde_json::Value::String(value))
        .map_err(|error| Error::Infrastructure(format!("invalid {name}: {error}")))
}

fn temporal_from_columns(
    kind: String,
    start: Option<DateTime<Utc>>,
    end: Option<DateTime<Utc>>,
) -> Result<TemporalExtent> {
    match kind.as_str() {
        "unknown" => Ok(TemporalExtent::Unknown),
        "instant" => Ok(TemporalExtent::Instant {
            at: start.ok_or_else(|| Error::Infrastructure("instant has no timestamp".into()))?,
        }),
        "interval" => Ok(TemporalExtent::Interval { start, end }),
        _ => Err(Error::Infrastructure("invalid temporal extent kind".into())),
    }
}

fn temporal_columns(
    value: &TemporalExtent,
) -> (&'static str, Option<DateTime<Utc>>, Option<DateTime<Utc>>) {
    match value {
        TemporalExtent::Unknown => ("unknown", None, None),
        TemporalExtent::Instant { at } => ("instant", Some(*at), None),
        TemporalExtent::Interval { start, end } => ("interval", *start, *end),
    }
}

fn operation_digest<T: Serialize>(kind: &str, subject: SubjectId, value: &T) -> Result<String> {
    canonical_request_digest(kind, subject, value)
}

impl MemoryService {
    async fn formation_time_in(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        subject: SubjectId,
        operation: OperationId,
        started_at: DateTime<Utc>,
    ) -> Result<DateTime<Utc>> {
        Ok(AuthorityStore::workflow_cognitive_time_in(
            tx,
            subject,
            "memory",
            &operation.0.to_string(),
        )
        .await?
        .unwrap_or(started_at))
    }

    pub fn new(
        material: nous_material::MaterialService,
        configuration: nous_configuration::ConfigurationService,
    ) -> Self {
        Self {
            configuration,
            store: material.store.clone(),
            objects: material.objects.clone(),
            cognition: material.cognition.clone(),
            material,
            capabilities: Arc::new(Vec::new()),
        }
    }

    pub fn with_capabilities(mut self, capabilities: Vec<CapabilityDescriptor>) -> Self {
        self.capabilities = Arc::new(capabilities);
        self
    }

    pub async fn status(&self) -> RuntimeStatus {
        RuntimeStatus {
            ready: true,
            authority: "postgresql".into(),
            capabilities: self
                .capabilities
                .iter()
                .map(|capability| CapabilityStatus {
                    capability_id: capability.operation.as_str().into(),
                    status: match capability.readiness {
                        CapabilityReadiness::Ready => Readiness::Ready,
                        CapabilityReadiness::Degraded => Readiness::Degraded,
                        CapabilityReadiness::Unavailable => Readiness::Unavailable,
                    },
                    reason: None,
                })
                .collect(),
        }
    }
}
