//! Memory-domain value objects and validation rules.
//!
//! The domain owns cognitive meaning. SQL, transport, retrieval, and provider
//! mechanics remain in their respective owners.

// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use chrono::{DateTime, Utc};
pub use nous_core::cognition::{
    AcceptanceState, BasisRole, CognitionDependency, DependencyRelation, EpistemicRelation,
    EvidenceLocator, EvidenceRef, EvidenceRoot, EvidenceRootCertainty, IntegrityState,
    RevisionBasis, SuppressionState, dependency_relation,
};
use nous_core::{
    AssociationEvidenceId, CognitiveRef, CognitiveSchemaId, CognitiveSchemaRevisionId, EntityRef,
    EpistemicClass, Error, MemoryId, MemoryRevisionId, OccurrenceId, OperationId, Result,
    SchemaEvidenceLinkId, SubjectId, TagId, TemporalExtent, UseEventRef,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CognitiveRole {
    Experiential,
    Declarative,
    ProceduralExperience,
}

impl CognitiveRole {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Experiential => "experiential",
            Self::Declarative => "declarative",
            Self::ProceduralExperience => "procedural_experience",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FormationMode {
    Grounded,
    Synthesized,
}

impl FormationMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Grounded => "grounded",
            Self::Synthesized => "synthesized",
        }
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RevisionIntent {
    Correct,
    Rephrase,
    Reinterpret,
}

impl RevisionIntent {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Correct => "correct",
            Self::Rephrase => "rephrase",
            Self::Reinterpret => "reinterpret",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryObject {
    pub memory_id: MemoryId,
    pub subject_id: SubjectId,
    pub cognitive_role: CognitiveRole,
    pub current_revision_id: MemoryRevisionId,
    pub object_epoch: i64,
    pub acceptance_state: AcceptanceState,
    pub integrity_state: IntegrityState,
    pub suppression_state: SuppressionState,
    pub accessibility_mode: AccessibilityMode,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryRevision {
    pub producer_signature_id: Option<Uuid>,
    pub memory_revision_id: MemoryRevisionId,
    pub memory_id: MemoryId,
    pub subject_id: SubjectId,
    pub revision_no: i32,
    pub parent_revision_id: Option<MemoryRevisionId>,
    pub revision_intent: Option<RevisionIntent>,
    pub formation_mode: FormationMode,
    pub grounding_occurrence_id: Option<OccurrenceId>,
    pub semantic_role: String,
    pub title: Option<String>,
    pub representation_text: String,
    pub epistemic_class: EpistemicClass,
    pub valid_time: TemporalExtent,
    #[serde(
        default = "nous_core::real_time_domain",
        skip_serializing_if = "nous_core::is_real_time_domain"
    )]
    pub valid_time_domain_ref: String,
    pub formed_at: DateTime<Utc>,
    pub formed_time_domain_ref: String,
    pub formed_recorded_at: DateTime<Utc>,
    pub recorded_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryRelation {
    DerivedFrom,
    Contradicts,
    TemporalSuccessor,
    ReplacesBasis,
    Elaborates,
}

impl MemoryRelation {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::DerivedFrom => "derived_from",
            Self::Contradicts => "contradicts",
            Self::TemporalSuccessor => "temporal_successor",
            Self::ReplacesBasis => "replaces_basis",
            Self::Elaborates => "elaborates",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryRevisionRelation {
    pub from_revision_id: MemoryRevisionId,
    pub to_revision_id: MemoryRevisionId,
    pub relation: MemoryRelation,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssociationPolarity {
    Positive,
    Negative,
}

impl AssociationPolarity {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Positive => "positive",
            Self::Negative => "negative",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssociationBasisClass {
    HostExplicit,
    SourceEvidence,
    CognitiveDerivation,
    MeaningfulUse,
    DerivedStructure,
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
        match self {
            Self::HostExplicit => "host_explicit",
            Self::SourceEvidence => "source_evidence",
            Self::CognitiveDerivation => "cognitive_derivation",
            Self::MeaningfulUse => "meaningful_use",
            Self::DerivedStructure => "derived_structure",
        }
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
    #[serde(
        default = "nous_core::real_time_domain",
        skip_serializing_if = "nous_core::is_real_time_domain"
    )]
    pub valid_time_domain_ref: String,
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
pub struct MemoryContent {
    pub producer: Option<nous_core::ProducerSignature>,
    pub formation_mode: FormationMode,
    pub grounding_occurrence_id: Option<OccurrenceId>,
    pub semantic_role: String,
    pub representation_text: String,
    pub title: Option<String>,
    pub basis: Vec<RevisionBasis>,
    pub aboutness: Vec<EntityRef>,
    pub tags: Vec<TagId>,
    pub valid_time: TemporalExtent,
    #[serde(
        default = "nous_core::real_time_domain",
        skip_serializing_if = "nous_core::is_real_time_domain"
    )]
    pub valid_time_domain_ref: String,
    pub epistemic_class: EpistemicClass,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExplicitMemoryInput {
    pub operation_id: OperationId,
    pub subject: SubjectId,
    pub cognitive_role: CognitiveRole,
    pub content: MemoryContent,
}

impl ExplicitMemoryInput {
    pub fn validate(&self) -> Result<()> {
        if self.operation_id.0.is_nil() {
            return Err(Error::Invalid("operation_id is required".into()));
        }
        self.content.validate()
    }
}

impl MemoryContent {
    pub fn validate(&self) -> Result<()> {
        validate_content(&self.semantic_role, &self.representation_text)?;
        nous_core::DomainTemporalExtent {
            time_domain_ref: self.valid_time_domain_ref.clone(),
            extent: self.valid_time.clone(),
        }
        .validate()?;
        if self.basis.is_empty() {
            return Err(Error::Invalid("a Memory revision needs support".into()));
        }
        validate_basis(
            self.formation_mode,
            self.grounding_occurrence_id,
            &self.basis,
        )?;
        let mut keys = std::collections::BTreeSet::new();
        for basis in &self.basis {
            if !keys.insert(basis.canonical_key()) {
                return Err(Error::Invalid("duplicate revision support".into()));
            }
        }
        if self.tags.len() > 128 {
            return Err(Error::Invalid("explicit Tag bound exceeded".into()));
        }
        let mut aboutness = std::collections::BTreeSet::new();
        for entity in &self.aboutness {
            EntityRef::new(entity.as_str())?;
            aboutness.insert(entity.as_str());
        }
        if aboutness.len() != self.aboutness.len() {
            return Err(Error::Invalid("duplicate aboutness entity".into()));
        }
        Ok(())
    }
}

pub fn validate_content(semantic_role: &str, representation_text: &str) -> Result<()> {
    if semantic_role.trim().is_empty() || semantic_role.len() > 128 {
        return Err(Error::Invalid(
            "semantic_role is required and bounded".into(),
        ));
    }
    if representation_text.trim().is_empty() || representation_text.len() > 1_000_000 {
        return Err(Error::Invalid(
            "representation_text is required and bounded".into(),
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
                Error::Invalid("grounded formation requires grounding_occurrence_id".into())
            })?;
            if !basis.iter().any(|basis| {
                matches!(basis, RevisionBasis::Evidence(evidence) if evidence.occurrence_id == occurrence)
            }) {
                return Err(Error::Invalid(
                    "grounding occurrence must be in the evidence support set".into(),
                ));
            }
        }
        FormationMode::Synthesized => {
            if grounding_occurrence.is_some() {
                return Err(Error::Invalid(
                    "synthesized formation cannot have grounding_occurrence_id".into(),
                ));
            }
            if basis.len() < 2 {
                return Err(Error::Invalid(
                    "synthesized formation needs at least two distinct inputs".into(),
                ));
            }
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchemaScope {
    pub description: String,
    #[serde(default)]
    pub aboutness: Vec<EntityRef>,
    #[serde(default)]
    pub tags: Vec<TagId>,
    pub valid_time: TemporalExtent,
    #[serde(
        default = "nous_core::real_time_domain",
        skip_serializing_if = "nous_core::is_real_time_domain"
    )]
    pub valid_time_domain_ref: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CognitiveSchema {
    pub schema_id: CognitiveSchemaId,
    pub subject_id: SubjectId,
    pub current_revision_id: CognitiveSchemaRevisionId,
    pub object_epoch: i64,
    pub acceptance_state: AcceptanceState,
    pub integrity_state: IntegrityState,
    pub suppression_state: SuppressionState,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CognitiveSchemaRevision {
    pub schema_revision_id: CognitiveSchemaRevisionId,
    pub schema_id: CognitiveSchemaId,
    pub revision_no: i32,
    pub parent_revision_id: Option<CognitiveSchemaRevisionId>,
    pub revision_intent: Option<RevisionIntent>,
    pub title: Option<String>,
    pub structural_claim: String,
    pub applicability_scope: SchemaScope,
    pub boundary_definition: String,
    pub formation_kind: SchemaFormationKind,
    pub formed_at: DateTime<Utc>,
    pub formed_time_domain_ref: String,
    pub formed_recorded_at: DateTime<Utc>,
    pub recorded_at: DateTime<Utc>,
    pub producer_signature_id: Option<Uuid>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SchemaFormationKind {
    #[default]
    ExplicitImport,
    Synthesized,
}

impl SchemaFormationKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ExplicitImport => "explicit_import",
            Self::Synthesized => "synthesized",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SchemaEvidenceRole {
    Support,
    Counterexample,
    BoundaryCase,
}

impl SchemaEvidenceRole {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Support => "support",
            Self::Counterexample => "counterexample",
            Self::BoundaryCase => "boundary_case",
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn evidence(occurrence: OccurrenceId) -> RevisionBasis {
        RevisionBasis::Evidence(EvidenceRef {
            epistemic_relation: None,
            occurrence_id: occurrence,
            locator: EvidenceLocator::WholeOccurrence,
            basis_role: BasisRole::Direct,
        })
    }

    #[test]
    fn grounded_formation_binds_its_declared_occurrence() {
        let occurrence = OccurrenceId::new();
        let mut input = ExplicitMemoryInput {
            operation_id: OperationId::new(),
            subject: SubjectId::new(),
            cognitive_role: CognitiveRole::Declarative,

            content: crate::MemoryContent {
                valid_time_domain_ref: "time:real".into(),
                producer: None,
                formation_mode: FormationMode::Grounded,
                grounding_occurrence_id: Some(occurrence),
                semantic_role: "fact".into(),
                representation_text: "bounded fact".into(),
                title: None,
                basis: vec![evidence(occurrence)],
                aboutness: Vec::new(),
                tags: Vec::new(),
                valid_time: TemporalExtent::Unknown,

                epistemic_class: EpistemicClass::Reported,
            },
        };
        assert!(input.validate().is_ok());
        input.content.grounding_occurrence_id = Some(OccurrenceId::new());
        assert!(input.validate().is_err());
    }

    #[test]
    fn synthesized_requires_two_inputs() {
        let input = ExplicitMemoryInput {
            operation_id: OperationId::new(),
            subject: SubjectId::new(),
            cognitive_role: CognitiveRole::Declarative,

            content: crate::MemoryContent {
                valid_time_domain_ref: "time:real".into(),
                producer: None,
                formation_mode: FormationMode::Synthesized,
                grounding_occurrence_id: None,
                semantic_role: "fact".into(),
                representation_text: "bounded fact".into(),
                title: None,
                basis: vec![evidence(OccurrenceId::new())],
                aboutness: Vec::new(),
                tags: Vec::new(),
                valid_time: TemporalExtent::Unknown,

                epistemic_class: EpistemicClass::Inferred,
            },
        };
        assert!(input.validate().is_err());
    }

    #[test]
    fn provenance_relation_is_conservative_for_unknown_encounters() {
        let known = EvidenceRoot {
            root_key: "artifact:a".into(),
            certainty: EvidenceRootCertainty::Known,
        };
        let same = known.clone();
        assert_eq!(
            dependency_relation(std::slice::from_ref(&known), std::slice::from_ref(&same)),
            DependencyRelation::SameRoot
        );
        assert_eq!(
            dependency_relation(
                std::slice::from_ref(&known),
                &[EvidenceRoot {
                    root_key: "artifact:b".into(),
                    certainty: EvidenceRootCertainty::Known,
                }]
            ),
            DependencyRelation::Independent
        );
        assert_eq!(
            dependency_relation(
                &[EvidenceRoot {
                    root_key: "occurrence:a".into(),
                    certainty: EvidenceRootCertainty::OccurrenceOnly,
                }],
                &[EvidenceRoot {
                    root_key: "occurrence:b".into(),
                    certainty: EvidenceRootCertainty::OccurrenceOnly,
                }]
            ),
            DependencyRelation::UnknownDependency
        );
    }
}
