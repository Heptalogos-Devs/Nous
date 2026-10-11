//! Memory-domain value objects and validation rules.
//!
//! The domain owns cognitive meaning. SQL, transport, retrieval, and provider
//! mechanics remain in their respective owners.

// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use chrono::{DateTime, Utc};
pub use nous_core::cognition::{
    AcceptanceState, BasisRole, CognitionDependency, DependencyRelation, EpistemicRelation,
    EvidenceLocator, EvidenceRef, EvidenceRoot, EvidenceRootCertainty, IntegrityState, PurgeState,
    RevisionBasis, SuppressionState, dependency_relation,
};
use nous_core::{
    AssociationEvidenceId, CognitiveRef, CognitiveSchemaId, CognitiveSchemaRevisionId, EntityRef,
    EpisodeId, EpisodeRevisionId, EpistemicClass, Error, ErrorCode, ErrorOwner, JournalId,
    JournalRevisionId, MemoryId, MemoryRevisionId, OccurrenceId, OperationId, Result,
    SchemaEvidenceLinkId, SubjectId, TagId, TemporalExtent, UseEventId,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// The four revisioned cognition families that share Memory Authority state.
/// Tags and Material are deliberately outside this finite owner model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CognitionKind {
    Memory,
    CognitiveSchema,
    Episode,
    Journal,
}

impl CognitionKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Memory => "memory",
            Self::CognitiveSchema => "cognitive_schema",
            Self::Episode => "episode",
            Self::Journal => "journal",
        }
    }

    pub fn object_ref(self, id: Uuid) -> CognitiveRef {
        match self {
            Self::Memory => CognitiveRef::Memory(MemoryId(id)),
            Self::CognitiveSchema => CognitiveRef::CognitiveSchema(CognitiveSchemaId(id)),
            Self::Episode => CognitiveRef::Episode(EpisodeId(id)),
            Self::Journal => CognitiveRef::Journal(JournalId(id)),
        }
    }

    pub fn revision_ref(self, id: Uuid) -> CognitiveRef {
        match self {
            Self::Memory => CognitiveRef::MemoryRevision(MemoryRevisionId(id)),
            Self::CognitiveSchema => {
                CognitiveRef::CognitiveSchemaRevision(CognitiveSchemaRevisionId(id))
            }
            Self::Episode => CognitiveRef::EpisodeRevision(EpisodeRevisionId(id)),
            Self::Journal => CognitiveRef::JournalRevision(JournalRevisionId(id)),
        }
    }

    pub fn from_reference(reference: &CognitiveRef) -> Result<(Self, Uuid, bool)> {
        Ok(match reference {
            CognitiveRef::Memory(id) => (Self::Memory, id.0, false),
            CognitiveRef::MemoryRevision(id) => (Self::Memory, id.0, true),
            CognitiveRef::CognitiveSchema(id) => (Self::CognitiveSchema, id.0, false),
            CognitiveRef::CognitiveSchemaRevision(id) => (Self::CognitiveSchema, id.0, true),
            CognitiveRef::Episode(id) => (Self::Episode, id.0, false),
            CognitiveRef::EpisodeRevision(id) => (Self::Episode, id.0, true),
            CognitiveRef::Journal(id) => (Self::Journal, id.0, false),
            CognitiveRef::JournalRevision(id) => (Self::Journal, id.0, true),
            _ => {
                return Err(Error::new(
                    ErrorOwner::Memory,
                    ErrorCode::InvalidRequest,
                    "reference is outside Memory Cognition Authority",
                ));
            }
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CognitionLifecycle {
    pub acceptance: AcceptanceState,
    pub integrity: IntegrityState,
    pub suppression: SuppressionState,
    pub purge: PurgeState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CognitionDetails {
    Memory {
        role: CognitiveRole,
        accessibility: AccessibilityMode,
    },
    CognitiveSchema,
    Episode {
        track_key: String,
    },
    Journal,
}

impl CognitionDetails {
    pub const fn kind(&self) -> CognitionKind {
        match self {
            Self::Memory { .. } => CognitionKind::Memory,
            Self::CognitiveSchema => CognitionKind::CognitiveSchema,
            Self::Episode { .. } => CognitionKind::Episode,
            Self::Journal => CognitionKind::Journal,
        }
    }

    pub fn memory_role(&self) -> CognitiveRole {
        match self {
            Self::Memory { role, .. } => *role,
            _ => unreachable!("Memory role requested from another Cognition kind"),
        }
    }

    pub fn accessibility_mode(&self) -> AccessibilityMode {
        match self {
            Self::Memory { accessibility, .. } => *accessibility,
            _ => unreachable!("Memory accessibility requested from another Cognition kind"),
        }
    }

    pub fn track_key(&self) -> &str {
        match self {
            Self::Episode { track_key } => track_key,
            _ => unreachable!("Episode track requested from another Cognition kind"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CognitionHeader {
    pub object_ref: CognitiveRef,
    pub subject_id: SubjectId,
    pub current_revision_ref: CognitiveRef,
    pub object_epoch: i64,
    pub lifecycle: CognitionLifecycle,
    pub created_at: DateTime<Utc>,
    pub details: CognitionDetails,
}

impl CognitionHeader {
    pub fn new(
        subject_id: SubjectId,
        object_id: Uuid,
        current_revision_id: Uuid,
        object_epoch: i64,
        lifecycle: CognitionLifecycle,
        created_at: DateTime<Utc>,
        details: CognitionDetails,
    ) -> Self {
        let kind = details.kind();
        Self {
            object_ref: kind.object_ref(object_id),
            subject_id,
            current_revision_ref: kind.revision_ref(current_revision_id),
            object_epoch,
            lifecycle,
            created_at,
            details,
        }
    }

    pub fn kind(&self) -> CognitionKind {
        self.details.kind()
    }

    pub fn object_id(&self) -> Uuid {
        match &self.object_ref {
            CognitiveRef::Memory(id) => id.0,
            CognitiveRef::CognitiveSchema(id) => id.0,
            CognitiveRef::Episode(id) => id.0,
            CognitiveRef::Journal(id) => id.0,
            _ => unreachable!("Cognition header has a non-object reference"),
        }
    }

    pub fn memory_id(&self) -> MemoryId {
        match &self.object_ref {
            CognitiveRef::Memory(id) => *id,
            _ => unreachable!("not a Memory header"),
        }
    }
    pub fn schema_id(&self) -> CognitiveSchemaId {
        match &self.object_ref {
            CognitiveRef::CognitiveSchema(id) => *id,
            _ => unreachable!("not a schema header"),
        }
    }
    pub fn episode_id(&self) -> EpisodeId {
        match &self.object_ref {
            CognitiveRef::Episode(id) => *id,
            _ => unreachable!("not an Episode header"),
        }
    }
    pub fn journal_id(&self) -> JournalId {
        match &self.object_ref {
            CognitiveRef::Journal(id) => *id,
            _ => unreachable!("not a Journal header"),
        }
    }

    pub fn current_revision_id(&self) -> Uuid {
        match &self.current_revision_ref {
            CognitiveRef::MemoryRevision(id) => id.0,
            CognitiveRef::CognitiveSchemaRevision(id) => id.0,
            CognitiveRef::EpisodeRevision(id) => id.0,
            CognitiveRef::JournalRevision(id) => id.0,
            _ => unreachable!("Cognition header has a non-revision head"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CognitionRevisionHeader {
    pub revision_ref: CognitiveRef,
    pub object_ref: CognitiveRef,
    pub revision_no: i32,
    pub parent_revision_ref: Option<CognitiveRef>,
    pub formed_at: DateTime<Utc>,
    pub recorded_at: DateTime<Utc>,
    pub producer_signature_id: Option<Uuid>,
}

impl CognitionRevisionHeader {
    pub fn new(
        kind: CognitionKind,
        object_id: Uuid,
        revision_id: Uuid,
        revision_no: i32,
        parent_revision_id: Option<Uuid>,
        formed_at: DateTime<Utc>,
        recorded_at: DateTime<Utc>,
        producer_signature_id: Option<Uuid>,
    ) -> Self {
        Self {
            revision_ref: kind.revision_ref(revision_id),
            object_ref: kind.object_ref(object_id),
            revision_no,
            parent_revision_ref: parent_revision_id.map(|id| kind.revision_ref(id)),
            formed_at,
            recorded_at,
            producer_signature_id,
        }
    }

    pub fn revision_id(&self) -> Uuid {
        match &self.revision_ref {
            CognitiveRef::MemoryRevision(id) => id.0,
            CognitiveRef::CognitiveSchemaRevision(id) => id.0,
            CognitiveRef::EpisodeRevision(id) => id.0,
            CognitiveRef::JournalRevision(id) => id.0,
            _ => unreachable!("Cognition revision header has a non-revision reference"),
        }
    }

    pub fn object_id(&self) -> Uuid {
        match &self.object_ref {
            CognitiveRef::Memory(id) => id.0,
            CognitiveRef::CognitiveSchema(id) => id.0,
            CognitiveRef::Episode(id) => id.0,
            CognitiveRef::Journal(id) => id.0,
            _ => unreachable!("Cognition revision header has a non-object reference"),
        }
    }

    pub fn memory_revision_id(&self) -> MemoryRevisionId {
        match &self.revision_ref {
            CognitiveRef::MemoryRevision(id) => *id,
            _ => unreachable!("not a Memory revision"),
        }
    }
    pub fn schema_revision_id(&self) -> CognitiveSchemaRevisionId {
        match &self.revision_ref {
            CognitiveRef::CognitiveSchemaRevision(id) => *id,
            _ => unreachable!("not a schema revision"),
        }
    }
    pub fn episode_revision_id(&self) -> EpisodeRevisionId {
        match &self.revision_ref {
            CognitiveRef::EpisodeRevision(id) => *id,
            _ => unreachable!("not an Episode revision"),
        }
    }
    pub fn journal_revision_id(&self) -> JournalRevisionId {
        match &self.revision_ref {
            CognitiveRef::JournalRevision(id) => *id,
            _ => unreachable!("not a Journal revision"),
        }
    }

    pub fn parent_revision_id(&self) -> Option<Uuid> {
        self.parent_revision_ref
            .as_ref()
            .map(|reference| match reference {
                CognitiveRef::MemoryRevision(id) => id.0,
                CognitiveRef::CognitiveSchemaRevision(id) => id.0,
                CognitiveRef::EpisodeRevision(id) => id.0,
                CognitiveRef::JournalRevision(id) => id.0,
                _ => unreachable!("Cognition parent header has a non-revision reference"),
            })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CognitionAction {
    Suppress,
    Restore,
    Withdraw,
    Reaccept,
    Purge,
    SetAccessibility(AccessibilityMode),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, strum::IntoStaticStr)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum CognitiveRole {
    Experiential,
    Declarative,
    ProceduralExperience,
}

impl CognitiveRole {
    pub fn as_str(self) -> &'static str {
        self.into()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, strum::IntoStaticStr)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum FormationMode {
    Grounded,
    Synthesized,
}

impl FormationMode {
    pub fn as_str(self) -> &'static str {
        self.into()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AccessibilityMode {
    #[default]
    Auto,
    Normal,
    Deep,
    Explicit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccessibilityLevel {
    Normal,
    Deep,
    Explicit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, strum::IntoStaticStr)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum RevisionIntent {
    Correct,
    Rephrase,
    Reinterpret,
}

impl RevisionIntent {
    pub fn as_str(self) -> &'static str {
        self.into()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryRevision {
    pub header: CognitionRevisionHeader,
    pub revision_intent: Option<RevisionIntent>,
    pub formation_mode: FormationMode,
    pub grounding_occurrence_id: Option<OccurrenceId>,
    pub semantic_role: String,
    pub title: Option<String>,
    pub representation_text: String,
    pub epistemic_class: EpistemicClass,
    pub valid_time: TemporalExtent,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryRevisionEvidence {
    pub evidence_no: i32,
    pub evidence: EvidenceRef,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, strum::IntoStaticStr)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum MemoryRelation {
    DerivedFrom,
    Contradicts,
    TemporalSuccessor,
    ReplacesBasis,
    Elaborates,
}

impl MemoryRelation {
    pub fn as_str(self) -> &'static str {
        self.into()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryRevisionRelation {
    pub from_revision_id: MemoryRevisionId,
    pub to_revision_id: MemoryRevisionId,
    pub relation: MemoryRelation,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tag {
    pub tag_id: TagId,
    pub subject_id: SubjectId,
    pub current_revision_id: Uuid,
    pub status: String,
    pub canonical_tag_id: Option<TagId>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TagRevision {
    pub tag_revision_id: Uuid,
    pub tag_id: TagId,
    pub revision_no: i32,
    pub label: String,
    pub description: Option<String>,
    pub kind_hint: Option<String>,
    pub origin: String,
    pub producer_signature_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, strum::IntoStaticStr)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum AssociationPolarity {
    Positive,
    Negative,
}

impl AssociationPolarity {
    pub fn as_str(self) -> &'static str {
        self.into()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, strum::IntoStaticStr)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum AssociationBasisClass {
    HostExplicit,
    SourceEvidence,
    CognitiveDerivation,
    MeaningfulUse,
    DerivedStructure,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UseEventRef {
    pub subject_id: SubjectId,
    pub consumer_ref: String,
    pub event_id: UseEventId,
}

impl UseEventRef {
    pub fn canonical_key(&self) -> String {
        format!(
            "{}:{}:{}",
            self.subject_id.0, self.consumer_ref, self.event_id.0
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AssociationBasis {
    Revision(RevisionBasis),
    UseEvent(UseEventRef),
}

impl AssociationBasis {
    pub fn epistemic_relation(&self) -> Option<EpistemicRelation> {
        match self {
            Self::Revision(v) => v.epistemic_relation(),
            Self::UseEvent(_) => None,
        }
    }
}

impl AssociationBasisClass {
    pub fn as_str(self) -> &'static str {
        self.into()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssociationEvidence {
    pub association_evidence_id: AssociationEvidenceId,
    pub subject_id: SubjectId,
    pub from: CognitiveRef,
    pub to: CognitiveRef,
    pub relation_kind: String,
    pub polarity: AssociationPolarity,
    pub basis_class: AssociationBasisClass,
    pub basis: Vec<AssociationBasis>,
    pub producer_signature_id: Option<Uuid>,
    pub valid_time: TemporalExtent,
    pub created_at: DateTime<Utc>,
    pub revoked_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone)]
pub struct AssociationNeighborhood {
    pub nodes: Vec<CognitiveRef>,
    pub associations: Vec<AssociationEvidence>,
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExplicitMemoryInput {
    #[serde(default)]
    pub producer: Option<nous_core::ProducerSignature>,
    pub operation_id: OperationId,
    #[serde(default)]
    pub subject: SubjectId,
    pub cognitive_role: CognitiveRole,
    pub formation_mode: FormationMode,
    pub grounding_occurrence_id: Option<OccurrenceId>,
    pub semantic_role: String,
    pub representation_text: String,
    pub title: Option<String>,
    #[serde(default)]
    pub basis: Vec<RevisionBasis>,
    #[serde(default)]
    pub aboutness: Vec<EntityRef>,
    #[serde(default)]
    pub tags: Vec<TagId>,
    pub valid_time: TemporalExtent,

    pub epistemic_class: EpistemicClass,
}

impl ExplicitMemoryInput {
    pub fn validate(&self) -> Result<()> {
        if self.operation_id.0.is_nil() {
            return Err(Error::new(
                ErrorOwner::Memory,
                ErrorCode::InvalidRequest,
                "operation_id is required",
            ));
        }
        validate_content(&self.semantic_role, &self.representation_text)?;
        self.valid_time.validate()?;
        if self.basis.is_empty() {
            return Err(Error::new(
                ErrorOwner::Memory,
                ErrorCode::InvalidRequest,
                "a Memory revision needs support",
            ));
        }
        validate_basis(
            self.formation_mode,
            self.grounding_occurrence_id,
            &self.basis,
        )?;
        let mut keys = std::collections::BTreeSet::new();
        for basis in &self.basis {
            if !keys.insert(basis.canonical_key()) {
                return Err(Error::new(
                    ErrorOwner::Memory,
                    ErrorCode::InvalidRequest,
                    "duplicate revision support",
                ));
            }
        }
        if self.tags.len() > 128 {
            return Err(Error::new(
                ErrorOwner::Memory,
                ErrorCode::InvalidRequest,
                "explicit Tag bound exceeded",
            ));
        }
        let mut aboutness = std::collections::BTreeSet::new();
        for entity in &self.aboutness {
            EntityRef::new(entity.as_str())?;
            aboutness.insert(entity.as_str());
        }
        if aboutness.len() != self.aboutness.len() {
            return Err(Error::new(
                ErrorOwner::Memory,
                ErrorCode::InvalidRequest,
                "duplicate aboutness entity",
            ));
        }
        Ok(())
    }
}

pub fn validate_content(semantic_role: &str, representation_text: &str) -> Result<()> {
    if semantic_role.trim().is_empty() || semantic_role.len() > 128 {
        return Err(Error::new(
            ErrorOwner::Memory,
            ErrorCode::InvalidRequest,
            "semantic_role is required and bounded",
        ));
    }
    if representation_text.trim().is_empty() || representation_text.len() > 1_000_000 {
        return Err(Error::new(
            ErrorOwner::Memory,
            ErrorCode::InvalidRequest,
            "representation_text is required and bounded",
        ));
    }
    Ok(())
}

fn validate_basis(
    mode: FormationMode,
    grounding_occurrence: Option<OccurrenceId>,
    basis: &[RevisionBasis],
) -> Result<()> {
    match mode {
        FormationMode::Grounded => {
            let occurrence = grounding_occurrence.ok_or_else(|| {
                Error::new(
                    ErrorOwner::Memory,
                    ErrorCode::InvalidRequest,
                    "grounded formation requires grounding_occurrence_id",
                )
            })?;
            if !basis.iter().any(|basis| {
                matches!(basis, RevisionBasis::Evidence(evidence) if evidence.occurrence_id == occurrence)
            }) {
                return Err(Error::new(ErrorOwner::Memory, ErrorCode::InvalidRequest, "grounding occurrence must be in the evidence support set"));
            }
        }
        FormationMode::Synthesized => {
            if grounding_occurrence.is_some() {
                return Err(Error::new(
                    ErrorOwner::Memory,
                    ErrorCode::InvalidRequest,
                    "synthesized formation cannot have grounding_occurrence_id",
                ));
            }
            if basis.len() < 2 {
                return Err(Error::new(
                    ErrorOwner::Memory,
                    ErrorCode::InvalidRequest,
                    "synthesized formation needs at least two distinct inputs",
                ));
            }
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryFormationProposal {
    #[serde(default)]
    pub producer: Option<nous_core::ProducerSignature>,
    pub operation_id: OperationId,
    pub cognitive_role: CognitiveRole,
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

impl MemoryFormationProposal {
    pub fn into_input(self, subject: SubjectId) -> ExplicitMemoryInput {
        ExplicitMemoryInput {
            producer: self.producer,
            operation_id: self.operation_id,
            subject,
            cognitive_role: self.cognitive_role,
            formation_mode: self.formation_mode,
            grounding_occurrence_id: self.grounding_occurrence_id,
            semantic_role: self.semantic_role,
            representation_text: self.representation_text,
            title: self.title,
            basis: self.basis,
            aboutness: self.aboutness,
            tags: Vec::new(),
            valid_time: self.valid_time,

            epistemic_class: self.epistemic_class,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TagProposal {
    Existing {
        tag_id: TagId,
    },
    New {
        label: String,
        description: Option<String>,
        kind_hint: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchemaScope {
    pub description: String,
    #[serde(default)]
    pub aboutness: Vec<EntityRef>,
    #[serde(default)]
    pub tags: Vec<TagId>,
    pub valid_time: TemporalExtent,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CognitiveSchemaRevision {
    pub header: CognitionRevisionHeader,
    pub revision_intent: Option<RevisionIntent>,
    pub title: Option<String>,
    pub structural_claim: String,
    pub applicability_scope: SchemaScope,
    pub boundary_definition: String,
    pub formation_kind: SchemaFormationKind,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default, strum::IntoStaticStr,
)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum SchemaFormationKind {
    #[default]
    ExplicitImport,
    Synthesized,
}

impl SchemaFormationKind {
    pub fn as_str(self) -> &'static str {
        self.into()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, strum::IntoStaticStr)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum SchemaEvidenceRole {
    Support,
    Counterexample,
    BoundaryCase,
}

impl SchemaEvidenceRole {
    pub fn as_str(self) -> &'static str {
        self.into()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchemaEvidenceLink {
    pub link_id: SchemaEvidenceLinkId,
    pub subject_id: SubjectId,
    pub schema_revision_id: CognitiveSchemaRevisionId,
    pub role: SchemaEvidenceRole,
    pub basis: RevisionBasis,
    pub producer_signature_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub revoked_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchemaLineage {
    pub from_revision_id: CognitiveSchemaRevisionId,
    pub to_revision_id: CognitiveSchemaRevisionId,
    pub relation: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchemaContent {
    pub producer: Option<nous_core::ProducerSignature>,
    pub title: Option<String>,
    pub structural_claim: String,
    pub applicability_scope: SchemaScope,
    pub boundary_definition: String,
    #[serde(default)]
    pub formation_kind: SchemaFormationKind,
    pub evidence_links: Vec<SchemaEvidenceLinkInput>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateSchemaInput {
    pub operation_id: OperationId,
    pub subject: SubjectId,
    #[serde(flatten)]
    pub content: SchemaContent,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchemaEvidenceLinkInput {
    pub role: SchemaEvidenceRole,
    pub basis: RevisionBasis,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviseSchemaInput {
    pub operation_id: OperationId,
    pub subject: SubjectId,
    pub schema_id: CognitiveSchemaId,
    pub expected_object_epoch: i64,
    pub intent: RevisionIntent,
    #[serde(flatten)]
    pub content: SchemaContent,
    pub copy_link_ids: Vec<SchemaEvidenceLinkId>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateAssociationInput {
    pub operation_id: OperationId,
    pub subject: SubjectId,
    pub from: CognitiveRef,
    pub to: CognitiveRef,
    pub relation_kind: String,
    pub polarity: AssociationPolarity,
    pub basis_class: AssociationBasisClass,
    pub basis: Vec<AssociationBasis>,
    pub valid_time: TemporalExtent,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TemporalEvidence {
    pub occurred: Vec<TemporalExtent>,
    pub observed_at: Option<DateTime<Utc>>,
}

#[cfg(test)]
#[path = "../tests/unit/domain.rs"]
mod tests;
