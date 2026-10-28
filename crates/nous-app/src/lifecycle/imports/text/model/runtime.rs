use super::{
    FactualExtraction, ImportSemanticInput, ImportTimeInput, NarrativeExtraction, TimeExtraction,
};
use nous_domain::{Error, ErrorCode, SourceReadRef};
use nous_foundation::{configuration::ConfigKey, models::*, prompts::PromptDefinition};
use schemars::JsonSchema;
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};

pub const NARRATIVE: &str = "import_narrative";
pub const FACTUAL: &str = "import_factual";
pub const TIME: &str = "import_time";

pub fn definitions() -> Vec<RoleDefinition> {
    vec![
        RoleDefinition {
            id: NARRATIVE,
            binding_key: ConfigKey::new("models.roles.import_narrative"),
            requirements: ModelRequirements::structured_text(),
            prompt: Some(PromptDefinition {
                id: NARRATIVE,
                template: include_str!("../../../../prompts/imports/narrative.md"),
            }),
            contract: RoleContract::new::<ImportSemanticInput>(|input| {
                semantic_draft::<NarrativeExtraction>(NARRATIVE, input)
            }),
        },
        RoleDefinition {
            id: FACTUAL,
            binding_key: ConfigKey::new("models.roles.import_factual"),
            requirements: ModelRequirements::structured_text(),
            prompt: Some(PromptDefinition {
                id: FACTUAL,
                template: include_str!("../../../../prompts/imports/factual.md"),
            }),
            contract: RoleContract::new::<ImportSemanticInput>(|input| {
                semantic_draft::<FactualExtraction>(FACTUAL, input)
            }),
        },
        RoleDefinition {
            id: TIME,
            binding_key: ConfigKey::new("models.roles.import_time"),
            requirements: ModelRequirements::structured_text(),
            prompt: Some(PromptDefinition {
                id: TIME,
                template: include_str!("../../../../prompts/imports/time.md"),
            }),
            contract: RoleContract::new::<ImportTimeInput>(time_draft),
        },
    ]
}

fn semantic_draft<T: JsonSchema>(
    name: &str,
    input: ImportSemanticInput,
) -> nous_domain::Result<InvocationDraft> {
    input.validate()?;
    draft::<T>(
        name,
        json!({"unit_id":input.unit_id,"text":input.text,"context":input.context}),
        input.source_refs,
    )
}
fn time_draft(input: ImportTimeInput) -> nous_domain::Result<InvocationDraft> {
    input.validate()?;
    draft::<TimeExtraction>(
        TIME,
        json!({"unit_id":input.unit_id,"text":input.text,"context":input.context,"reference_time":input.reference_time}),
        input.source_refs,
    )
}
fn draft<T: JsonSchema>(
    name: &str,
    content: Value,
    sources: Vec<SourceReadRef>,
) -> nous_domain::Result<InvocationDraft> {
    Ok(InvocationDraft::chat(
        Default::default(),
        vec![ModelMessage::user(content.to_string())],
        OutputContract::JsonSchema {
            name: name.into(),
            schema: serde_json::to_value(schemars::schema_for!(T))?,
        },
    )
    .with_sources(sources))
}

pub async fn prepare_semantic(
    plan: &InvocationPlan,
    attempt: AttemptContext,
    input: ImportSemanticInput,
) -> Result<PreparedCall<InvocationMetadata<ImportSemanticInput>>, ModelFailure> {
    plan.prepare(attempt, input, InvocationEdits::default())
        .await
}
pub async fn prepare_time(
    plan: &InvocationPlan,
    attempt: AttemptContext,
    input: ImportTimeInput,
) -> Result<PreparedCall<InvocationMetadata<ImportTimeInput>>, ModelFailure> {
    plan.prepare(attempt, input, InvocationEdits::default())
        .await
}

pub async fn narrative(
    runtime: &ModelRuntime,
    prepared: PreparedCall<InvocationMetadata<ImportSemanticInput>>,
) -> ModelExecution<InvocationMetadata<ImportSemanticInput>, NarrativeExtraction> {
    runtime
        .execute(prepared)
        .await
        .map(|output, _| parse(output, NarrativeExtraction::validate))
}
pub async fn factual(
    runtime: &ModelRuntime,
    prepared: PreparedCall<InvocationMetadata<ImportSemanticInput>>,
) -> ModelExecution<InvocationMetadata<ImportSemanticInput>, FactualExtraction> {
    runtime
        .execute(prepared)
        .await
        .map(|output, _| parse(output, FactualExtraction::validate))
}
pub async fn time(
    runtime: &ModelRuntime,
    prepared: PreparedCall<InvocationMetadata<ImportTimeInput>>,
) -> ModelExecution<InvocationMetadata<ImportTimeInput>, TimeExtraction> {
    runtime
        .execute(prepared)
        .await
        .map(|output, _| parse(output, TimeExtraction::validate))
}
fn parse<T: DeserializeOwned + Serialize>(
    output: ModelOutput,
    validate: fn(&T) -> nous_domain::Result<()>,
) -> Result<T, ModelFailure> {
    let ModelOutput::Text(text) = output else {
        return Err(ModelFailure::domain(Error::invalid(
            "import extraction requires structured text output",
        )));
    };
    if text.len() > 256 * 1024 {
        return Err(ModelFailure::domain(Error::new(
            ErrorCode::ResourceExhausted,
            "import extraction exceeds 256 KiB output budget",
        )));
    }
    let value: T = serde_json::from_str(&text).map_err(|error| {
        ModelFailure::domain(
            Error::invalid("import model output differs from its typed contract")
                .with_cause(&error),
        )
    })?;
    validate(&value).map_err(ModelFailure::domain)?;
    Ok(value)
}
