//! Memory Authority orchestration for the R1 reference profile.

// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

mod accessibility;
mod accretion;
mod cognition;
mod concept_maintenance;
mod consolidation_context;
mod dependencies;
mod episode;
mod historical;
mod historical_query;
mod journal;
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
pub use nous_persistence::MutationResult;
use nous_persistence::{
    AuthorityStore, MutationEnvelope, MutationReceipt, MutationStart, OwnerLock,
    ProjectionInvalidation, database_error as db,
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
pub use cognition::*;
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

pub(super) fn cognition_owner(kind: CognitionKind) -> ErrorOwner {
    if kind == CognitionKind::CognitiveSchema {
        ErrorOwner::Concept
    } else {
        ErrorOwner::Memory
    }
}

pub(super) fn single_mutation_result(
    receipt: MutationReceipt,
    owner: ErrorOwner,
) -> Result<MutationResult> {
    match receipt.results.as_slice() {
        [result] => Ok(result.clone()),
        _ => Err(Error::new(
            owner,
            ErrorCode::InternalFailure,
            "single-result mutation receipt has an invalid result count",
        )),
    }
}

pub(super) fn decode_cognition_header(
    row: &sqlx::postgres::PgRow,
    kind: CognitionKind,
    object_id: Uuid,
    current_revision_id: Uuid,
    details: CognitionDetails,
) -> Result<CognitionHeader> {
    let owner = cognition_owner(kind);
    Ok(CognitionHeader::new(
        SubjectId(row.try_get("subject_id").map_err(db)?),
        object_id,
        current_revision_id,
        row.try_get("object_epoch").map_err(db)?,
        CognitionLifecycle {
            acceptance: parse_enum(
                owner,
                row.try_get("acceptance_state").map_err(db)?,
                "acceptance state",
            )?,
            integrity: parse_enum(
                owner,
                row.try_get("integrity_state").map_err(db)?,
                "integrity state",
            )?,
            suppression: parse_enum(
                owner,
                row.try_get("suppression_state").map_err(db)?,
                "suppression state",
            )?,
            purge: parse_enum(
                owner,
                row.try_get("purge_state").map_err(db)?,
                "purge state",
            )?,
        },
        row.try_get("created_at").map_err(db)?,
        details,
    ))
}

pub(super) fn decode_cognition_revision_header(
    row: &sqlx::postgres::PgRow,
    kind: CognitionKind,
    object_id: Uuid,
    revision_id: Uuid,
) -> Result<CognitionRevisionHeader> {
    Ok(CognitionRevisionHeader::new(
        kind,
        object_id,
        revision_id,
        row.try_get("revision_no").map_err(db)?,
        row.try_get("parent_revision_id").map_err(db)?,
        row.try_get("formed_at").map_err(db)?,
        row.try_get("recorded_at").map_err(db)?,
        row.try_get("producer_signature_id").map_err(db)?,
    ))
}

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
    pub(in crate::service) async fn insert_cognition_revision_header_in(
        store: &AuthorityStore,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        subject: SubjectId,
        header: &CognitionRevisionHeader,
        display_name: &str,
    ) -> Result<()> {
        sqlx::query("INSERT INTO cognition_revision_headers(revision_id,subject_id,object_kind,object_id,revision_no,parent_revision_id,formed_at,recorded_at,producer_signature_id) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9)")
            .bind(header.revision_id())
            .bind(subject.0)
            .bind(CognitionKind::from_reference(&header.object_ref)?.0.as_str())
            .bind(header.object_id())
            .bind(header.revision_no)
            .bind(header.parent_revision_id())
            .bind(header.formed_at)
            .bind(header.recorded_at)
            .bind(header.producer_signature_id)
            .execute(&mut **tx)
            .await
            .map_err(db)?;
        store
            .ensure_identity_addresses_in(
                tx,
                subject,
                &[header.object_ref.clone(), header.revision_ref.clone()],
                display_name,
            )
            .await?;
        Ok(())
    }

    async fn start_mutation(
        &self,
        owner: ErrorOwner,
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
            owner,
        )
        .await?;
        if let MutationStart::Active(mutation) = &mut start {
            mutation
                .capture_authority_time(self.cognition.now(subject))
                .await?;
        }
        Ok(start)
    }

    async fn resume_mutation(
        &self,
        error_owner: ErrorOwner,
        subject: SubjectId,
        operation: OperationId,
        kind: &str,
        digest: &str,
    ) -> Result<MutationStart<'_>> {
        let mut start = MutationEnvelope::resume(
            &self.store,
            subject,
            operation,
            kind,
            digest,
            Some(OwnerLock("memory-authority")),
            error_owner,
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
    pub header: CognitionHeader,
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
    pub tags: Vec<TagId>,
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

fn parse_enum<T: DeserializeOwned>(owner: ErrorOwner, value: String, name: &str) -> Result<T> {
    let message = match owner {
        ErrorOwner::Memory => "Persisted Memory enum is invalid",
        ErrorOwner::Concept => "Persisted CognitiveSchema enum is invalid",
        _ => "Persisted cognition enum is invalid",
    };
    serde_json::from_value(serde_json::Value::String(value)).map_err(|error| {
        Error::new(owner, ErrorCode::InternalFailure, message)
            .with_context("field", name)
            .with_source(error)
    })
}

/// Decode the common persisted support row used by Memory, Episode,
/// CognitiveSchema, and Association evidence.
///
/// Family-specific queries normalize their columns to `basis_kind` and
/// `basis_ref`; this keeps the stored union and its validation in one place.
fn decode_revision_basis(owner: ErrorOwner, row: &sqlx::postgres::PgRow) -> Result<RevisionBasis> {
    let kind: String = row.try_get("basis_kind").map_err(db)?;
    let basis_role = parse_enum(owner, row.try_get("basis_role").map_err(db)?, "basis role")?;
    let epistemic_relation =
        parse_epistemic_relation(row.try_get("epistemic_relation").map_err(db)?)?;

    if kind == "evidence" {
        let occurrence_id = row
            .try_get::<Option<Uuid>, _>("occurrence_id")
            .map_err(db)?
            .ok_or_else(|| {
                Error::new(
                    owner,
                    ErrorCode::InternalFailure,
                    "persisted evidence basis has no occurrence",
                )
            })?;
        let locator = match (
            row.try_get::<Option<Uuid>, _>("source_region_id")
                .map_err(db)?,
            row.try_get::<Option<Uuid>, _>("derived_representation_id")
                .map_err(db)?,
            row.try_get::<Option<Uuid>, _>("derived_region_id")
                .map_err(db)?,
        ) {
            (Some(id), None, None) => EvidenceLocator::SourceRegion(SourceRegionId(id)),
            (None, Some(id), None) => {
                EvidenceLocator::DerivedRepresentation(DerivedRepresentationId(id))
            }
            (None, None, Some(id)) => EvidenceLocator::DerivedRegion(DerivedRegionId(id)),
            (None, None, None) => EvidenceLocator::WholeOccurrence,
            _ => {
                return Err(Error::new(
                    owner,
                    ErrorCode::InternalFailure,
                    "persisted evidence locator is invalid",
                ));
            }
        };
        Ok(RevisionBasis::Evidence(EvidenceRef {
            epistemic_relation,
            occurrence_id: OccurrenceId(occurrence_id),
            locator,
            basis_role,
        }))
    } else {
        let basis_ref = row
            .try_get::<Option<String>, _>("basis_ref")
            .map_err(db)?
            .ok_or_else(|| {
                Error::new(
                    owner,
                    ErrorCode::InternalFailure,
                    "persisted cognition dependency has no target reference",
                )
            })?;
        Ok(RevisionBasis::CognitionDependency(CognitionDependency {
            epistemic_relation,
            target_revision: parse_reference(&kind, &basis_ref)?,
            basis_role,
        }))
    }
}

fn temporal_from_columns(
    owner: ErrorOwner,
    kind: String,
    start: Option<DateTime<Utc>>,
    end: Option<DateTime<Utc>>,
) -> Result<TemporalExtent> {
    match kind.as_str() {
        "unknown" => Ok(TemporalExtent::Unknown),
        "instant" => Ok(TemporalExtent::Instant {
            at: start.ok_or_else(|| {
                Error::new(
                    owner,
                    nous_core::ErrorCode::InternalFailure,
                    "instant has no timestamp",
                )
            })?,
        }),
        "interval" => Ok(TemporalExtent::Interval { start, end }),
        _ => Err(Error::new(
            owner,
            nous_core::ErrorCode::InternalFailure,
            "invalid temporal extent kind",
        )),
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
