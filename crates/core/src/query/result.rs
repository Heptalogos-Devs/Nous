// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QueryStatus {
    Complete,
    Degraded,
    Partial,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Degradation {
    pub code: String,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct QueryGenerationTrace {
    pub lexical: Option<ServingGenerationId>,
    #[serde(default)]
    pub dense: Vec<ServingGenerationId>,
    pub topology: Option<ServingGenerationId>,
    pub epa_basis: Option<ServingGenerationId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceFamily {
    Exact,
    Runtime,
    Entity,
    Lexical,
    Dense,
    Temporal,
    SchemaDirect,
    TagDirect,
    TopologyWave,
    Resource,
    LanguageRerank,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MatchEvidence {
    #[serde(default)]
    pub families: Vec<EvidenceFamily>,
    pub base_rank_score: f64,
    pub preference_score: f64,
    pub rerank_score: Option<f64>,
    pub baseline_rank: u32,
    pub final_rank: u32,
    pub best_lane_rank: u32,
    pub enabled_lane_count: u32,
    pub final_score: f64,
    #[serde(default)]
    pub variants: Vec<String>,
    pub explanation: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CognitiveHit {
    pub authority_epoch: Option<i64>,
    #[serde(default)]
    pub preference_refs: Vec<CognitiveRef>,
    pub reference: CognitiveRef,
    pub revision: Option<CognitiveRef>,
    pub semantic_role: Option<String>,
    pub cognitive_role: Option<String>,
    pub formation_mode: Option<String>,
    pub representation: Option<String>,
    pub authority: AuthorityClass,
    pub freshness: FreshnessDescriptor,
    #[serde(default)]
    pub entity_refs: Vec<EntityRef>,
    #[serde(default)]
    pub evidence: Vec<EvidenceHandle>,
    pub match_evidence: MatchEvidence,
    #[serde(default)]
    pub materialization: Vec<MaterializationHandle>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceActionSuggestion {
    pub action_id: Uuid,
    pub query_text: String,
    pub limit: usize,
    pub materialize: bool,
    pub adapter_kind: String,
    pub provider_profile: String,
    pub provider_locator: String,
    pub descriptor_digest: String,
    pub resource: ResourceRef,
    pub action: String,
    pub reason: String,
    #[serde(default)]
    pub current_authority: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StableExternalRef {
    pub provider_kind: String,
    pub provider_profile: String,
    pub profile_digest: String,
    pub resource_ref: ResourceRef,
    pub provider_resource_id: String,
    pub entry_id: String,
    pub entry_version: Option<String>,
    pub content_digest: String,
    pub source_locator: String,
    pub retrieved_at: String,
    pub access_scope: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExternalResourceRecord {
    pub resource_ref: ResourceRef,
    pub reference: StableExternalRef,
    pub title: Option<String>,
    pub content: String,
    pub provider_rank: u32,
    pub provider_score: Option<f64>,
    pub version_status: String,
    pub access_status: String,
}
#[derive(Debug, Clone)]
pub struct ExternalResourceResult {
    pub action_id: Uuid,
    pub resource_ref: ResourceRef,
    pub status: String,
    pub records: Vec<ExternalResourceRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryDiagnostics {
    pub candidate_counts: std::collections::BTreeMap<String, usize>,
    pub lane_status: std::collections::BTreeMap<String, String>,
    pub topology_complete: Option<bool>,
    pub topology_discarded_mass: Option<f64>,
    pub trace: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CognitiveQueryResult {
    pub query_id: Uuid,
    pub generation: QueryGenerationTrace,
    pub status: QueryStatus,
    pub results: Vec<CognitiveHit>,
    #[serde(default)]
    pub resource_actions: Vec<ResourceActionSuggestion>,
    pub resource_records: Vec<ExternalResourceRecord>,
    #[serde(default)]
    pub degradation: Vec<Degradation>,
    pub diagnostics: Option<QueryDiagnostics>,
}

/// Semantic serving families required by one query.  This stays independent
/// of the concrete index implementation used to serve each family.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServingNeed {
    pub lexical: bool,
    pub dense: bool,
    pub topology: bool,
    pub concept: bool,
    pub concept_vectors: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Readiness {
    Ready,
    Degraded,
    Unavailable,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilityStatus {
    pub capability_id: String,
    pub status: Readiness,
    pub reason: Option<String>,
}
