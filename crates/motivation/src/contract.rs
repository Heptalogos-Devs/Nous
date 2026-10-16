// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
use chrono::{DateTime, Utc};
use nous_core::*;
use nous_core::{DomainStamp, DomainTemporalExtent};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MotivationScope {
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
pub enum TemporalForm {
    Achieve,
    Cease,
    Maintain,
    Avoid,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Comparison {
    Lt,
    Le,
    Eq,
    Ge,
    Gt,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Criterion {
    EventWitness {
        version: u32,
        event_type: String,
        target: Option<String>,
        activity: Option<String>,
    },
    OwnerState {
        version: u32,
        reference: CognitiveRef,
        state: String,
    },
    Measurement {
        version: u32,
        name: String,
        unit: String,
        comparison: Comparison,
        value: f64,
    },
    Relation {
        version: u32,
        reference: CognitiveRef,
        relation_type: String,
        degree: Option<String>,
    },
    Semantic {
        version: u32,
        question: String,
        material_refs: Vec<CognitiveRef>,
    },
    All {
        items: Vec<Criterion>,
    },
    Any {
        items: Vec<Criterion>,
    },
    Not {
        item: Box<Criterion>,
    },
}
impl Criterion {
    pub fn validate(&self, max_depth: usize, max_leaves: usize) -> Result<()> {
        fn visit(c: &Criterion, d: usize, max: usize, n: &mut usize) -> Result<()> {
            if d > max {
                return Err(Error::Invalid("criterion depth budget exceeded".into()));
            }
            let valid = match c {
                Criterion::All { items } | Criterion::Any { items } => {
                    if items.is_empty() {
                        return Err(Error::Invalid("empty criterion composition".into()));
                    }
                    for i in items {
                        visit(i, d + 1, max, n)?;
                    }
                    true
                }
                Criterion::Not { item } => {
                    visit(item, d + 1, max, n)?;
                    true
                }
                Criterion::EventWitness {
                    version,
                    event_type,
                    ..
                } => {
                    *n += 1;
                    *version == 1 && bounded(event_type, 256)
                }
                Criterion::OwnerState {
                    version,
                    reference,
                    state,
                } => {
                    *n += 1;
                    *version == 1
                        && ((matches!(reference, CognitiveRef::WorkContext(_))
                            && ["open", "paused", "ended"].contains(&state.as_str()))
                            || reference.is_cognition_revision()
                                && [
                                    "accepted",
                                    "withdrawn",
                                    "valid",
                                    "revalidation_required",
                                    "normal",
                                    "suppressed",
                                ]
                                .contains(&state.as_str()))
                }
                Criterion::Measurement {
                    version,
                    name,
                    unit,
                    value,
                    ..
                } => {
                    *n += 1;
                    *version == 1 && bounded(name, 256) && bounded(unit, 128) && value.is_finite()
                }
                Criterion::Relation {
                    version,
                    reference,
                    relation_type,
                    degree,
                } => {
                    *n += 1;
                    *version == 1
                        && matches!(reference, CognitiveRef::SocialRelationRevision(_))
                        && bounded(relation_type, 128)
                        && degree.as_ref().is_none_or(|s| bounded(s, 128))
                }
                Criterion::Semantic {
                    version,
                    question,
                    material_refs,
                } => {
                    *n += 1;
                    *version == 1
                        && bounded(question, 8192)
                        && !material_refs.is_empty()
                        && material_refs.len() <= 64
                }
            };
            if !valid {
                return Err(Error::Invalid(
                    "invalid or unsupported criterion version/parameters".into(),
                ));
            }
            Ok(())
        }
        let mut leaves = 0;
        visit(self, 1, max_depth, &mut leaves)?;
        if leaves > max_leaves {
            return Err(Error::Invalid("criterion leaf budget exceeded".into()));
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DesiredCondition {
    pub expression: Criterion,
    pub temporal_form: TemporalForm,
    pub window: DomainTemporalExtent,
    /// Human meaning of required observations. Evaluator derives actual coverage independently.
    pub coverage_requirement: String,
    pub hold_seconds: Option<u64>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MotivationLink {
    pub relation: String,
    pub target_revision: CognitiveRef,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MotivationMatter {
    /// Stable semantic matter key; revisions cannot silently change the subject of a wish.
    pub matter_key: String,
    pub statement: String,
    pub scope: MotivationScope,
    pub basis: Vec<RevisionBasis>,
    pub valid_time: TemporalExtent,
    #[serde(
        default = "nous_core::real_time_domain",
        skip_serializing_if = "nous_core::is_real_time_domain"
    )]
    pub valid_time_domain_ref: String,
    pub producer: Option<ProducerSignature>,
    #[serde(default)]
    pub links: Vec<MotivationLink>,
    pub condition: Option<DesiredCondition>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "family", content = "value", rename_all = "snake_case")]
pub enum MotivationContent {
    Desire(MotivationMatter),
    Concern(MotivationMatter),
    DesiredCondition(MotivationMatter),
}
impl MotivationContent {
    pub fn matter(&self) -> &MotivationMatter {
        match self {
            Self::Desire(v) | Self::Concern(v) | Self::DesiredCondition(v) => v,
        }
    }
    pub fn matter_mut(&mut self) -> &mut MotivationMatter {
        match self {
            Self::Desire(v) | Self::Concern(v) | Self::DesiredCondition(v) => v,
        }
    }
    pub fn family(&self) -> &'static str {
        match self {
            Self::Desire(_) => "desire",
            Self::Concern(_) => "concern",
            Self::DesiredCondition(_) => "desired_condition",
        }
    }
    pub fn basis(&self) -> &[RevisionBasis] {
        &self.matter().basis
    }
    pub fn valid_time(&self) -> &TemporalExtent {
        &self.matter().valid_time
    }
    pub fn scope(&self) -> &MotivationScope {
        &self.matter().scope
    }
    pub fn scope_mut(&mut self) -> &mut MotivationScope {
        &mut self.matter_mut().scope
    }
    pub fn producer_mut(&mut self) -> &mut Option<ProducerSignature> {
        &mut self.matter_mut().producer
    }
    pub fn producer(&self) -> Option<&ProducerSignature> {
        self.matter().producer.as_ref()
    }
    pub fn label(&self) -> &str {
        &self.matter().statement
    }
    pub fn entities(&self) -> Vec<EntityRef> {
        let mut e = self.scope().entities.clone();
        if let Some(g) = &self.scope().group {
            e.push(g.clone())
        }
        e.sort_by(|a, b| a.as_str().cmp(b.as_str()));
        e.dedup();
        e
    }
    pub fn text(&self) -> String {
        serde_json::to_string_pretty(self).expect("serializable Motivation")
    }
    pub fn identity(&self) -> serde_json::Value {
        serde_json::json!([self.family(), self.matter().matter_key])
    }
    pub fn validate(&self) -> Result<()> {
        let m = self.matter();
        validate_exact_basis(&m.basis)?;
        nous_core::DomainTemporalExtent {
            time_domain_ref: m.valid_time_domain_ref.clone(),
            extent: m.valid_time.clone(),
        }
        .validate()?;
        if !bounded(&m.matter_key, 1024)
            || !bounded(&m.statement, 65536)
            || !bounded(&m.scope.description, 8192)
            || m.basis.is_empty()
            || m.basis.len() > 64
            || m.scope.entities.len() > 64
            || m.links.len() > 64
        {
            return Err(Error::Invalid(
                "invalid Motivation matter/scope/basis".into(),
            ));
        }
        for field in [&m.scope.channel, &m.scope.conversation, &m.scope.domain] {
            if field.as_ref().is_some_and(|v| !bounded(v, 1024)) {
                return Err(Error::Invalid(
                    "Motivation structural scope must be bounded".into(),
                ));
            }
        }
        for link in &m.links {
            if ![
                "refines",
                "prerequisite",
                "conflicts",
                "long_term_priority",
                "supports",
                "replaces",
            ]
            .contains(&link.relation.as_str())
                || link.target_revision.domain() != CognitiveDomain::Motivation
                || !link.target_revision.is_cognition_revision()
            {
                return Err(Error::Invalid("invalid exact Motivation link".into()));
            }
        }
        if let Self::DesiredCondition(_) = self {
            let c = m
                .condition
                .as_ref()
                .ok_or_else(|| Error::Invalid("DesiredCondition requires a criterion".into()))?;
            c.expression.validate(32, 1024)?;
            c.window.extent.validate()?;
            if !bounded(&c.window.time_domain_ref, 1024) || !bounded(&c.coverage_requirement, 8192)
            {
                return Err(Error::Invalid("invalid condition time/coverage".into()));
            }
        } else if m.condition.is_some() {
            return Err(Error::Invalid(
                "Desire/Concern cannot masquerade as DesiredCondition".into(),
            ));
        }
        Ok(())
    }
}
fn bounded(s: &str, n: usize) -> bool {
    !s.trim().is_empty() && s.len() <= n
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MotivationEvidence {
    pub role: String,
    pub basis: RevisionBasis,
    pub recorded_at: DateTime<Utc>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MotivationRecord {
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
    pub content: MotivationContent,
    pub formed_at: DateTime<Utc>,
    pub recorded_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    pub formed_stamp: DomainStamp,
    pub evidence: Vec<MotivationEvidence>,
}
impl MotivationRecord {
    pub fn reference(&self) -> CognitiveRef {
        match self.content {
            MotivationContent::Desire(_) => {
                CognitiveRef::DesireRevision(DesireRevisionId(self.revision_id))
            }
            MotivationContent::Concern(_) => {
                CognitiveRef::ConcernRevision(ConcernRevisionId(self.revision_id))
            }
            MotivationContent::DesiredCondition(_) => {
                CognitiveRef::DesiredConditionRevision(DesiredConditionRevisionId(self.revision_id))
            }
        }
    }
    pub fn object_reference(&self) -> CognitiveRef {
        match self.content {
            MotivationContent::Desire(_) => CognitiveRef::Desire(DesireId(self.object_id)),
            MotivationContent::Concern(_) => CognitiveRef::Concern(ConcernId(self.object_id)),
            MotivationContent::DesiredCondition(_) => {
                CognitiveRef::DesiredCondition(DesiredConditionId(self.object_id))
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
pub struct AdoptMotivation {
    pub subject: SubjectId,
    pub operation_id: OperationId,
    pub content: MotivationContent,
    pub reason: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviseMotivation {
    pub subject: SubjectId,
    pub operation_id: OperationId,
    pub object_id: Uuid,
    pub expected_revision: Uuid,
    pub expected_epoch: i64,
    pub content: MotivationContent,
    pub reason: String,
}
