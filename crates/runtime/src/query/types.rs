// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use nous_core::*;
use std::collections::HashSet;
use uuid::Uuid;

/// The immutable query contract consumed by candidate providers.
///
/// Binding happens before any serving or Authority candidate is inspected.  A
/// provider may therefore report only lane-local relevance and cannot change
/// the enabled lane set or its budgets while it is running.
#[derive(Debug, Clone)]
pub struct BoundQuery {
    /// The owner-authoritative view shared by this query's candidates and any
    /// later owner validation. Query-only scope and planning remain below.
    pub read_context: ReadContext,
    pub context_snapshot: std::sync::Arc<super::QueryContextSnapshot>,
    pub activation: super::QueryActivation,
    pub activation_view: Option<std::sync::Arc<dyn QueryActivationView>>,
    pub plan: super::QueryPlan,
    pub representation: super::QueryRepresentation,
    pub query_id: Uuid,
    pub bound_at: chrono::DateTime<chrono::Utc>,
    pub source_query: CognitiveQuery,
    pub bound_at_authority_seq: i64,
    pub revision_policy: RevisionPolicy,
    pub exact_bindings: Vec<ExactBinding>,
    pub runtime_refs: Vec<CognitiveRef>,
    pub runtime_sources: Vec<(CognitiveRef, String)>,
    pub topology_seed_refs: Vec<(CognitiveRef, String)>,
    pub selected_embedding_space: Option<EmbeddingSpaceSignature>,
}

/// Minimum subject-scoped read facts shared by query, exact, history, and
/// consumer projection paths. This is deliberately not a query DTO.
#[derive(Debug, Clone)]
pub struct ReadContext {
    pub subject: SubjectId,
    pub view: AuthorityReadView,
    pub temporal: TemporalFrame,
    pub accessibility: AccessibilityQueryPolicy,
    /// Immutable config view captured at query binding. Owner projections
    /// resolve their canonical read policies against this same snapshot.
    pub config_snapshot: std::sync::Arc<nous_configuration::ConfigSnapshot>,
    pub purge_epoch: i64,
}

impl ReadContext {
    pub fn historical_view(&self) -> Option<&HistoricalAuthoritySnapshot> {
        match &self.view {
            AuthorityReadView::Current { .. } => None,
            AuthorityReadView::Historical(snapshot) => Some(snapshot.as_ref()),
        }
    }

    pub fn is_historical(&self) -> bool {
        matches!(self.view, AuthorityReadView::Historical(_))
    }
}

#[derive(Debug, Clone)]
pub enum AuthorityReadView {
    Current { bound_authority_seq: i64 },
    Historical(std::sync::Arc<HistoricalAuthoritySnapshot>),
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

#[derive(Debug, Clone, Copy)]
pub struct AccessibilityQueryPolicy {
    pub effort: CognitiveEffort,
    pub exact_target_bypasses_auto_level: bool,
}

pub trait QueryReadLease: std::fmt::Debug + Send + Sync {}
pub trait QueryActivationView: QueryReadLease {
    fn provider(&self) -> &dyn super::SharedLaneProvider;
    fn generation_trace(&self) -> QueryGenerationTrace {
        QueryGenerationTrace::default()
    }
}

#[derive(Debug, Clone)]
pub struct QueryExecution {
    pub read_lease: Option<std::sync::Arc<dyn QueryReadLease>>,
    pub result: CognitiveQueryResult,
    pub bound: std::sync::Arc<BoundQuery>,
    pub(super) leaves: Vec<BoundLeaf>,
}
#[derive(Debug, Clone)]
pub(super) struct BoundLeaf {
    pub ordinal: usize,
    pub bound: std::sync::Arc<BoundQuery>,
    pub hits: Vec<CandidateStamp>,
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

impl BoundQuery {
    pub fn exact_binding_for(&self, reference: &CognitiveRef) -> Option<&ExactBinding> {
        self.exact_bindings
            .iter()
            .find(|binding| &binding.requested_ref == reference)
    }
}
