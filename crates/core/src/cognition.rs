//! Cognition-owner shared lifecycle, basis, and provenance primitives.
//!
//! Domain-specific cognition types remain in their owning crates. These types
//! are shared because every cognition owner must apply the same lifecycle and
//! exact-basis semantics without depending on another domain.

// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use crate::{
    CognitiveRef, CognitiveSeedVersionId, DerivedRegionId, DerivedRepresentationId, Error,
    EvidenceHandle, OccurrenceId, Result, SourceRegionId,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SeedBasisRef {
    pub seed_version_id: CognitiveSeedVersionId,
    pub semantic_path: String,
}

impl SeedBasisRef {
    pub fn new(
        seed_version_id: CognitiveSeedVersionId,
        semantic_path: impl Into<String>,
    ) -> Result<Self> {
        let semantic_path = semantic_path.into();
        if semantic_path.is_empty()
            || semantic_path.len() > 512
            || semantic_path.chars().any(char::is_whitespace)
        {
            return Err(Error::Invalid(
                "invalid Cognitive Seed semantic path".into(),
            ));
        }
        Ok(Self {
            seed_version_id,
            semantic_path,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AcceptanceState {
    Accepted,
    Withdrawn,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IntegrityState {
    Valid,
    RevalidationRequired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SuppressionState {
    Normal,
    Suppressed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EvidenceLocator {
    WholeOccurrence,
    SourceRegion(SourceRegionId),
    DerivedRepresentation(DerivedRepresentationId),
    DerivedRegion(DerivedRegionId),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BasisRole {
    Direct,
    Interpretation,
    Contextual,
}

impl BasisRole {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Direct => "direct",
            Self::Interpretation => "interpretation",
            Self::Contextual => "contextual",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EpistemicRelation {
    Supports,
    Contradicts,
    Corroborates,
    Weakens,
    Corrects,
    Counterexample,
    InferredFrom,
}
impl EpistemicRelation {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Supports => "supports",
            Self::Contradicts => "contradicts",
            Self::Corroborates => "corroborates",
            Self::Weakens => "weakens",
            Self::Corrects => "corrects",
            Self::Counterexample => "counterexample",
            Self::InferredFrom => "inferred_from",
        }
    }
}
pub fn epistemic_relation_text(relation: Option<EpistemicRelation>) -> &'static str {
    relation.map_or("", EpistemicRelation::as_str)
}
pub fn parse_epistemic_relation(value: String) -> Result<Option<EpistemicRelation>> {
    if value.is_empty() {
        return Ok(None);
    }
    serde_json::from_value(serde_json::Value::String(value))
        .map(Some)
        .map_err(|_| Error::Infrastructure("invalid epistemic relation".into()))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvidenceRef {
    pub occurrence_id: OccurrenceId,
    pub locator: EvidenceLocator,
    pub basis_role: BasisRole,
    #[serde(default)]
    pub epistemic_relation: Option<EpistemicRelation>,
}

impl EvidenceRef {
    pub fn cognitive_ref(&self) -> CognitiveRef {
        match self.locator {
            EvidenceLocator::WholeOccurrence => CognitiveRef::Occurrence(self.occurrence_id),
            EvidenceLocator::SourceRegion(id) => CognitiveRef::SourceRegion(id),
            EvidenceLocator::DerivedRepresentation(id) => CognitiveRef::DerivedRepresentation(id),
            EvidenceLocator::DerivedRegion(id) => CognitiveRef::DerivedRegion(id),
        }
    }

    pub fn canonical_key(&self) -> String {
        let locator = match self.locator {
            EvidenceLocator::WholeOccurrence => "whole_occurrence".to_owned(),
            EvidenceLocator::SourceRegion(id) => format!("source_region:{id:?}"),
            EvidenceLocator::DerivedRepresentation(id) => format!("derived_representation:{id:?}"),
            EvidenceLocator::DerivedRegion(id) => format!("derived_region:{id:?}"),
        };
        format!(
            "{}|{}|{}|{}",
            self.occurrence_id.0,
            locator,
            self.basis_role.as_str(),
            epistemic_relation_text(self.epistemic_relation)
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceRootCertainty {
    Known,
    OccurrenceOnly,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct EvidenceRoot {
    pub root_key: String,
    pub certainty: EvidenceRootCertainty,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DependencyRelation {
    SameRoot,
    PartiallyShared,
    Independent,
    UnknownDependency,
}

pub fn dependency_relation(left: &[EvidenceRoot], right: &[EvidenceRoot]) -> DependencyRelation {
    let left_keys = left
        .iter()
        .map(|root| root.root_key.as_str())
        .collect::<BTreeSet<_>>();
    let right_keys = right
        .iter()
        .map(|root| root.root_key.as_str())
        .collect::<BTreeSet<_>>();
    let intersection = left_keys
        .intersection(&right_keys)
        .copied()
        .collect::<BTreeSet<_>>();
    if !intersection.is_empty()
        && intersection.len() == left_keys.len()
        && intersection.len() == right_keys.len()
    {
        return DependencyRelation::SameRoot;
    }
    if !intersection.is_empty() {
        return DependencyRelation::PartiallyShared;
    }
    let all_known = left
        .iter()
        .chain(right.iter())
        .all(|root| matches!(root.certainty, EvidenceRootCertainty::Known));
    if all_known && !left_keys.is_empty() && !right_keys.is_empty() {
        DependencyRelation::Independent
    } else {
        DependencyRelation::UnknownDependency
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CognitionDependency {
    pub target_revision: CognitiveRef,
    pub basis_role: BasisRole,
    #[serde(default)]
    pub epistemic_relation: Option<EpistemicRelation>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum RevisionBasis {
    Evidence(EvidenceRef),
    CognitionDependency(CognitionDependency),
    Seed(SeedBasisRef),
}

impl RevisionBasis {
    pub fn epistemic_relation(&self) -> Option<EpistemicRelation> {
        match self {
            Self::Evidence(v) => v.epistemic_relation,
            Self::CognitionDependency(v) => v.epistemic_relation,
            Self::Seed(_) => None,
        }
    }
    pub fn evidence_handle(&self) -> EvidenceHandle {
        let (reference, role) = match self {
            Self::Evidence(value) => (value.cognitive_ref(), value.basis_role.as_str()),
            Self::CognitionDependency(value) => {
                (value.target_revision.clone(), value.basis_role.as_str())
            }
            Self::Seed(value) => (
                CognitiveRef::CognitiveSeedVersion(value.seed_version_id),
                "seed",
            ),
        };
        EvidenceHandle {
            reference,
            epistemic_relation: self.epistemic_relation(),
            basis_role: role.into(),
        }
    }
    pub fn canonical_key(&self) -> String {
        match self {
            Self::Evidence(value) => format!("evidence:{}", value.canonical_key()),
            Self::CognitionDependency(value) => {
                format!(
                    "dependency:{}|{}|{}",
                    value.target_revision,
                    value.basis_role.as_str(),
                    epistemic_relation_text(value.epistemic_relation)
                )
            }
            Self::Seed(value) => {
                format!("seed:{}:{}", value.seed_version_id.0, value.semantic_path)
            }
        }
    }
}

pub fn validate_exact_basis(basis: &[RevisionBasis]) -> Result<()> {
    if basis.is_empty() {
        return Err(Error::Invalid("a cognition revision needs support".into()));
    }
    let mut keys = BTreeSet::new();
    for basis in basis {
        if !keys.insert(basis.canonical_key()) {
            return Err(Error::Invalid("duplicate revision support".into()));
        }
        if let RevisionBasis::CognitionDependency(value) = basis
            && !value.target_revision.is_cognition_revision()
        {
            return Err(Error::Invalid(
                "cognition support must target an exact revision".into(),
            ));
        }
    }
    Ok(())
}
