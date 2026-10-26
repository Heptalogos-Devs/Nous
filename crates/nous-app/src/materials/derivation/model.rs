use super::{input::MediaModelInput, output::RawInterpretation};
use nous_domain::{media::*, *};
use nous_foundation::{
    configuration::ConfigKey,
    models::*,
    prompts::{PromptArgument, PromptArguments, PromptDefinition},
};
use serde_json::json;

pub const DESCRIBE: &str = "media_description";
pub const DIRECT: &str = "media_direct_structuring";
pub const STRUCTURE: &str = "media_structuring";
pub fn definitions() -> Vec<RoleDefinition> {
    vec![
        RoleDefinition {
            id: DESCRIBE,
            binding_key: ConfigKey::<Option<RoleBinding>>::new("models.roles.media_description"),
            requirements: ModelRequirements {
                input_modalities: vec![ModelModality::Text],
                capabilities: vec![ModelCapability::TextGeneration],
            },
            prompt: Some(PromptDefinition {
                id: DESCRIBE,
                template: include_str!("../../prompts/media/describe.md"),
            }),
            contract: RoleContract::new::<MediaModelInput>(description_draft),
        },
        RoleDefinition {
            id: DIRECT,
            binding_key: ConfigKey::<Option<RoleBinding>>::new(
                "models.roles.media_direct_structuring",
            ),
            requirements: ModelRequirements::structured_text(),
            prompt: Some(PromptDefinition {
                id: DIRECT,
                template: include_str!("../../prompts/media/direct.md"),
            }),
            contract: RoleContract::new::<MediaModelInput>(structured_draft),
        },
        RoleDefinition {
            id: STRUCTURE,
            binding_key: ConfigKey::<Option<RoleBinding>>::new("models.roles.media_structuring"),
            requirements: ModelRequirements::structured_text(),
            prompt: Some(PromptDefinition {
                id: STRUCTURE,
                template: include_str!("../../prompts/media/structure.md"),
            }),
            contract: RoleContract::new::<MediaModelInput>(structured_draft),
        },
    ]
}
fn arguments(input: &MediaModelInput) -> PromptArguments {
    PromptArguments::from([(
        "source_context".into(),
        PromptArgument {
            value: json!(json!({"duration_ms":input.quality.source_duration_ms,"partial":input.quality.partial,
                "sampling":input.quality.sampling.as_ref().map(|sampling| json!({"sampled_duration_ms":sampling.sampled_duration_ms,
                    "sampled_timestamps_ms":sampling.sampled_timestamps_ms,"omitted_timestamps_ms":sampling.omitted_timestamps_ms,
                    "audio_included":sampling.audio_included,"audio_omitted":sampling.audio_omitted}))}).to_string()),
            source: Some("committed media preparation coverage".into()),
        },
    )])
}
fn description_draft(input: MediaModelInput) -> Result<InvocationDraft> {
    input.validate()?;
    Ok(InvocationDraft::chat(
        arguments(&input),
        vec![ModelMessage {
            role: MessageRole::User,
            parts: input.messages(),
        }],
        OutputContract::Text,
    )
    .with_sources(
        input
            .catalog
            .iter()
            .map(|entry| SourceReadRef::Media {
                reference: entry.source.clone(),
                observation_id: None,
            })
            .collect(),
    ))
}
fn structured_draft(input: MediaModelInput) -> Result<InvocationDraft> {
    input.validate()?;
    let access = input.access();
    if input.strategy != DerivationStrategy::DescriptionOnly
        && ![access.visual, access.audio, access.source_text].contains(&ChannelMode::Original)
    {
        return Err(Error::invalid(
            "media structuring requires an actually supplied original channel",
        ));
    }
    if input.strategy == DerivationStrategy::DescribeThenStructure
        && !input
            .catalog
            .iter()
            .any(|entry| entry.role == DerivationInputRole::Description)
    {
        return Err(Error::invalid(
            "second-stage structuring requires its committed free description",
        ));
    }
    if !input.catalog.iter().any(|entry| {
        matches!(
            entry.role,
            DerivationInputRole::Source | DerivationInputRole::SampledFrame
        )
    }) {
        return Err(Error::invalid(
            "media structuring requires exact media inputs",
        ));
    }
    Ok(InvocationDraft::chat(
        arguments(&input),
        vec![ModelMessage {
            role: MessageRole::User,
            parts: input.messages(),
        }],
        OutputContract::JsonSchema {
            name: "media_interpretation".into(),
            schema: super::output::schema(&input),
        },
    )
    .with_sources(
        input
            .catalog
            .iter()
            .map(|entry| SourceReadRef::Media {
                reference: entry.source.clone(),
                observation_id: None,
            })
            .collect(),
    ))
}
pub async fn prepare(
    plan: &InvocationPlan,
    attempt: AttemptContext,
    input: MediaModelInput,
) -> std::result::Result<PreparedCall<InvocationMetadata<MediaModelInput>>, ModelFailure> {
    plan.prepare(attempt, input, Default::default()).await
}
pub async fn describe(
    runtime: &ModelRuntime,
    prepared: PreparedCall<InvocationMetadata<MediaModelInput>>,
) -> ModelExecution<InvocationMetadata<MediaModelInput>, String> {
    runtime
        .execute(prepared)
        .await
        .map(|output, _| match output {
            ModelOutput::Text(text) if !text.trim().is_empty() && text.len() <= 1_000_000 => {
                Ok(text)
            }
            _ => Err(ModelFailure::domain(Error::invalid(
                "media description must contain bounded natural-language text",
            ))),
        })
}
pub async fn structure(
    runtime: &ModelRuntime,
    prepared: PreparedCall<InvocationMetadata<MediaModelInput>>,
) -> ModelExecution<InvocationMetadata<MediaModelInput>, MediaInterpretation> {
    runtime.execute(prepared).await.map(|output, metadata| {
        let ModelOutput::Text(text) = output else {
            return Err(ModelFailure::domain(Error::invalid(
                "media structuring requires a JSON response",
            )));
        };
        let raw: RawInterpretation = serde_json::from_str(&text)
            .map_err(|error| ModelFailure::domain(Error::invalid(error.to_string())))?;
        raw.compile(&metadata.input).map_err(ModelFailure::domain)
    })
}
