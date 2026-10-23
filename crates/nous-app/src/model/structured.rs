//! Structured cognition proposal boundary adapted from previous Nous schemas,
//! consolidation/concept/longitudinal prompts (Apache-2.0), and Nous-pr56's
//! createCognition.formChanges source catalog and independent change groups
//! (AGPL-3.0-only). Exact commits, SHA-256 and licenses: structured-sources.json.
// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0 AND AGPL-3.0-only

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
};

use anyhow::{Result as ConfigResult, bail};
use genai::{
    Client, ServiceTarget,
    chat::{ChatMessage, ChatOptions, ChatRequest, JsonSpec},
};
use nous_domain as d;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::{
    ModelCallRecord, ModelFailure, ModelProtocol, ModelResource, ModelRuntime,
    OWNED_REQUEST_FIELDS, PromptMetadata, ResourceMetadata, RetryClass, validate_resource,
};

pub const STRUCTURED_PROMPT_VERSION: &str = "nous.structured.formation.v1";
pub const STRUCTURED_SCHEMA_VERSION: &str = "nous.structured.proposal.v1";
const PROMPT: &str = include_str!("../prompts/structured-formation-v1.md");
const MAX_GROUPS: usize = 8;
const MAX_CHANGES: usize = 32;
const MAX_OUTPUT_BYTES: usize = 256 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StructuredFormationConfig {
    pub resource: ModelResource,
    #[serde(default = "prompt_version")]
    pub prompt_version: String,
    #[serde(default = "input_limit")]
    pub max_input_bytes: usize,
    #[serde(default = "output_limit")]
    pub max_output_tokens: u32,
    #[serde(default = "daily_limit")]
    pub max_calls_per_day: u32,
    #[serde(default)]
    pub native_parameters: BTreeMap<String, Value>,
}
fn prompt_version() -> String {
    STRUCTURED_PROMPT_VERSION.into()
}
fn input_limit() -> usize {
    64 * 1024
}
fn output_limit() -> u32 {
    8192
}
fn daily_limit() -> u32 {
    20
}
impl StructuredFormationConfig {
    pub fn validate(&self) -> ConfigResult<()> {
        validate_resource(&self.resource)?;
        if self.resource.protocol != ModelProtocol::OpenAiChatCompletions {
            bail!("structured formation requires openai_chat_completions protocol");
        }
        if self.prompt_version != STRUCTURED_PROMPT_VERSION {
            bail!("unsupported structured formation prompt version");
        }
        if !(1..=1024 * 1024).contains(&self.max_input_bytes)
            || !(1..=32768).contains(&self.max_output_tokens)
            || self.max_calls_per_day > 100_000
        {
            bail!(
                "structured formation input bytes must be 1..1048576, output tokens 1..32768 and daily calls at most 100000"
            );
        }
        if self
            .native_parameters
            .keys()
            .any(|key| OWNED_REQUEST_FIELDS.contains(&key.as_str()))
        {
            bail!("structured formation native parameters cannot override owned request fields");
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceCatalogEntry {
    pub key: String,
    pub source: d::EvidenceSource,
    pub text: String,
    #[serde(default)]
    pub source_event_kind: Option<d::ObservationKind>,
    #[serde(default)]
    pub truncated: bool,
}
impl SourceCatalogEntry {
    pub fn new(key: impl Into<String>, source: d::EvidenceSource, text: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            source,
            text: text.into(),
            source_event_kind: None,
            truncated: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StructuredFormationInput {
    pub namespace_id: d::NamespaceId,
    pub subject_id: d::SubjectId,
    pub knowledge_cutoff: i64,
    pub sources: Vec<SourceCatalogEntry>,
    pub slots: Vec<d::StructuredTargetSlot>,
}
impl StructuredFormationInput {
    pub fn validate(&self) -> d::Result<()> {
        if self.sources.is_empty()
            || self.sources.len() > 128
            || self.slots.is_empty()
            || self.slots.len() > MAX_CHANGES
            || self.knowledge_cutoff < 0
        {
            return Err(d::Error::invalid(
                "structured input requires 1..128 sources, 1..32 target slots and a nonnegative knowledge cutoff",
            ));
        }
        let mut keys = BTreeSet::new();
        for source in &self.sources {
            validate_key(&source.key)?;
            source.source.validate()?;
            if source.text.trim().is_empty() || !keys.insert(source.key.as_str()) {
                return Err(d::Error::invalid(
                    "source catalog text must be nonempty and keys unique",
                ));
            }
        }
        let mut identities = BTreeSet::new();
        for slot in &self.slots {
            validate_key(&slot.key)?;
            if slot.kind == d::CognitiveKind::Memory
                || slot.expected_revision < 0
                || slot.expected_revision == i64::MAX
                || !keys.insert(slot.key.as_str())
                || !identities.insert((slot.kind, slot.id.as_str()))
            {
                return Err(d::Error::invalid(
                    "structured slots require unique supported targets and keys with nonnegative expected revisions",
                ));
            }
            slot.output_reference()?;
            if slot.kind == d::CognitiveKind::MemorySemantics
                && !self
                    .sources
                    .iter()
                    .any(|entry| matches!(entry.source, d::EvidenceSource::MemoryRevision { .. }))
            {
                return Err(d::Error::invalid(
                    "memory semantics requires an exact existing Memory in the source catalog",
                ));
            }
        }
        Ok(())
    }

    fn reference(&self, key: &str) -> d::Result<d::CognitiveRef> {
        if let Some(entry) = self.sources.iter().find(|entry| entry.key == key) {
            return entry.source.cognitive_ref().ok_or_else(|| {
                d::Error::invalid(format!(
                    "source key {key} is evidence, not a cognition reference"
                ))
            });
        }
        self.slots
            .iter()
            .find(|slot| slot.key == key)
            .ok_or_else(|| d::Error::invalid(format!("unknown cognition catalog/slot key: {key}")))?
            .output_reference()
    }

    fn evidence_source(&self, key: &str) -> d::Result<d::EvidenceSource> {
        if let Some(entry) = self.sources.iter().find(|entry| entry.key == key) {
            return Ok(entry.source.clone());
        }
        self.reference(key)
            .map(|reference| d::EvidenceSource::CognitiveRevision { reference })
    }

    fn observation(&self, key: &str) -> d::Result<d::ObservationId> {
        match self.evidence_source(key)? {
            d::EvidenceSource::Observation { observation_id } => Ok(observation_id),
            _ => Err(d::Error::invalid(
                "grounding observation must select an Observation source catalog key",
            )),
        }
    }
}
fn validate_key(key: &str) -> d::Result<()> {
    if key.is_empty()
        || key.len() > 256
        || key
            .chars()
            .any(|value| value.is_control() || value.is_whitespace())
    {
        return Err(d::Error::invalid(
            "catalog, slot and group keys require 1..256 visible non-whitespace bytes",
        ));
    }
    Ok(())
}

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
impl RawEvidenceLink {
    fn compile(&self, input: &StructuredFormationInput) -> d::Result<d::EvidenceLink> {
        Ok(d::EvidenceLink {
            source: input.evidence_source(&self.source.0)?,
            polarity: self.polarity,
            relation: self.relation,
            explanation: self.explanation.clone(),
        })
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RawLineageLink {
    pub relation: d::LineageRelation,
    pub target: ReferenceKey,
    pub reason: String,
}
impl RawLineageLink {
    fn compile(&self, input: &StructuredFormationInput) -> d::Result<d::LineageLink> {
        Ok(d::LineageLink {
            relation: self.relation,
            target: input.reference(&self.target.0)?,
            reason: self.reason.clone(),
        })
    }
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
impl RawCognitiveContent {
    fn compile(&self, input: &StructuredFormationInput) -> d::Result<d::CognitiveContent> {
        let refs = |values: &[ReferenceKey]| {
            values
                .iter()
                .map(|key| input.reference(&key.0))
                .collect::<d::Result<Vec<_>>>()
        };
        let sources = |values: &[SourceKey]| {
            values
                .iter()
                .map(|key| input.evidence_source(&key.0))
                .collect::<d::Result<Vec<_>>>()
        };
        let lineage = |values: &[RawLineageLink]| {
            values
                .iter()
                .map(|value| value.compile(input))
                .collect::<d::Result<Vec<_>>>()
        };
        Ok(match self {
            Self::MemorySemantics(value) => {
                let memory = input.reference(&value.memory.0)?;
                if memory.kind != d::CognitiveKind::Memory {
                    return Err(d::Error::invalid(
                        "memory semantics must select an existing Memory revision",
                    ));
                }
                d::CognitiveContent::MemorySemantics(d::MemorySemantics {
                    memory_id: memory.id.parse()?,
                    memory_revision: memory.revision,
                    cognitive_role: value.cognitive_role,
                    formation_mode: value.formation_mode,
                    epistemic: value.epistemic.clone(),
                    grounding_observation: value
                        .grounding_observation
                        .as_ref()
                        .map(|key| input.observation(&key.0))
                        .transpose()?,
                    concepts: refs(&value.concepts)?,
                    qualifications: value.qualifications.clone(),
                })
            }
            Self::Concept(value) => d::CognitiveContent::Concept(d::ConceptContent {
                label: value.label.clone(),
                description: value.description.clone(),
                aliases: value.aliases.clone(),
                canonical: value
                    .canonical
                    .as_ref()
                    .map(|key| input.reference(&key.0))
                    .transpose()?,
                members: refs(&value.members)?,
                lineage: lineage(&value.lineage)?,
            }),
            Self::Episode(value) => d::CognitiveContent::Episode(d::EpisodeContent {
                title: value.title.clone(),
                track: value.track.clone(),
                experience: value.experience.clone(),
                parent: value
                    .parent
                    .as_ref()
                    .map(|key| input.reference(&key.0))
                    .transpose()?,
                members: value
                    .members
                    .iter()
                    .map(|member| {
                        Ok(d::EpisodeMember {
                            source: input.evidence_source(&member.source.0)?,
                            role: member.role.clone(),
                            ordinal: member.ordinal,
                        })
                    })
                    .collect::<d::Result<_>>()?,
                boundary_explanation: value.boundary_explanation.clone(),
                lineage: lineage(&value.lineage)?,
            }),
            Self::Journal(value) => d::CognitiveContent::Journal(d::JournalContent {
                title: value.title.clone(),
                narrative: value.narrative.clone(),
                period: value.period.clone(),
                knowledge_cutoff: input.knowledge_cutoff,
                perspective: value.perspective.clone(),
                episodes: refs(&value.episodes)?,
                lineage: lineage(&value.lineage)?,
                points: value
                    .points
                    .iter()
                    .map(|point| {
                        Ok(d::JournalPoint {
                            role: point.role,
                            text: point.text.clone(),
                            evidence: point
                                .evidence
                                .iter()
                                .map(|link| link.compile(input))
                                .collect::<d::Result<_>>()?,
                        })
                    })
                    .collect::<d::Result<_>>()?,
            }),
            Self::Schema(value) => d::CognitiveContent::Schema(d::SchemaContent {
                title: value.title.clone(),
                claim: value.claim.clone(),
                scope: d::SchemaScope {
                    description: value.scope.description.clone(),
                    about: refs(&value.scope.about)?,
                    concepts: refs(&value.scope.concepts)?,
                    validity: value.scope.validity.clone(),
                },
                boundary: value.boundary.clone(),
                formation_mode: value.formation_mode,
                epistemic: value.epistemic.clone(),
                support: sources(&value.support)?,
                counterexamples: sources(&value.counterexamples)?,
                boundary_cases: sources(&value.boundary_cases)?,
                lineage: lineage(&value.lineage)?,
            }),
            Self::Association(value) => d::CognitiveContent::Association(d::AssociationContent {
                from: input.reference(&value.from.0)?,
                to: input.reference(&value.to.0)?,
                relation: value.relation.clone(),
                polarity: value.polarity,
                basis_class: value.basis_class,
                description: value.description.clone(),
            }),
        })
    }
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
impl RawProposalGroup {
    /// Only this group is compiled. Its invalid references cannot erase other
    /// independent proposals. The app/transaction owner checks semantic validity,
    /// expected revisions, dependency outcomes and authoritative source scope.
    pub fn compile(
        &self,
        input: &StructuredFormationInput,
    ) -> d::Result<d::StructuredProposalGroup> {
        input.validate()?;
        let mut assigned = BTreeSet::new();
        let changes = self
            .changes
            .iter()
            .map(|change| {
                let slot = input
                    .slots
                    .iter()
                    .find(|slot| slot.key == change.slot_key)
                    .ok_or_else(|| d::Error::invalid("proposal selected an unknown target slot"))?;
                if !assigned.insert(slot.key.as_str()) {
                    return Err(d::Error::invalid("proposal group repeats a target slot"));
                }
                let body = change.body.compile(input)?;
                if body.kind() != slot.kind {
                    return Err(d::Error::invalid(
                        "proposal body kind differs from its preallocated target slot",
                    ));
                }
                Ok(d::StructuredProposalChange {
                    slot_key: slot.key.clone(),
                    body,
                    evidence: change
                        .evidence
                        .iter()
                        .map(|link| link.compile(input))
                        .collect::<d::Result<_>>()?,
                })
            })
            .collect::<d::Result<_>>()?;
        Ok(d::StructuredProposalGroup {
            key: self.key.clone(),
            depends_on: self.depends_on.clone(),
            changes,
        })
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RawStructuredProposal {
    pub groups: Vec<RawProposalGroup>,
}
impl RawStructuredProposal {
    pub fn validate_envelope(&self) -> d::Result<()> {
        if self.groups.len() > MAX_GROUPS
            || self
                .groups
                .iter()
                .map(|group| group.changes.len())
                .sum::<usize>()
                > MAX_CHANGES
        {
            return Err(d::Error::invalid(
                "structured proposal exceeds its group/change envelope",
            ));
        }
        let mut prior = BTreeSet::new();
        let mut assigned_slots = BTreeSet::new();
        for group in &self.groups {
            validate_key(&group.key)?;
            let mut dependencies = BTreeSet::new();
            if group.changes.is_empty()
                || group.changes.len() > 16
                || prior.contains(&group.key)
                || group
                    .depends_on
                    .iter()
                    .any(|key| !prior.contains(key) || !dependencies.insert(key))
            {
                return Err(d::Error::invalid(
                    "structured groups need unique keys, nonempty bounded changes and distinct earlier dependencies",
                ));
            }
            prior.insert(group.key.clone());
            if group
                .changes
                .iter()
                .any(|change| !assigned_slots.insert(change.slot_key.as_str()))
            {
                return Err(d::Error::invalid(
                    "one proposal may write each target slot only once",
                ));
            }
        }
        Ok(())
    }
}

/// Build the actual provider contract from the typed DTOs, constraining every
/// reference namespace to this invocation and every change to its assigned kind.
pub fn structured_output_schema(input: &StructuredFormationInput) -> ConfigResult<Value> {
    input
        .validate()
        .map_err(|error| anyhow::anyhow!(error.to_string()))?;
    let mut schema = serde_json::to_value(schemars::schema_for!(RawStructuredProposal))?;
    let defs = schema
        .get_mut("$defs")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| anyhow::anyhow!("structured schema has no definitions"))?;
    let source_keys: Vec<_> = input
        .sources
        .iter()
        .map(|source| source.key.clone())
        .chain(input.slots.iter().map(|slot| slot.key.clone()))
        .collect();
    let reference_keys: Vec<_> = input
        .sources
        .iter()
        .filter(|source| source.source.cognitive_ref().is_some())
        .map(|source| source.key.clone())
        .chain(input.slots.iter().map(|slot| slot.key.clone()))
        .collect();
    let memory_keys: Vec<_> = input
        .sources
        .iter()
        .filter(|source| matches!(source.source, d::EvidenceSource::MemoryRevision { .. }))
        .map(|source| source.key.clone())
        .collect();
    let observation_keys: Vec<_> = input
        .sources
        .iter()
        .filter(|source| matches!(source.source, d::EvidenceSource::Observation { .. }))
        .map(|source| source.key.clone())
        .collect();
    for (name, values) in [
        ("SourceKey", source_keys),
        ("ReferenceKey", reference_keys),
        ("MemoryKey", memory_keys),
        ("ObservationKey", observation_keys.clone()),
    ] {
        if !values.is_empty() {
            defs.insert(name.into(), json!({"type":"string","enum":values}));
        }
    }
    if observation_keys.is_empty()
        && let Some(memory) = defs.get_mut("RawMemorySemantics")
    {
        memory["properties"]["grounding_observation"] = json!({"type":"null"});
    }
    let variants = defs
        .get("RawCognitiveContent")
        .and_then(|body| body.get("oneOf").or_else(|| body.get("anyOf")))
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("structured body schema has no typed alternatives"))?
        .clone();
    let change = defs
        .get("RawProposalChange")
        .ok_or_else(|| anyhow::anyhow!("structured change schema is missing"))?
        .clone();
    let mut choices = Vec::new();
    for slot in &input.slots {
        let kind = serde_json::to_value(slot.kind)?;
        let body = variants
            .iter()
            .find(|variant| {
                variant["properties"]["kind"]["const"] == kind
                    || variant["properties"]["kind"]["enum"]
                        .as_array()
                        .is_some_and(|names| names.len() == 1 && names[0] == kind)
            })
            .ok_or_else(|| anyhow::anyhow!("target slot has no typed output schema"))?;
        let mut choice = change.clone();
        choice["properties"]["slot_key"] = json!({"type":"string","const":slot.key});
        choice["properties"]["body"] = body.clone();
        choice["properties"]["evidence"]["minItems"] = json!(1);
        choice["properties"]["evidence"]["maxItems"] = json!(32);
        choices.push(choice);
    }
    defs.insert("RawProposalChange".into(), json!({"anyOf":choices}));
    if let Some(group) = defs.get_mut("RawProposalGroup") {
        group["properties"]["changes"]["minItems"] = json!(1);
        group["properties"]["changes"]["maxItems"] = json!(16);
        group["properties"]["depends_on"]["maxItems"] = json!(MAX_GROUPS - 1);
    }
    schema["properties"]["groups"]["maxItems"] = json!(MAX_GROUPS);
    prune_definitions(&mut schema)?;
    // The pinned SDK's mature provider sanitizer handles nullable required
    // properties and discriminated unions for OpenAI's strict JSON dialect.
    Ok(genai::chat::sanitize_json_schema(
        &schema,
        genai::chat::JsonSchemaDialect::OpenAiStrict,
    ))
}

fn prune_definitions(schema: &mut Value) -> ConfigResult<()> {
    fn references(value: &Value, found: &mut BTreeSet<String>) {
        match value {
            Value::Object(fields) => {
                if let Some(name) = fields
                    .get("$ref")
                    .and_then(Value::as_str)
                    .and_then(|reference| reference.strip_prefix("#/$defs/"))
                {
                    found.insert(name.into());
                }
                for child in fields.values() {
                    references(child, found);
                }
            }
            Value::Array(values) => {
                for child in values {
                    references(child, found);
                }
            }
            _ => {}
        }
    }
    let defs = schema
        .as_object_mut()
        .and_then(|root| root.remove("$defs"))
        .and_then(|value| value.as_object().cloned())
        .ok_or_else(|| anyhow::anyhow!("structured definitions are missing"))?;
    let mut pending = BTreeSet::new();
    references(schema, &mut pending);
    let mut retained = serde_json::Map::new();
    while let Some(name) = pending.pop_first() {
        if retained.contains_key(&name) {
            continue;
        }
        let definition = defs
            .get(&name)
            .ok_or_else(|| anyhow::anyhow!("structured schema references an undefined type"))?
            .clone();
        references(&definition, &mut pending);
        retained.insert(name, definition);
    }
    schema["$defs"] = Value::Object(retained);
    Ok(())
}

#[derive(Debug, Clone, Serialize)]
pub struct StructuredCallMetadata {
    pub schema_version: u32,
    pub call_id: String,
    pub role: String,
    pub resource: ResourceMetadata,
    pub prompt: PromptMetadata,
    pub input: StructuredFormationInput,
    pub rendered_user: String,
    pub output_schema_version: String,
    pub output_schema_sha256: String,
    pub output_schema: Value,
    pub parameters: Value,
    pub max_calls_per_day: u32,
}

pub struct PreparedStructured {
    pub metadata: StructuredCallMetadata,
    client: Client,
    target: ServiceTarget,
    request: ChatRequest,
    options: ChatOptions,
}
impl fmt::Debug for PreparedStructured {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PreparedStructured")
            .field("metadata", &self.metadata)
            .finish_non_exhaustive()
    }
}
pub type StructuredCallRecord = ModelCallRecord<StructuredCallMetadata>;
pub struct StructuredExecution {
    pub record: StructuredCallRecord,
    pub result: Result<RawStructuredProposal, ModelFailure>,
}
impl ModelRuntime {
    pub fn structured_config(&self) -> Option<&StructuredFormationConfig> {
        self.config.structured_formation.as_ref()
    }
    pub fn structured_inspect(&self) -> Value {
        let Some(config) = self.structured_config() else {
            return json!({"state":"waiting_configuration","reason":"structured formation resource is not configured"});
        };
        let ready = self.paths.read_secret(&config.resource.credential).is_ok();
        json!({"state":if ready {"ready"} else {"waiting_configuration"},
            "reason":if ready {None} else {Some("model credential is missing or unreadable")},
            "resource":ResourceMetadata::from(&config.resource),"prompt_version":config.prompt_version,
            "schema_version":STRUCTURED_SCHEMA_VERSION,"max_input_bytes":config.max_input_bytes,
            "max_output_tokens":config.max_output_tokens,"max_calls_per_day":config.max_calls_per_day,
            "max_groups":MAX_GROUPS,"max_changes":MAX_CHANGES,"native_parameters":config.native_parameters,"cost":null})
    }

    pub fn prepare_structured(
        &self,
        input: StructuredFormationInput,
    ) -> Result<PreparedStructured, ModelFailure> {
        let config = self.structured_config().ok_or_else(|| {
            ModelFailure::waiting("structured formation resource is not configured")
        })?;
        input.validate().map_err(domain_failure)?;
        let output_schema = structured_output_schema(&input).map_err(|error| {
            let mut failure = ModelFailure::new(
                RetryClass::Permanent,
                d::ErrorCode::Internal,
                "structured output schema is unavailable",
            );
            failure.source = Some(error.into_boxed_dyn_error());
            failure
        })?;
        let rendered_user = serde_json::to_string(&input)
            .expect("typed structured input has a JSON representation");
        let schema_bytes =
            serde_json::to_vec(&output_schema).expect("output schema has a JSON representation");
        if PROMPT.len() + rendered_user.len() + schema_bytes.len() > config.max_input_bytes {
            return Err(ModelFailure::new(
                RetryClass::Permanent,
                d::ErrorCode::ResourceExhausted,
                "structured prompt, source catalog and output schema exceed max_input_bytes",
            ));
        }
        let options = ChatOptions::default()
            .with_max_tokens(config.max_output_tokens)
            .with_capture_raw_body(true)
            .with_response_format(JsonSpec::new(
                "nous_structured_formation_v1",
                output_schema.clone(),
            ));
        let options = if config.native_parameters.is_empty() {
            options
        } else {
            options.with_extra_body(json!(&config.native_parameters))
        };
        let (client, target) = self.build_client(&config.resource)?;
        let metadata = StructuredCallMetadata {
            schema_version: 1,
            call_id: uuid::Uuid::new_v4().to_string(),
            role: "structured_formation".into(),
            resource: (&config.resource).into(),
            prompt: PromptMetadata {
                id: "structured_formation".into(),
                version: config.prompt_version.clone(),
                sha256: format!("{:x}", Sha256::digest(PROMPT.as_bytes())),
                text: PROMPT.into(),
            },
            input,
            rendered_user: rendered_user.clone(),
            output_schema_version: STRUCTURED_SCHEMA_VERSION.into(),
            output_schema_sha256: format!("{:x}", Sha256::digest(schema_bytes)),
            output_schema,
            parameters: json!({"max_output_tokens":config.max_output_tokens,"max_input_bytes":config.max_input_bytes,
                "native_parameters":config.native_parameters,"max_groups":MAX_GROUPS,"max_changes":MAX_CHANGES,"max_output_bytes":MAX_OUTPUT_BYTES}),
            max_calls_per_day: config.max_calls_per_day,
        };
        Ok(PreparedStructured {
            metadata,
            client,
            target,
            request: ChatRequest::new(vec![ChatMessage::user(rendered_user)]).with_system(PROMPT),
            options,
        })
    }

    /// One shared SDK call. Save record + raw proposals under the existing model
    /// ledger, then let the app compile/adopt each independent group transaction.
    pub async fn execute_structured(&self, prepared: PreparedStructured) -> StructuredExecution {
        let (mut record, text) = self
            .call_chat(
                prepared.metadata,
                prepared.client,
                prepared.target,
                prepared.request,
                prepared.options,
            )
            .await;
        let result = text.and_then(|text| parse_proposal(&text));
        super::complete_call(&mut record, &result);
        StructuredExecution { record, result }
    }
}
fn domain_failure(error: d::Error) -> ModelFailure {
    ModelFailure::new(RetryClass::Permanent, error.code, error.message)
}
fn parse_proposal(text: &str) -> Result<RawStructuredProposal, ModelFailure> {
    if text.len() > MAX_OUTPUT_BYTES {
        return Err(ModelFailure::new(
            RetryClass::Permanent,
            d::ErrorCode::ResourceExhausted,
            "structured proposal exceeds its output byte budget",
        ));
    }
    let proposal: RawStructuredProposal = serde_json::from_str(text).map_err(|error| {
        let mut failure = ModelFailure::new(
            RetryClass::Permanent,
            d::ErrorCode::InvalidArgument,
            "model output does not match the typed structured proposal contract",
        );
        failure.source = Some(Box::new(error));
        failure
    })?;
    proposal.validate_envelope().map_err(domain_failure)?;
    Ok(proposal)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        instance::{InstancePaths, SecretRef},
        model::ModelConfig,
    };
    use axum::{Json, Router, http::HeaderMap, routing::post};
    use std::{
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
        time::Duration,
    };

    fn input() -> StructuredFormationInput {
        let memory_1 = "00000000-0000-0000-0000-000000000001".parse().unwrap();
        let memory_2 = "00000000-0000-0000-0000-000000000002".parse().unwrap();
        let mut observation = SourceCatalogEntry::new(
            "observation",
            d::EvidenceSource::Observation {
                observation_id: "00000000-0000-0000-0000-000000000003".parse().unwrap(),
            },
            "实际阅读两份港湾观测记录。",
        );
        observation.source_event_kind = Some(d::ObservationKind::Contact);
        StructuredFormationInput {
            namespace_id: "00000000-0000-0000-0000-000000000010".parse().unwrap(),
            subject_id: "00000000-0000-0000-0000-000000000011".parse().unwrap(),
            knowledge_cutoff: 100,
            sources: vec![
                SourceCatalogEntry::new(
                    "memory_1",
                    d::EvidenceSource::MemoryRevision {
                        memory_id: memory_1,
                        revision: 1,
                    },
                    "港湾研究组在周三记录海水样本的冷链运输。",
                ),
                SourceCatalogEntry::new(
                    "memory_2",
                    d::EvidenceSource::MemoryRevision {
                        memory_id: memory_2,
                        revision: 2,
                    },
                    "次月另一次调查保留冷链例外与适用条件。",
                ),
                observation,
            ],
            slots: [
                ("semantics", d::CognitiveKind::MemorySemantics),
                ("concept", d::CognitiveKind::Concept),
                ("episode", d::CognitiveKind::Episode),
                ("journal", d::CognitiveKind::Journal),
                ("schema", d::CognitiveKind::Schema),
                ("association", d::CognitiveKind::Association),
            ]
            .into_iter()
            .enumerate()
            .map(|(index, (key, kind))| d::StructuredTargetSlot {
                key: key.into(),
                kind,
                id: format!("00000000-0000-0000-0000-{:012}", index + 101)
                    .parse()
                    .unwrap(),
                expected_revision: 0,
            })
            .collect(),
        }
    }
    fn proposal() -> Value {
        let stance = json!({"class":"reported","attribution":"港湾研究组","confidence":0.7});
        let evidence = json!([{"source":"observation","polarity":"support","relation":null,"explanation":"实际阅读的观测来源"},
            {"source":"memory_1","polarity":"support","relation":"supports","explanation":"第一份观测记录"},
            {"source":"memory_2","polarity":"support","relation":"corroborates","explanation":"另一份观测记录"}]);
        let range = json!({"domain":"utc","start":0,"end":100});
        let bodies = [
            (
                "semantics",
                json!({"kind":"memory_semantics","content":{"memory":"memory_1","cognitive_role":"declarative","formation_mode":"grounded","epistemic":stance,"grounding_observation":"observation","concepts":["concept"],"qualifications":["仅指所述冷链调查条件"]}}),
            ),
            (
                "concept",
                json!({"kind":"concept","content":{"label":"冷链调查","description":"需要保温运输的海洋采样调查","aliases":["低温样本运输"],"canonical":null,"members":["memory_1"],"lineage":[]}}),
            ),
            (
                "episode",
                json!({"kind":"episode","content":{"title":"阅读调查记录","track":"activity","experience":range,"parent":null,"members":[{"source":"observation","role":"reading","ordinal":0}],"boundary_explanation":"围绕同一调查记录的阅读活动","lineage":[]}}),
            ),
            (
                "journal",
                json!({"kind":"journal","content":{"title":"调查记录日记","narrative":"我阅读了两份带条件的调查记录。","period":range,"perspective":{"class":"narrative","attribution":null,"confidence":null},"points":[{"role":"summary","text":"阅读两份观测记录。","evidence":evidence}],"episodes":["episode"],"lineage":[]}}),
            ),
            (
                "schema",
                json!({"kind":"schema","content":{"title":"带适用条件的冷链调查模式","claim":"所述两次调查保留了低温运输条件。","scope":{"description":"当前两次港湾调查","about":["memory_1"],"concepts":["concept"],"validity":null},"boundary":"不外推到其它样本或运输条件","formation_mode":"synthesized","epistemic":stance,"support":["memory_1","memory_2"],"counterexamples":[],"boundary_cases":[],"lineage":[]}}),
            ),
            (
                "association",
                json!({"kind":"association","content":{"from":"memory_1","to":"concept","relation":"assoc.related","polarity":"positive","basis_class":"source_evidence","description":"观测记录与冷链调查概念相关"}}),
            ),
        ];
        json!({"groups":[{"key":"coastal-understanding","depends_on":[],"changes":bodies.into_iter().map(|(key,body)|json!({"slot_key":key,"body":body,"evidence":evidence})).collect::<Vec<_>>()}]})
    }

    #[test]
    fn six_typed_bodies_compile_exact_refs_slots_and_host_cutoff() {
        let input = input();
        input.validate().unwrap();
        let raw = parse_proposal(&proposal().to_string()).unwrap();
        let group = raw.groups[0].compile(&input).unwrap();
        assert_eq!(group.changes.len(), 6);
        assert_eq!(
            group
                .changes
                .iter()
                .map(|change| change.body.kind())
                .collect::<BTreeSet<_>>(),
            input.slots.iter().map(|slot| slot.kind).collect()
        );
        match &group.changes[0].body {
            d::CognitiveContent::MemorySemantics(value) => {
                assert_eq!(
                    value.memory_id.as_str(),
                    "00000000-0000-0000-0000-000000000001"
                );
                assert_eq!(value.memory_revision, 1);
                assert_eq!(
                    value.concepts[0],
                    input.slots[1].output_reference().unwrap()
                );
                assert_eq!(
                    value.grounding_observation.as_ref().unwrap().as_str(),
                    "00000000-0000-0000-0000-000000000003"
                );
            }
            _ => unreachable!(),
        }
        match &group.changes[3].body {
            d::CognitiveContent::Journal(value) => {
                assert_eq!(value.knowledge_cutoff, input.knowledge_cutoff);
                assert_eq!(
                    value.episodes[0],
                    input.slots[2].output_reference().unwrap()
                );
                assert_eq!(value.points[0].evidence[0].source, input.sources[2].source);
            }
            _ => unreachable!(),
        }
        assert_eq!(group.changes[5].evidence[1].source, input.sources[0].source);
        assert_eq!(parse_proposal(r#"{"groups":[]}"#).unwrap().groups.len(), 0);
    }

    #[test]
    fn bad_group_references_stay_local_while_duplicate_groups_slots_and_cycles_fail_globally() {
        let input = input();
        let mut raw: RawStructuredProposal = serde_json::from_value(proposal()).unwrap();
        let mut bad = raw.groups[0].clone();
        bad.key = "independent-bad-key".into();
        bad.changes = vec![bad.changes.pop().unwrap()];
        if let RawCognitiveContent::Association(content) = &mut bad.changes[0].body {
            content.to = ReferenceKey("invented-uuid-00000000-0000-0000-0000-000000009999".into());
        }
        raw.groups[0].changes.pop();
        raw.groups.push(bad);
        raw.validate_envelope().unwrap();
        assert!(raw.groups[0].compile(&input).is_ok());
        assert!(raw.groups[1].compile(&input).is_err());
        let mut duplicate = raw.clone();
        duplicate.groups[1].key = duplicate.groups[0].key.clone();
        assert!(duplicate.validate_envelope().is_err());
        let mut duplicate = raw.clone();
        duplicate.groups[1].changes[0].slot_key = "concept".into();
        assert!(duplicate.validate_envelope().is_err());
        let mut cycle = raw;
        cycle.groups[0].depends_on = vec![cycle.groups[1].key.clone()];
        assert!(cycle.validate_envelope().is_err());
        assert!(parse_proposal(r#"{"groups":[],"sql":"SELECT 1"}"#).is_err());
    }

    #[test]
    fn actual_schema_binds_available_keys_and_slot_body_kinds() {
        let input = input();
        let schema = structured_output_schema(&input).unwrap();
        assert_eq!(
            schema["$defs"]["SourceKey"]["enum"]
                .as_array()
                .unwrap()
                .len(),
            9
        );
        assert_eq!(
            schema["$defs"]["MemoryKey"]["enum"],
            json!(["memory_1", "memory_2"])
        );
        let choices = schema["$defs"]["RawProposalChange"]["anyOf"]
            .as_array()
            .unwrap();
        assert_eq!(choices.len(), 6);
        for (slot, choice) in input.slots.iter().zip(choices) {
            assert_eq!(choice["properties"]["slot_key"]["const"], slot.key);
            assert_eq!(
                choice["properties"]["body"]["properties"]["kind"]["const"],
                json!(slot.kind)
            );
        }
        assert!(schema["$defs"].get("CognitiveRef").is_none());
        assert!(schema["$defs"].get("CognitiveId").is_none());
        let mut no_observation = input;
        no_observation
            .sources
            .retain(|source| !matches!(source.source, d::EvidenceSource::Observation { .. }));
        assert_eq!(
            structured_output_schema(&no_observation).unwrap()["$defs"]["RawMemorySemantics"]["properties"]
                ["grounding_observation"],
            json!({"type":"null"})
        );
    }

    #[tokio::test]
    async fn controlled_sdk_call_freezes_prompt_schema_input_and_keeps_unadopted_proposals() {
        const TOKEN: &str = "controlled-structured-provider-credential-00000001";
        let calls = Arc::new(AtomicUsize::new(0));
        let counted = calls.clone();
        let provider=Router::new().route("/v1/chat/completions",post(move |headers:HeaderMap,Json(request):Json<Value>| {
            let counted=counted.clone();async move {
                counted.fetch_add(1,Ordering::SeqCst);
                assert_eq!(headers["authorization"],format!("Bearer {TOKEN}"));
                assert_eq!(request["model"],"private-structured-alias");
                assert_eq!(request["response_format"]["json_schema"]["name"],"nous_structured_formation_v1");
                assert_eq!(request["thinking"]["type"],"disabled");
                assert!(request["messages"].to_string().contains("source_event_kind"));
                Json(json!({"id":"controlled-structured","object":"chat.completion","created":0,"model":"actual-structured-model",
                    "choices":[{"index":0,"message":{"role":"assistant","content":proposal().to_string()},"finish_reason":"stop"}],
                    "usage":{"prompt_tokens":100,"completion_tokens":200,"total_tokens":300}}))
            }
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}/v1/", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            axum::serve(listener, provider).await.unwrap();
        });
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../temp");
        std::fs::create_dir_all(&root).unwrap();
        let temporary = tempfile::Builder::new()
            .prefix("structured-client-")
            .tempdir_in(root)
            .unwrap();
        let paths = InstancePaths::new(temporary.path()).unwrap();
        paths.ensure_layout().unwrap();
        let credential = SecretRef {
            file: "credentials/structured-test.token".into(),
        };
        std::fs::write(paths.resolve(&credential.file).unwrap(), TOKEN).unwrap();
        let config:ModelConfig=serde_json::from_value(json!({"structured_formation":{
            "resource":{"id":"structured-test","provider":"test","protocol":"openai_chat_completions","endpoint":endpoint,"model":"private-structured-alias","credential":credential},
            "native_parameters":{"thinking":{"type":"disabled"}}
        }})).unwrap();
        let runtime = ModelRuntime::new(
            &paths,
            &config,
            reqwest::Client::builder()
                .no_proxy()
                .connect_timeout(Duration::from_secs(1))
                .timeout(Duration::from_secs(3))
                .build()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(runtime.structured_inspect()["state"], "ready");
        let prepared = runtime.prepare_structured(input()).unwrap();
        let serialized = serde_json::to_string(&prepared.metadata).unwrap();
        assert!(
            !serialized.contains(TOKEN)
                && !serialized.contains("credentials/structured-test.token")
        );
        assert_eq!(
            prepared.metadata.output_schema_sha256,
            format!(
                "{:x}",
                Sha256::digest(serde_json::to_vec(&prepared.metadata.output_schema).unwrap())
            )
        );
        let frozen = prepared.metadata.input.clone();
        let execution = runtime.execute_structured(prepared).await;
        assert_eq!(
            execution.result.unwrap().groups[0]
                .compile(&frozen)
                .unwrap()
                .changes
                .len(),
            6
        );
        assert_eq!(execution.record.status, "succeeded");
        assert_eq!(
            execution.record.provider_model.as_deref(),
            Some("actual-structured-model")
        );
        assert_eq!(
            execution.record.usage.as_ref().unwrap()["total_tokens"],
            300
        );
        assert!(
            execution.record.provider_response.is_some() && execution.record.raw_output.is_some()
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(
            !paths.database.exists(),
            "model client created a second persistence owner"
        );
        server.abort();
    }
}
