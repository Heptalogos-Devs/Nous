// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
use chrono::{DateTime, Utc};
use nous_core::*;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Structural conditions are conjunctive. Entity sets are semantic sets, never identity guesses.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SocialScope {
    pub description: String,
    #[serde(default)]
    pub entities: Vec<EntityRef>,
    pub group: Option<EntityRef>,
    pub channel: Option<String>,
    pub conversation: Option<String>,
    pub work_context: Option<WorkContextId>,
    pub domain: Option<String>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RelationType {
    Familiarity,
    Affinity,
    Trust,
    Closeness,
    Collaboration,
    MentorOf,
    MemberOf,
    ConflictWith,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum RelationDegree {
    Ordinal(String),
    Affinity(f64),
    State(String),
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SocialJudgment {
    pub state: String,
    pub explanation: String,
    pub estimate: Option<f64>,
    pub estimate_method: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelationContent {
    pub from: EntityRef,
    pub to: EntityRef,
    pub relation_type: RelationType,
    pub degree: Option<RelationDegree>,
    pub role: Option<String>,
    /// Only collaboration/conflict may describe an explicit shared matter.
    #[serde(default)]
    pub symmetric: bool,
    pub scope: SocialScope,
    pub statement: String,
    pub judgment: SocialJudgment,
    #[serde(default)]
    pub coordination_expectations: Vec<String>,
    pub basis: Vec<RevisionBasis>,
    pub valid_time: TemporalExtent,
    #[serde(
        default = "nous_core::real_time_domain",
        skip_serializing_if = "nous_core::is_real_time_domain"
    )]
    pub valid_time_domain_ref: String,
    pub producer: Option<ProducerSignature>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConventionContent {
    pub expression: String,
    #[serde(default)]
    pub variants: Vec<String>,
    #[serde(default)]
    pub case_insensitive: bool,
    pub meaning: String,
    pub referent: Option<EntityRef>,
    pub scope: SocialScope,
    pub pragmatic_use: String,
    pub judgment: SocialJudgment,
    pub basis: Vec<RevisionBasis>,
    pub valid_time: TemporalExtent,
    #[serde(
        default = "nous_core::real_time_domain",
        skip_serializing_if = "nous_core::is_real_time_domain"
    )]
    pub valid_time_domain_ref: String,
    pub producer: Option<ProducerSignature>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "family", content = "value", rename_all = "snake_case")]
pub enum SocialContent {
    SocialRelation(RelationContent),
    LanguageConvention(ConventionContent),
}
impl SocialContent {
    pub fn family(&self) -> &'static str {
        match self {
            Self::SocialRelation(_) => "social_relation",
            Self::LanguageConvention(_) => "language_convention",
        }
    }
    pub fn basis(&self) -> &[RevisionBasis] {
        match self {
            Self::SocialRelation(v) => &v.basis,
            Self::LanguageConvention(v) => &v.basis,
        }
    }
    pub fn valid_time_domain_ref(&self) -> &str {
        match self {
            Self::SocialRelation(value) => &value.valid_time_domain_ref,
            Self::LanguageConvention(value) => &value.valid_time_domain_ref,
        }
    }
    pub fn valid_time(&self) -> &TemporalExtent {
        match self {
            Self::SocialRelation(v) => &v.valid_time,
            Self::LanguageConvention(v) => &v.valid_time,
        }
    }
    pub fn scope(&self) -> &SocialScope {
        match self {
            Self::SocialRelation(v) => &v.scope,
            Self::LanguageConvention(v) => &v.scope,
        }
    }
    pub fn scope_mut(&mut self) -> &mut SocialScope {
        match self {
            Self::SocialRelation(v) => &mut v.scope,
            Self::LanguageConvention(v) => &mut v.scope,
        }
    }
    pub fn judgment(&self) -> &SocialJudgment {
        match self {
            Self::SocialRelation(v) => &v.judgment,
            Self::LanguageConvention(v) => &v.judgment,
        }
    }
    pub fn producer(&self) -> Option<&ProducerSignature> {
        match self {
            Self::SocialRelation(v) => v.producer.as_ref(),
            Self::LanguageConvention(v) => v.producer.as_ref(),
        }
    }
    pub fn producer_mut(&mut self) -> &mut Option<ProducerSignature> {
        match self {
            Self::SocialRelation(v) => &mut v.producer,
            Self::LanguageConvention(v) => &mut v.producer,
        }
    }
    pub fn label(&self) -> &str {
        match self {
            Self::SocialRelation(v) => &v.statement,
            Self::LanguageConvention(v) => &v.expression,
        }
    }
    pub fn entities(&self) -> Vec<EntityRef> {
        let mut e = self.scope().entities.clone();
        if let Some(g) = &self.scope().group {
            e.push(g.clone());
        }
        match self {
            Self::SocialRelation(v) => {
                e.push(v.from.clone());
                e.push(v.to.clone());
            }
            Self::LanguageConvention(v) => {
                if let Some(r) = &v.referent {
                    e.push(r.clone());
                }
            }
        };
        e.sort_by(|a, b| a.as_str().cmp(b.as_str()));
        e.dedup();
        e
    }
    pub fn text(&self) -> String {
        serde_json::to_string_pretty(self).expect("Social content serializable")
    }
    /// Object identity cannot be revised into a different social matter.
    pub fn identity(&self) -> serde_json::Value {
        let mut scope = self.scope().clone();
        scope.description.clear();
        match self {
            Self::SocialRelation(v) => {
                serde_json::json!([v.from, v.relation_type, v.to, v.symmetric, scope])
            }
            Self::LanguageConvention(v) => {
                serde_json::json!([v.expression, v.meaning, v.referent, scope])
            }
        }
    }
    pub fn validate(&self) -> Result<()> {
        let bounded = |s: &str, n: usize| !s.trim().is_empty() && s.len() <= n;
        validate_exact_basis(self.basis())?;
        nous_core::DomainTemporalExtent {
            time_domain_ref: self.valid_time_domain_ref().into(),
            extent: self.valid_time().clone(),
        }
        .validate()?;
        let scope = self.scope();
        let j = self.judgment();
        if self.basis().is_empty()
            || self.basis().len() > 64
            || !bounded(&scope.description, 8192)
            || scope.entities.len() > 64
            || !bounded(&j.explanation, 8192)
            || !["hypothesis", "supported", "disputed"].contains(&j.state.as_str())
            || j.estimate
                .is_some_and(|v| !v.is_finite() || !(0.0..=1.0).contains(&v))
            || j.estimate.is_some() && j.estimate_method.as_ref().is_none_or(|v| !bounded(v, 8192))
        {
            return Err(Error::Invalid(
                "invalid social scope, basis or epistemic judgment".into(),
            ));
        }
        for s in [&scope.channel, &scope.conversation, &scope.domain] {
            if s.as_ref().is_some_and(|v| !bounded(v, 1024)) {
                return Err(Error::Invalid("invalid social structural scope".into()));
            }
        }
        match self {
            Self::SocialRelation(v) => {
                if v.from == v.to
                    || !bounded(&v.statement, 65536)
                    || v.relation_type == RelationType::Trust && scope.domain.is_none()
                    || v.symmetric
                        && !matches!(
                            v.relation_type,
                            RelationType::Collaboration | RelationType::ConflictWith
                        )
                    || v.coordination_expectations.len() > 32
                    || v.coordination_expectations
                        .iter()
                        .any(|s| !bounded(s, 8192))
                {
                    return Err(Error::Invalid(
                        "invalid social relation identity or task scope".into(),
                    ));
                }
                if v.role.as_ref().is_some_and(|s| !bounded(s, 1024))
                    || (v.relation_type == RelationType::MemberOf && v.role.is_none())
                {
                    return Err(Error::Invalid(
                        "member_of requires an explicit role; role must be bounded".into(),
                    ));
                }
                if let Some(d) = &v.degree {
                    let valid = match (v.relation_type, d) {
                        (RelationType::Affinity, RelationDegree::Affinity(x)) => {
                            x.is_finite() && (-1.0..=1.0).contains(x)
                        }
                        (RelationType::Familiarity, RelationDegree::Ordinal(x)) => {
                            ["acquainted", "growing", "familiar", "deep"].contains(&x.as_str())
                        }
                        (RelationType::Trust, RelationDegree::Ordinal(x)) => {
                            ["cautious", "limited", "stable"].contains(&x.as_str())
                        }
                        (RelationType::Closeness, RelationDegree::Ordinal(x)) => {
                            ["ordinary", "close", "very_close"].contains(&x.as_str())
                        }
                        (RelationType::Collaboration, RelationDegree::State(x)) => {
                            ["proposed", "active", "paused", "ended"].contains(&x.as_str())
                        }
                        (RelationType::ConflictWith, RelationDegree::State(x)) => {
                            ["disagreement", "ongoing", "resolved"].contains(&x.as_str())
                        }
                        _ => false,
                    };
                    if !valid {
                        return Err(Error::Invalid(
                            "degree is not defined by this relation type".into(),
                        ));
                    }
                }
            }
            Self::LanguageConvention(v) => {
                if !bounded(&v.expression, 4096)
                    || !bounded(&v.meaning, 65536)
                    || v.variants.len() > 64
                    || v.variants.iter().any(|s| !bounded(s, 4096))
                    || v.pragmatic_use.len() > 8192
                    || scope.entities.is_empty()
                        && scope.group.is_none()
                        && scope.channel.is_none()
                        && scope.conversation.is_none()
                        && scope.work_context.is_none()
                {
                    return Err(Error::Invalid("language convention requires explicit social scope, expression and meaning".into()));
                }
            }
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SocialEvidence {
    pub role: String,
    pub basis: RevisionBasis,
    pub recorded_at: DateTime<Utc>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SocialRecord {
    pub object_id: Uuid,
    pub revision_id: Uuid,
    pub subject_id: SubjectId,
    pub current_revision_id: Uuid,
    pub object_epoch: i64,
    pub revision_no: i32,
    pub acceptance_state: String,
    pub integrity_state: String,
    pub suppression_state: String,
    pub parent_revision_id: Option<Uuid>,
    pub reason: String,
    pub content: SocialContent,
    pub formed_at: DateTime<Utc>,
    pub formed_time_domain_ref: String,
    pub recorded_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    pub evidence: Vec<SocialEvidence>,
}
impl SocialRecord {
    pub fn reference(&self) -> CognitiveRef {
        match self.content {
            SocialContent::SocialRelation(_) => {
                CognitiveRef::SocialRelationRevision(SocialRelationRevisionId(self.revision_id))
            }
            SocialContent::LanguageConvention(_) => CognitiveRef::LanguageConventionRevision(
                LanguageConventionRevisionId(self.revision_id),
            ),
        }
    }
    pub fn object_reference(&self) -> CognitiveRef {
        match self.content {
            SocialContent::SocialRelation(_) => {
                CognitiveRef::SocialRelation(SocialRelationId(self.object_id))
            }
            SocialContent::LanguageConvention(_) => {
                CognitiveRef::LanguageConvention(LanguageConventionId(self.object_id))
            }
        }
    }
    pub fn is_current_available(&self) -> bool {
        self.acceptance_state == "accepted"
            && self.integrity_state == "valid"
            && self.suppression_state == "normal"
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdoptSocial {
    pub subject: SubjectId,
    pub operation_id: OperationId,
    pub content: SocialContent,
    pub reason: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviseSocial {
    pub subject: SubjectId,
    pub operation_id: OperationId,
    pub object_id: Uuid,
    pub expected_revision: Uuid,
    pub expected_epoch: i64,
    pub content: SocialContent,
    pub reason: String,
}

/// Initial fixed type semantics; extensions belong to schema/contracts, not user scripts.
#[derive(Debug, Clone, Copy)]
pub struct RelationTypeDefinition {
    pub directed: bool,
    pub shared_matter_allowed: bool,
    pub inverse_read_role: Option<&'static str>,
    pub endpoint_roles: (&'static str, &'static str),
    pub degree_semantics: &'static str,
}
impl RelationType {
    pub fn definition(self) -> RelationTypeDefinition {
        let (shared, inverse, endpoints, degree) = match self {
            Self::Familiarity => (
                false,
                None,
                ("person_or_subject", "person_or_subject"),
                "acquainted < growing < familiar < deep",
            ),
            Self::Affinity => (
                false,
                None,
                ("person_or_subject", "entity"),
                "bounded affinity [-1,1]",
            ),
            Self::Trust => (
                false,
                None,
                ("person_or_subject", "entity"),
                "task-scoped cautious < limited < stable",
            ),
            Self::Closeness => (
                false,
                None,
                ("person_or_subject", "person_or_subject"),
                "ordinary < close < very_close",
            ),
            Self::Collaboration => (
                true,
                None,
                ("entity", "entity"),
                "proposed / active / paused / ended",
            ),
            Self::MentorOf => (
                false,
                Some("mentee_of"),
                ("person_or_subject", "person_or_subject"),
                "role; no universal intensity",
            ),
            Self::MemberOf => (
                false,
                Some("has_member"),
                ("person_or_subject", "group"),
                "role and valid time; no universal intensity",
            ),
            Self::ConflictWith => (
                true,
                None,
                ("entity", "entity"),
                "disagreement / ongoing / resolved",
            ),
        };
        RelationTypeDefinition {
            directed: true,
            shared_matter_allowed: shared,
            inverse_read_role: inverse,
            endpoint_roles: endpoints,
            degree_semantics: degree,
        }
    }
}
