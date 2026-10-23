// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0 AND AGPL-3.0-only
// Adapted sources and licenses: ../structured-sources.json.

use nous_domain as d;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

macro_rules! catalog_key {
    ($name:ident) => {
        #[derive(Debug, Clone, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub String);
        impl JsonSchema for $name {
            fn schema_name() -> std::borrow::Cow<'static, str> { stringify!($name).into() }
            fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
                json!({"type":"string","minLength":1,"maxLength":256}).try_into().expect("key schema is an object")
            }
        }
    };
}
catalog_key!(ReferenceKey);
catalog_key!(SourceKey);
catalog_key!(MemoryKey);
catalog_key!(ObservationKey);

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RawEvidenceLink {
    pub source: SourceKey,
    pub polarity: d::EvidencePolarity,
    pub relation: Option<d::EpistemicRelation>,
    pub explanation: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RawLineageLink {
    pub relation: d::LineageRelation,
    pub target: ReferenceKey,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RawMemorySemantics {
    pub memory: MemoryKey,
    pub cognitive_role: d::CognitiveRole,
    pub formation_mode: d::FormationMode,
    pub epistemic: d::EpistemicStance,
    pub grounding_observation: Option<ObservationKey>,
    pub concepts: Vec<ReferenceKey>,
    pub qualifications: Vec<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RawConceptContent {
    pub label: String,
    pub description: String,
    pub aliases: Vec<String>,
    pub canonical: Option<ReferenceKey>,
    pub members: Vec<ReferenceKey>,
    pub lineage: Vec<RawLineageLink>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RawEpisodeMember {
    pub source: SourceKey,
    pub role: String,
    pub ordinal: u32,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RawEpisodeContent {
    pub title: String,
    pub track: String,
    pub experience: d::TimeRange,
    pub parent: Option<ReferenceKey>,
    pub members: Vec<RawEpisodeMember>,
    pub boundary_explanation: String,
    pub lineage: Vec<RawLineageLink>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RawJournalPoint {
    pub role: d::JournalPointRole,
    pub text: String,
    pub evidence: Vec<RawEvidenceLink>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RawJournalContent {
    pub title: String,
    pub narrative: String,
    pub period: d::TimeRange,
    pub perspective: d::EpistemicStance,
    pub points: Vec<RawJournalPoint>,
    pub episodes: Vec<ReferenceKey>,
    pub lineage: Vec<RawLineageLink>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RawSchemaScope {
    pub description: String,
    pub about: Vec<ReferenceKey>,
    pub concepts: Vec<ReferenceKey>,
    pub validity: Option<d::TimeRange>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RawSchemaContent {
    pub title: String,
    pub claim: String,
    pub scope: RawSchemaScope,
    pub boundary: String,
    pub formation_mode: d::FormationMode,
    pub epistemic: d::EpistemicStance,
    pub support: Vec<SourceKey>,
    pub counterexamples: Vec<SourceKey>,
    pub boundary_cases: Vec<SourceKey>,
    pub lineage: Vec<RawLineageLink>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RawAssociationContent {
    pub from: ReferenceKey,
    pub to: ReferenceKey,
    pub relation: String,
    pub polarity: d::AssociationPolarity,
    pub basis_class: d::AssociationBasisClass,
    pub description: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(
    tag = "kind",
    content = "content",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum RawCognitiveContent {
    MemorySemantics(RawMemorySemantics),
    Concept(RawConceptContent),
    Episode(RawEpisodeContent),
    Journal(RawJournalContent),
    Schema(RawSchemaContent),
    Association(RawAssociationContent),
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RawProposalChange {
    pub slot_key: String,
    pub body: RawCognitiveContent,
    pub evidence: Vec<RawEvidenceLink>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RawProposalGroup {
    pub key: String,
    pub depends_on: Vec<String>,
    pub changes: Vec<RawProposalChange>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RawStructuredProposal {
    pub groups: Vec<RawProposalGroup>,
}
