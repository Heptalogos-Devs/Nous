// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
use chrono::{DateTime, Utc};
use nous_core::*;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SelfScope {
    pub description: String,
    #[serde(default)]
    pub purposes: Vec<String>,
    #[serde(default)]
    pub entities: Vec<EntityRef>,
    pub work_context: Option<WorkContextId>,
    pub conversation: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SelfException {
    pub statement: String,
    pub basis: Vec<RevisionBasis>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SelfFacetContent {
    pub kind: String,
    pub statement: String,
    pub epistemic_class: EpistemicClass,
    pub scope: SelfScope,
    pub stance: String,
    #[serde(default)]
    pub exceptions: Vec<SelfException>,
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
pub struct NarrativeContent {
    pub theme: String,
    pub interpretation: String,
    pub epistemic_class: EpistemicClass,
    pub source_refs: Vec<CognitiveRef>,
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
pub enum SelfContent {
    SelfFacet(SelfFacetContent),
    NarrativeThread(NarrativeContent),
}
impl SelfContent {
    pub fn family(&self) -> &'static str {
        match self {
            Self::SelfFacet(_) => "self_facet",
            Self::NarrativeThread(_) => "narrative_thread",
        }
    }
    pub fn basis(&self) -> &[RevisionBasis] {
        match self {
            Self::SelfFacet(v) => &v.basis,
            Self::NarrativeThread(v) => &v.basis,
        }
    }
    pub fn valid_time_domain_ref(&self) -> &str {
        match self {
            Self::SelfFacet(value) => &value.valid_time_domain_ref,
            Self::NarrativeThread(value) => &value.valid_time_domain_ref,
        }
    }
    pub fn valid_time(&self) -> &TemporalExtent {
        match self {
            Self::SelfFacet(v) => &v.valid_time,
            Self::NarrativeThread(v) => &v.valid_time,
        }
    }
    pub fn text(&self) -> String {
        let body = match self {
            Self::SelfFacet(v) => format!(
                "{}: {}\nEpistemic status: {}\nStance: {}\nScope: {}{}",
                v.kind,
                v.statement,
                v.epistemic_class.as_str(),
                v.stance,
                serde_json::to_string(&v.scope).expect("Self scope is serializable"),
                v.exceptions
                    .iter()
                    .map(|e| format!("\nException: {}", e.statement))
                    .collect::<String>()
            ),
            Self::NarrativeThread(v) => format!(
                "{}\nEpistemic status: {}\n{}",
                v.theme,
                v.epistemic_class.as_str(),
                v.interpretation
            ),
        };
        format!(
            "{body}\nValidity: {}",
            serde_json::to_string(self.valid_time()).expect("Temporal primitives are serializable")
        )
    }
    pub fn label(&self) -> &str {
        match self {
            Self::SelfFacet(v) => &v.statement,
            Self::NarrativeThread(v) => &v.theme,
        }
    }
    pub fn validate(&self) -> Result<()> {
        let bounded = |s: &str, n: usize| !s.trim().is_empty() && s.len() <= n;
        if self.basis().len() > 64 {
            return Err(Error::Invalid("Self basis exceeds source bound".into()));
        }
        validate_exact_basis(self.basis())?;
        nous_core::DomainTemporalExtent {
            time_domain_ref: self.valid_time_domain_ref().into(),
            extent: self.valid_time().clone(),
        }
        .validate()?;
        match self {
            Self::SelfFacet(v) => {
                if ![
                    "identity",
                    "role",
                    "capability_belief",
                    "limitation",
                    "tendency",
                    "value",
                    "preference",
                ]
                .contains(&v.kind.as_str())
                    || !["adopted", "tentative", "aspirational", "attributed"]
                        .contains(&v.stance.as_str())
                    || !bounded(&v.statement, 65536)
                    || !bounded(&v.scope.description, 8192)
                    || v.exceptions.len() > 32
                    || v.scope.purposes.iter().any(|p| {
                        !["deliberation", "expression", "reflection", "worker"]
                            .contains(&p.as_str())
                    })
                    || v.scope.purposes.len() > 4
                    || v.scope
                        .conversation
                        .as_ref()
                        .is_some_and(|s| !bounded(s, 1024))
                    || v.scope.entities.len() > 64
                {
                    return Err(Error::Invalid("invalid Self facet content or scope".into()));
                }
                for exception in &v.exceptions {
                    if !bounded(&exception.statement, 8192) {
                        return Err(Error::Invalid("invalid Self exception".into()));
                    }
                    validate_exact_basis(&exception.basis)?;
                }
            }
            Self::NarrativeThread(v) => {
                if !bounded(&v.theme, 1024)
                    || !bounded(&v.interpretation, 65536)
                    || v.source_refs.is_empty()
                    || v.source_refs.len() > 64
                    || v.source_refs.iter().any(|r| !r.is_cognition_revision())
                {
                    return Err(Error::Invalid(
                        "Narrative needs a theme, interpretation and exact cognition sources"
                            .into(),
                    ));
                }
            }
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SelfEvidence {
    pub role: String,
    pub basis: RevisionBasis,
    pub recorded_at: DateTime<Utc>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SelfRecord {
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
    pub content: SelfContent,
    pub formed_at: DateTime<Utc>,
    pub formed_time_domain_ref: String,
    pub recorded_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    pub evidence: Vec<SelfEvidence>,
}
impl SelfRecord {
    pub fn reference(&self) -> CognitiveRef {
        match self.content {
            SelfContent::SelfFacet(_) => {
                CognitiveRef::SelfFacetRevision(SelfFacetRevisionId(self.revision_id))
            }
            SelfContent::NarrativeThread(_) => {
                CognitiveRef::NarrativeThreadRevision(NarrativeThreadRevisionId(self.revision_id))
            }
        }
    }
    pub fn object_reference(&self) -> CognitiveRef {
        match self.content {
            SelfContent::SelfFacet(_) => CognitiveRef::SelfFacet(SelfFacetId(self.object_id)),
            SelfContent::NarrativeThread(_) => {
                CognitiveRef::NarrativeThread(NarrativeThreadId(self.object_id))
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
pub struct AdoptSelf {
    pub subject: SubjectId,
    pub operation_id: OperationId,
    pub content: SelfContent,
    pub reason: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviseSelf {
    pub subject: SubjectId,
    pub operation_id: OperationId,
    pub object_id: Uuid,
    pub expected_revision: Uuid,
    pub expected_epoch: i64,
    pub content: SelfContent,
    pub reason: String,
}

/// A bounded owner read, distinct from complete management pagination.
pub struct SelfSelectionRequest<'a> {
    pub source_read: Option<&'a SourceReadView>,
    pub scope: &'a SelfScope,
    pub targets: &'a [CognitiveRef],
    pub sources: &'a [CognitiveRef],
    pub ranked: &'a [CognitiveRef],
    pub max_items: usize,
    pub max_bytes: usize,
    pub automatic: bool,
}
pub struct SelfSelection {
    pub records: Vec<SelfRecord>,
    pub degradation: Vec<Degradation>,
}
