// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum QueryTarget {
    EntityNeighborhood { entity_ref: EntityRef },
    SchemaNeighborhood { schema: CognitiveSchemaId },
    Exact { reference: CognitiveRef },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResultDomain {
    Memory,
    Schema,
    Episode,
    Journal,
    Evidence,
    Resource,
}
impl ResultDomain {
    pub fn name(self) -> &'static str {
        match self {
            Self::Memory => "memory",
            Self::Schema => "schema",
            Self::Episode => "episode",
            Self::Journal => "journal",
            Self::Evidence => "evidence",
            Self::Resource => "resource",
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResultProjection {
    pub domains: Vec<ResultDomain>,
}
impl Default for ResultProjection {
    fn default() -> Self {
        Self {
            domains: vec![
                ResultDomain::Memory,
                ResultDomain::Schema,
                ResultDomain::Episode,
                ResultDomain::Journal,
            ],
        }
    }
}
impl ResultProjection {
    pub fn domain_names(&self) -> Vec<&'static str> {
        self.domains.iter().map(|domain| domain.name()).collect()
    }
    pub fn allows_reference(&self, reference: &CognitiveRef) -> bool {
        self.domain_names()
            .contains(&reference_query_domain(reference))
    }
    pub fn has_cognition(&self) -> bool {
        self.domains.iter().any(|domain| {
            matches!(
                domain,
                ResultDomain::Memory
                    | ResultDomain::Schema
                    | ResultDomain::Episode
                    | ResultDomain::Journal
            )
        })
    }
    pub fn validate(&self) -> Result<()> {
        let unique: std::collections::BTreeSet<_> = self.domains.iter().collect();
        if self.domains.is_empty() || unique.len() != self.domains.len() {
            return Err(Error::Invalid("invalid result projection".into()));
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(tag = "kind", content = "at", rename_all = "snake_case")]
pub enum AuthorityView {
    #[default]
    Current,
    AsOf(DateTime<Utc>),
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum RevisionView {
    #[default]
    Current,
    History,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemporalFrame {
    pub clock_now: DateTime<Utc>,
    pub authority_view: AuthorityView,
    pub revision_view: RevisionView,
    #[serde(default)]
    pub original_expressions: Vec<String>,
}
impl Default for TemporalFrame {
    fn default() -> Self {
        Self {
            clock_now: DateTime::<Utc>::UNIX_EPOCH,
            authority_view: AuthorityView::Current,
            revision_view: RevisionView::Current,
            original_expressions: Vec::new(),
        }
    }
}

pub fn reference_query_domain(reference: &CognitiveRef) -> &'static str {
    match reference {
        CognitiveRef::Memory(_) | CognitiveRef::MemoryRevision(_) => "memory",
        CognitiveRef::CognitiveSchema(_) | CognitiveRef::CognitiveSchemaRevision(_) => "schema",
        CognitiveRef::Episode(_) | CognitiveRef::EpisodeRevision(_) => "episode",
        CognitiveRef::Journal(_) | CognitiveRef::JournalRevision(_) => "journal",
        CognitiveRef::Resource(_) => "resource",
        CognitiveRef::Tag(_) => "concept",
        _ => "evidence",
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextCue {
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConceptCue {
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntityCue {
    pub entity_ref: EntityRef,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObjectCue {
    pub object_ref: ObjectRef,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactCue {
    pub artifact: ArtifactId,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaRegionCue {
    pub region: CognitiveRef,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TagCue {
    pub tag: TagId,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchemaCue {
    pub schema: CognitiveSchemaId,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelationCue {
    pub from: CognitiveRef,
    pub to: CognitiveRef,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExampleCue {
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceCue {
    pub resource: ResourceRef,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Cue {
    Text(TextCue),
    Concept(ConceptCue),
    Entity(EntityCue),
    Object(ObjectCue),
    Artifact(ArtifactCue),
    MediaRegion(MediaRegionCue),
    Tag(TagCue),
    Schema(SchemaCue),
    Relation(RelationCue),
    Example(ExampleCue),
    Resource(ResourceCue),
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct QueryConstraints {
    #[serde(default)]
    pub current_authority: CurrentAuthorityNeed,
    #[serde(default)]
    pub source_classes_include: Vec<SourceClass>,
    #[serde(default)]
    pub source_classes_exclude: Vec<SourceClass>,
    #[serde(default)]
    pub cognitive_roles_include: Vec<String>,
    #[serde(default)]
    pub formation_modes_include: Vec<String>,
    #[serde(default)]
    pub entity_requirements: Vec<EntityRef>,
    pub occurred: Option<TimePredicate>,
    pub observed: Option<TimePredicate>,
    pub valid: Option<TimePredicate>,
    pub formed: Option<TimePredicate>,
    pub recorded: Option<TimePredicate>,
    #[serde(default)]
    pub include_suppressed: bool,
    pub authority: Option<AuthorityClass>,
    #[serde(default)]
    pub modalities: Vec<Modality>,
    #[serde(default)]
    pub evidence_classes: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ExplorationIntent {
    #[default]
    None,
    BoundedAssociative,
    AroundTag,
    AroundSchema,
    ExplainAssociation,
    Global,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CurrentAuthorityNeed {
    #[default]
    None,
    Prefer,
    Required,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ResourceIntent {
    #[serde(default)]
    pub synopsis_only: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResultNeed {
    #[serde(default = "default_result_limit")]
    pub limit: usize,
    #[serde(default = "default_true")]
    pub need_evidence: bool,
    #[serde(default)]
    pub need_materialization_handles: bool,
}

pub const MAX_QUERY_RESULT_ITEMS: usize = 2048;

fn default_result_limit() -> usize {
    // Explicit Rust callers may select the reference default; public requests
    // resolve omitted limits through the Runtime Configuration snapshot.
    let reference: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../config/reference/retrieval-ranking.json"
    ))
    .expect("retrieval reference profile");
    reference["values"]["retrieval.query.default_result_limit"]
        .as_u64()
        .expect("reference result limit") as usize
}

fn default_true() -> bool {
    true
}

impl Default for ResultNeed {
    fn default() -> Self {
        Self {
            limit: default_result_limit(),
            need_evidence: true,
            need_materialization_handles: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CognitiveEffort {
    Light,
    #[default]
    Normal,
    Deep,
    Maximum,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilityPolicy {
    #[serde(default)]
    pub rerank: RequirementStrength,
    #[serde(default)]
    pub text_embedding: RequirementStrength,
    #[serde(default)]
    pub multimodal_interpretation: RequirementStrength,
    #[serde(default)]
    pub residual_sensing: RequirementStrength,
    #[serde(default)]
    pub query_concept_enrichment: RequirementStrength,
}

impl Default for CapabilityPolicy {
    fn default() -> Self {
        Self {
            rerank: RequirementStrength::Optional,
            text_embedding: RequirementStrength::Optional,
            multimodal_interpretation: RequirementStrength::Optional,
            residual_sensing: RequirementStrength::Optional,
            query_concept_enrichment: RequirementStrength::Optional,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticsRequest {
    None,
    #[default]
    Summary,
    Full,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CognitiveQueryExpr {
    pub operation: QueryOperation,
    #[serde(default)]
    pub targets: Vec<QueryTarget>,
    #[serde(default)]
    pub cues: Vec<Cue>,
    #[serde(default)]
    pub constraints: QueryConstraints,
    #[serde(default)]
    pub children: Vec<CognitiveQueryExpr>,
    #[serde(default)]
    pub preferences: Vec<QueryPreference>,
}

impl Default for CognitiveQueryExpr {
    fn default() -> Self {
        Self {
            operation: QueryOperation::Atom,
            targets: Vec::new(),
            cues: Vec::new(),
            constraints: QueryConstraints::default(),
            children: Vec::new(),
            preferences: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryPreference {
    pub negative: bool,
    pub operand: PreferenceOperand,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PreferenceOperand {
    Cue(Cue),
    Exact(CognitiveRef),
    Recent(TimeAxis),
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum TimeAxis {
    Occurred,
    Observed,
    Valid,
    Formed,
    Recorded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QueryOperation {
    Atom,
    All,
    Any,
}
