//! Local concept proposals; every accepted change belongs to its typed owner mutation.
// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

mod context;
mod plan;
use super::*;
use nous_configuration::*;
use std::collections::BTreeMap;
pub const CONCEPT_MAINTENANCE: ConfigKey<ConceptMaintenancePolicy> =
    ConfigKey::new("maintenance.concept");
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConceptMaintenancePolicy {
    pub max_candidates: usize,
    pub max_suggestions: usize,
}
impl Default for ConceptMaintenancePolicy {
    fn default() -> Self {
        Self {
            max_candidates: 16,
            max_suggestions: 4,
        }
    }
}
pub(super) fn register_concept_configuration(registry: &mut ConfigRegistryBuilder) -> Result<()> {
    registry.register(
        CONCEPT_MAINTENANCE,
        "memory",
        "Local concept candidates and ordered suggestions.",
        ConceptMaintenancePolicy::default(),
        ConfigExposure::Developer,
        ConfigScopePolicy::SubjectOverrideAllowed,
        ConfigApplyMode::Live,
        ConfigSemanticEffect::AuthorityFormation,
        |p| {
            if !(1..=32).contains(&p.max_candidates) || !(1..=4).contains(&p.max_suggestions) {
                return Err(Error::Invalid("concept policy bounds exceeded".into()));
            }
            Ok(())
        },
    )
}
#[derive(Debug, Clone)]
pub struct ConceptTag {
    pub key: String,
    pub target: TagExpectation,
    pub content: TagContent,
    pub aliases: Vec<String>,
    pub attached: bool,
    pub lexical_score: f64,
    pub semantic_score: Option<f64>,
    pub accretion: Option<AccretionSignal>,
}
#[derive(Debug, Clone)]
pub struct ConceptAssociation {
    pub key: String,
    pub id: AssociationEvidenceId,
    pub from: String,
    pub to: String,
    pub relation: String,
}
#[derive(Debug, Clone)]
pub struct ConceptPlan {
    pub subject: SubjectId,
    pub authority_seq: i64,
    pub config_digest: String,
    pub policy: ConceptMaintenancePolicy,
    pub references: BTreeMap<String, CognitiveRef>,
    pub tags: Vec<ConceptTag>,
    pub associations: Vec<ConceptAssociation>,
    pub basis: BTreeMap<String, AssociationBasis>,
    pub model_input: serde_json::Value,
}
impl MemoryService {
    pub(crate) async fn enqueue_concept_in(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        subject: SubjectId,
        mut reference: CognitiveRef,
        sequence: i64,
    ) -> Result<()> {
        if let CognitiveRef::CognitiveSchema(id) = reference {
            let revision:Uuid = sqlx::query_scalar("SELECT current_revision_id FROM cognitive_schemas WHERE subject_id=$1 AND schema_id=$2")
                .bind(subject.0).bind(id.0).fetch_one(&mut **tx).await.map_err(db)?;
            reference = CognitiveRef::CognitiveSchemaRevision(CognitiveSchemaRevisionId(revision));
        }
        let (kind, value) = reference_parts(&reference);
        self.cognition
            .enqueue_maintenance_in(
                tx,
                &nous_runtime::MaintenanceRequest {
                    subject,
                    kind: "concept_maintenance".into(),
                    scope_kind: kind,
                    scope_ref: value,
                    trigger_authority_seq: sequence,
                    due_at: self.cognition.recorded_now(),
                    priority: 20,
                },
            )
            .await?;
        Ok(())
    }
}
