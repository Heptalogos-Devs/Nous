// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use nous_core::*;
use std::collections::{BTreeMap, HashSet};
use uuid::Uuid;

/// The immutable query contract consumed by candidate providers.
///
/// Binding happens before any serving or Authority candidate is inspected.  A
/// provider may therefore report only lane-local relevance and cannot change
/// the enabled lane set or its budgets while it is running.
#[derive(Debug, Clone)]
pub struct BoundQuery {
    pub context_snapshot: std::sync::Arc<super::QueryContextSnapshot>,
    pub historical_authority: Option<std::sync::Arc<HistoricalAuthoritySnapshot>>,
    pub activation: super::QueryActivation,
    pub activation_view: Option<std::sync::Arc<dyn QueryActivationView>>,
    pub concept_enrichment: super::ConceptEnrichment,
    pub representation: std::sync::Arc<super::QueryRepresentation>,
    pub query_id: Uuid,
    pub bound_at: chrono::DateTime<chrono::Utc>,
    pub source_query: CognitiveQuery,
    pub bound_at_authority_seq: i64,
    pub revision_policy: RevisionPolicy,
    pub exact_bindings: Vec<ExactBinding>,
    pub enabled_lanes: Vec<EvidenceFamily>,
    pub lane_budgets: BTreeMap<EvidenceFamily, usize>,
    pub selected_embedding_space: Option<EmbeddingSpaceSignature>,
    pub topology_required: bool,
    pub config_snapshot: nous_configuration::ConfigSnapshot,
    pub retrieval_policy: super::RetrievalPolicy,
}

#[derive(Debug, Clone)]
pub enum RevisionPolicy {
    CurrentOnly,
    ExactHistorical {
        allowed_revision_refs: HashSet<CognitiveRef>,
    },
}

impl RevisionPolicy {
    pub fn allows_historical(&self, reference: &CognitiveRef) -> bool {
        matches!(
            self,
            Self::ExactHistorical {
                allowed_revision_refs
            } if allowed_revision_refs.contains(reference)
        )
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ExactBinding {
    pub requested_ref: CognitiveRef,
    pub bound_ref: CognitiveRef,
    pub bound_object_epoch: Option<i64>,
    pub mutable_object: bool,
}

pub trait QueryActivationView: std::fmt::Debug + Send + Sync {
    fn provider(&self) -> &dyn super::SharedLaneProvider;
    fn generation_trace(&self) -> QueryGenerationTrace {
        QueryGenerationTrace::default()
    }
}

#[derive(Debug, Clone)]
pub struct QueryExecution {
    pub result: CognitiveQueryResult,
    pub bound: BoundQuery,
    pub(super) leaves: Vec<BoundLeaf>,
}
#[derive(Debug, Clone)]
pub(super) struct BoundLeaf {
    pub ordinal: usize,
    pub expression: CognitiveQueryExpr,
    pub exact_bindings: Vec<ExactBinding>,
    pub hits: Vec<CandidateStamp>,
}
impl BoundLeaf {
    pub fn validation_bound(&self, prepared: &BoundQuery) -> BoundQuery {
        let mut bound = prepared.clone();
        bound.source_query.expression = self.expression.clone();
        bound.exact_bindings = self.exact_bindings.clone();
        bound
    }
}
#[derive(Debug, Clone)]
pub(super) struct CandidateStamp {
    pub reference: CognitiveRef,
    pub revision: Option<CognitiveRef>,
    pub authority_epoch: Option<i64>,
}
impl From<&CognitiveHit> for CandidateStamp {
    fn from(hit: &CognitiveHit) -> Self {
        Self {
            reference: hit.reference.clone(),
            revision: hit.revision.clone(),
            authority_epoch: hit.authority_epoch,
        }
    }
}

#[derive(Default)]
pub struct QueryHostCapabilities {
    pub embedding_available: bool,
    pub embedding_required: bool,
    pub enrichment_available: bool,
    pub enrichment_required: bool,
    pub rerank_available: bool,
    pub rerank_required: bool,
}

impl BoundQuery {
    pub fn host_work(&mut self, host: &QueryHostCapabilities) -> Result<(bool, bool)> {
        let query = &self.source_query;
        let embedding = query.capabilities.text_embedding != RequirementStrength::Forbidden
            && !self.representation.text.is_empty()
            && (self.concept_enrichment != super::ConceptEnrichment::Off
                || self.enabled_lanes.iter().any(|lane| {
                    *lane == EvidenceFamily::Dense
                        || (*lane == EvidenceFamily::TopologyWave
                            && self
                                .retrieval_policy
                                .cognitive_profile
                                .requirements()
                                .query_embedding)
                }));
        let enrichment = self.concept_enrichment == super::ConceptEnrichment::Model
            && query.capabilities.query_concept_enrichment != RequirementStrength::Forbidden;
        let mut nodes = vec![&query.expression];
        let mut intent = false;
        while let Some(node) = nodes.pop() {
            intent |= node.cues.iter().any(|cue| match cue {
                Cue::Text(c) => !c.text.trim().is_empty(),
                Cue::Concept(c) => !c.text.trim().is_empty(),
                _ => false,
            });
            nodes.extend(&node.children);
        }
        let rerank = !query.is_exact_read()
            && intent
            && query.capabilities.rerank != RequirementStrength::Forbidden;
        for (needed, available, required, role, requirement) in [
            (
                embedding,
                host.embedding_available,
                host.embedding_required,
                "query_embedding",
                query.capabilities.text_embedding,
            ),
            (
                enrichment,
                host.enrichment_available,
                host.enrichment_required,
                "query_concept_enrichment",
                query.capabilities.query_concept_enrichment,
            ),
            (
                rerank,
                host.rerank_available,
                host.rerank_required,
                "query_rerank",
                query.capabilities.rerank,
            ),
        ] {
            if needed && !available && (required || requirement == RequirementStrength::Required) {
                return Err(DomainError::new(
                    DomainErrorCode::CapabilityUnavailable,
                    format!("Required model.{role} is unavailable"),
                )
                .with_context("capability", format!("model.{role}"))
                .with_context("requirement", "required")
                .into());
            }
        }
        if enrichment && !host.enrichment_available {
            self.concept_enrichment = super::ConceptEnrichment::Existing;
            self.activation.degradation.push(Degradation {code: "query_concept_model_unavailable".into(), detail: Some("Host has no executable query_concept_enrichment role; existing concepts remain available".into())});
        }
        Ok((
            embedding && host.embedding_available,
            embedding && host.embedding_available
                || enrichment && host.enrichment_available
                || rerank && host.rerank_available,
        ))
    }

    pub fn runtime_sources(&self) -> impl Iterator<Item = &super::QuerySource> {
        self.context_snapshot
            .source_refs
            .iter()
            .filter(|source| source.is_runtime())
    }

    pub fn query(&self) -> &CognitiveQuery {
        &self.source_query
    }

    pub fn lane_enabled(&self, family: EvidenceFamily) -> bool {
        self.enabled_lanes.contains(&family)
    }

    pub fn lane_budget(&self, family: EvidenceFamily) -> usize {
        self.lane_budgets.get(&family).copied().unwrap_or(0)
    }
}
