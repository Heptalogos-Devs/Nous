use super::{
    MAX_OUTPUT_BYTES, RawStructuredProposal, StructuredFormationInput, structured_output_schema,
};
use nous_domain::{CognitiveKind, Error, ErrorCode, EvidenceSource};
use nous_foundation::configuration::ConfigKey;
use nous_foundation::models::AttemptContext;
use nous_foundation::models::InvocationDraft;
use nous_foundation::models::InvocationEdits;
use nous_foundation::models::InvocationMetadata;
use nous_foundation::models::InvocationPlan;
use nous_foundation::models::ModelExecution;
use nous_foundation::models::ModelFailure;
use nous_foundation::models::ModelMessage;
use nous_foundation::models::ModelOutput;
use nous_foundation::models::ModelRequirements;
use nous_foundation::models::ModelRuntime;
use nous_foundation::models::OutputContract;
use nous_foundation::models::PreparedCall;
use nous_foundation::models::RoleContract;
use nous_foundation::models::RoleDefinition;
use nous_foundation::prompts::PromptArgument;
use nous_foundation::prompts::PromptArguments;
use nous_foundation::prompts::PromptDefinition;
use serde_json::json;

const TASKS: [(CognitiveKind, &str, &str, &str); 6] = [
    (
        CognitiveKind::Concept,
        "concept_formation",
        "models.roles.concept_formation",
        include_str!("../../../prompts/formation/concept.md"),
    ),
    (
        CognitiveKind::MemorySemantics,
        "memory_semantics",
        "models.roles.memory_semantics",
        include_str!("../../../prompts/formation/memory-semantics.md"),
    ),
    (
        CognitiveKind::Episode,
        "episode_formation",
        "models.roles.episode_formation",
        include_str!("../../../prompts/formation/episode.md"),
    ),
    (
        CognitiveKind::Schema,
        "schema_formation",
        "models.roles.schema_formation",
        include_str!("../../../prompts/formation/schema.md"),
    ),
    (
        CognitiveKind::Journal,
        "journal_formation",
        "models.roles.journal_formation",
        include_str!("../../../prompts/formation/journal.md"),
    ),
    (
        CognitiveKind::Association,
        "association_formation",
        "models.roles.association_formation",
        include_str!("../../../prompts/formation/association.md"),
    ),
];

pub fn role(kind: CognitiveKind) -> nous_domain::Result<&'static str> {
    TASKS
        .iter()
        .find(|task| task.0 == kind)
        .map(|task| task.1)
        .ok_or_else(|| Error::invalid("unsupported structured formation kind"))
}

pub fn definitions() -> impl Iterator<Item = RoleDefinition> {
    TASKS
        .into_iter()
        .map(|(kind, id, key, template)| RoleDefinition {
            id,
            binding_key: ConfigKey::new(key),
            requirements: ModelRequirements::structured_text(),
            prompt: Some(PromptDefinition { id, template }),
            contract: RoleContract::new::<StructuredFormationInput>(move |input| {
                draft(kind, input)
            }),
        })
}

fn draft(
    kind: CognitiveKind,
    input: StructuredFormationInput,
) -> nous_domain::Result<InvocationDraft> {
    input.validate()?;
    if input.slots.iter().any(|slot| slot.kind != kind) {
        return Err(Error::invalid(
            "one formation task handles one cognition kind",
        ));
    }
    let schema =
        structured_output_schema(&input).map_err(|error| Error::internal(error.to_string()))?;
    let requirements = input
        .sources
        .iter()
        .map(|entry| nous_domain::SourceReadRef::evidence(&entry.source))
        .collect();
    let sources = input
        .sources
        .iter()
        .map(|entry| {
            let source_kind = match &entry.source {
                EvidenceSource::MemoryRevision { .. } => json!("memory"),
                EvidenceSource::CognitiveRevision { reference } => json!(reference.kind),
                EvidenceSource::Observation { .. } => json!("observation"),
                EvidenceSource::Material { .. } => json!("material"),
                EvidenceSource::Media { .. } => json!("media"),
            };
            json!({"key":entry.key,"kind":source_kind,"text":entry.text,
            "source_event_kind":entry.source_event_kind,"truncated":entry.truncated})
        })
        .collect::<Vec<_>>();
    let targets = input.slots.iter().map(|slot| &slot.key).collect::<Vec<_>>();
    let arguments = PromptArguments::from([
        (
            "sources".into(),
            PromptArgument {
                value: json!(sources),
                source: Some("formation.sources".into()),
            },
        ),
        (
            "targets".into(),
            PromptArgument {
                value: json!(targets),
                source: Some("formation.targets".into()),
            },
        ),
        (
            "knowledge_cutoff".into(),
            PromptArgument {
                value: json!(input.knowledge_cutoff),
                source: Some("formation.knowledge_cutoff".into()),
            },
        ),
    ]);
    let user = json!({"sources":sources,"targets":targets}).to_string();
    Ok(InvocationDraft::chat(
        arguments,
        vec![ModelMessage::user(user)],
        OutputContract::JsonSchema {
            name: format!("nous_{}", role(kind)?),
            schema,
        },
    )
    .with_sources(requirements))
}

pub type PreparedStructured = PreparedCall<InvocationMetadata<StructuredFormationInput>>;
pub async fn prepare(
    plan: &InvocationPlan,
    attempt: AttemptContext,
    input: StructuredFormationInput,
) -> Result<PreparedStructured, ModelFailure> {
    plan.prepare(attempt, input, InvocationEdits::default())
        .await
}

pub async fn execute(
    runtime: &ModelRuntime,
    prepared: PreparedStructured,
) -> ModelExecution<InvocationMetadata<StructuredFormationInput>, RawStructuredProposal> {
    runtime
        .execute(prepared)
        .await
        .map(|output, _| match output {
            ModelOutput::Text(text) => parse_proposal(&text),
            _ => Err(ModelFailure::domain(Error::internal(
                "structured formation requires chat output",
            ))),
        })
}

fn parse_proposal(text: &str) -> Result<RawStructuredProposal, ModelFailure> {
    if text.len() > MAX_OUTPUT_BYTES {
        return Err(ModelFailure::domain(Error::new(
            ErrorCode::ResourceExhausted,
            "structured proposal exceeds output byte budget",
        )));
    }
    let proposal: RawStructuredProposal = serde_json::from_str(text).map_err(|error| {
        ModelFailure::domain(Error::invalid(
            "model output does not match typed structured proposal contract",
        ))
        .with_source(Box::new(error))
    })?;
    proposal.validate_envelope().map_err(ModelFailure::domain)?;
    Ok(proposal)
}
