// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use crate::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextBudget {
    pub max_items: usize,
    pub max_text_bytes: usize,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ContextBudgetUsage {
    pub items: usize,
    pub text_bytes: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MaterializationPolicy {
    ReferencesOnly,
    AvailableText,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConsumerProfile {
    pub consumer_id: String,
    pub context_budget: ContextBudget,
    pub accepted_modalities: Vec<Modality>,
    pub materialization_policy: MaterializationPolicy,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkingSetRequest {
    pub subject: SubjectId,
    pub session_id: SessionId,
    pub consumer: ConsumerProfile,
    #[serde(default)]
    pub query_results: Vec<CognitiveHit>,
    #[serde(default)]
    pub references: Vec<CognitiveRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConsumerWorkingSet {
    pub session_id: SessionId,
    pub consumer_id: String,
    pub refs: Vec<CognitiveRef>,
    pub contributions: Vec<ContextContribution>,
    pub budget_used: ContextBudgetUsage,
    pub degradation: Vec<Degradation>,
}

pub struct ContextSource {
    pub source_revision: Option<MemoryRevisionId>,
    pub media_type: String,
    pub text: Option<String>,
    pub authority: AuthorityClass,
    pub evidence: Vec<EvidenceHandle>,
    pub provenance: ProvenanceSummary,
}

#[async_trait::async_trait]
pub trait ContextResolver: Send + Sync {
    async fn context_source(
        &self,
        subject: SubjectId,
        reference: &CognitiveRef,
        max_bytes: usize,
        explicit: bool,
    ) -> Result<ContextSource>;
}

impl CognitiveRuntimeService {
    pub async fn working_set(
        &self,
        request: WorkingSetRequest,
        resolver: &dyn ContextResolver,
    ) -> Result<ConsumerWorkingSet> {
        let profile = &request.consumer;
        if profile.consumer_id.trim().is_empty()
            || profile.context_budget.max_items == 0
            || profile.context_budget.max_items > 2048
        {
            return Err(Error::new(
                nous_core::ErrorOwner::Runtime,
                nous_core::ErrorCode::InvalidRequest,
                "consumer identity and a bounded context item budget are required",
            ));
        }
        self.require_session(request.subject, request.session_id)
            .await?;
        let explicit_refs: HashSet<_> = request
            .references
            .iter()
            .cloned()
            .chain(request.query_results.iter().map(|h| h.reference.clone()))
            .collect();
        let mut references = request
            .query_results
            .iter()
            .map(|hit| hit.reference.clone())
            .chain(request.references)
            .collect::<Vec<_>>();
        references.extend(
            self.session(request.subject, request.session_id)
                .await?
                .resident
                .into_iter()
                .rev()
                .map(|resident| resident.reference),
        );
        let mut seen = HashSet::new();
        let mut result = ConsumerWorkingSet {
            session_id: request.session_id,
            consumer_id: profile.consumer_id.clone(),
            refs: Vec::new(),
            contributions: Vec::new(),
            budget_used: ContextBudgetUsage::default(),
            degradation: Vec::new(),
        };
        for reference in references {
            if !seen.insert(reference.clone()) {
                continue;
            }
            if result.budget_used.items >= profile.context_budget.max_items {
                break;
            }
            self.store
                .validate_reference(request.subject, &reference)
                .await?;
            let remaining = profile
                .context_budget
                .max_text_bytes
                .saturating_sub(result.budget_used.text_bytes);
            let source = match resolver
                .context_source(
                    request.subject,
                    &reference,
                    if profile.materialization_policy == MaterializationPolicy::AvailableText {
                        remaining
                    } else {
                        0
                    },
                    explicit_refs.contains(&reference),
                )
                .await
            {
                Ok(source) => source,
                Err(error) => {
                    result.degradation.push(Degradation {
                        code: "context_source_unavailable".into(),
                        detail: Some(error.to_string()),
                    });
                    continue;
                }
            };
            if !profile
                .accepted_modalities
                .contains(&modality(&source.media_type))
            {
                continue;
            }
            let text = if profile.materialization_policy == MaterializationPolicy::AvailableText {
                source.text.map(|text| bounded_text(text, remaining))
            } else {
                None
            };
            result.budget_used.text_bytes += text.as_ref().map_or(0, String::len);
            result.budget_used.items += 1;
            result.refs.push(reference.clone());
            result.contributions.push(ContextContribution {
                source_revision: source.source_revision,
                reference: reference.clone(),
                semantic_role: "context_evidence".into(),
                text,
                authority: source.authority,
                freshness: FreshnessDescriptor {
                    occurred: Vec::new(),
                    observed_at: None,
                    valid_time: TemporalExtent::Unknown,
                    formed_at: None,
                    recorded_at: None,
                },
                evidence: source.evidence,
                provenance: source.provenance,
                priority: 1.0,
                estimated_tokens: None,
                materialization: Some(MaterializationHandle {
                    reference,
                    level: "source".into(),
                }),
            });
        }
        Ok(result)
    }
}

pub fn modality(media: &str) -> Modality {
    if media.starts_with("text/") {
        Modality::Text
    } else if media.starts_with("image/") {
        Modality::Image
    } else if media.starts_with("audio/") {
        Modality::Audio
    } else if media.starts_with("video/") {
        Modality::Video
    } else if media.contains("json") || media.contains("xml") {
        Modality::Structured
    } else {
        Modality::Binary
    }
}

pub fn bounded_text(mut text: String, max_bytes: usize) -> String {
    let mut end = text.len().min(max_bytes);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text.truncate(end);
    text
}
