//! Memory formation declares its role, source input, prompt arguments and draft decoder.
use nous_domain::media::{ChannelAccess, ChannelMode, MediaReference};
use nous_domain::{Error, MaterialId, MemoryDraft, NamespaceId, ObservationId, SubjectId};
use nous_foundation::configuration::ConfigKey;
use nous_foundation::models::AttemptContext;
use nous_foundation::models::InvocationDraft;
use nous_foundation::models::InvocationMetadata;
use nous_foundation::models::InvocationPlan;
use nous_foundation::models::MessagePart;
use nous_foundation::models::MessageRole;
use nous_foundation::models::ModelExecution;
use nous_foundation::models::ModelFailure;
use nous_foundation::models::ModelMessage;
use nous_foundation::models::ModelOutput;
use nous_foundation::models::ModelRequirements;
use nous_foundation::models::ModelRuntime;
use nous_foundation::models::OutputContract;
use nous_foundation::models::PreparedCall;
use nous_foundation::models::RoleBinding;
use nous_foundation::models::RoleContract;
use nous_foundation::models::RoleDefinition;
use nous_foundation::models::SourceAccess;
use nous_foundation::prompts::PromptArgument;
use nous_foundation::prompts::PromptArguments;
use nous_foundation::prompts::PromptDefinition;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;
pub const ROLE: &str = "memory_formation";
pub const BINDING: ConfigKey<Option<RoleBinding>> = ConfigKey::new("models.roles.memory_formation");
pub fn definition() -> RoleDefinition {
    RoleDefinition {
        id: ROLE,
        binding_key: BINDING,
        requirements: ModelRequirements::structured_text(),
        contract: RoleContract::new::<FormationInput>(draft),
        prompt: Some(PromptDefinition {
            id: ROLE,
            template: include_str!("../../prompts/memory-formation.md"),
        }),
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FormationInput {
    pub namespace_id: NamespaceId,
    pub subject_id: SubjectId,
    pub material_id: MaterialId,
    pub material_commit_seq: i64,
    pub observation_id: ObservationId,
    pub observation_commit_seq: i64,
    #[serde(default)]
    pub source_event_kind: nous_domain::ObservationKind,
    pub text: String,
}
pub type PreparedFormation = PreparedCall<InvocationMetadata<FormationInput>>;
fn draft(input: FormationInput) -> nous_domain::Result<InvocationDraft> {
    if input.text.trim().is_empty()
        || input.material_commit_seq <= 0
        || input.observation_commit_seq <= 0
    {
        return Err(Error::invalid(
            "formation requires source text and positive source versions",
        ));
    }
    let arguments = PromptArguments::from([
        (
            "material_text".into(),
            PromptArgument {
                value: json!(input.text),
                source: Some(format!(
                    "material:{}@{}",
                    input.material_id, input.material_commit_seq
                )),
            },
        ),
        (
            "source_event_kind".into(),
            PromptArgument {
                value: json!(input.source_event_kind),
                source: Some(format!(
                    "observation:{}@{}",
                    input.observation_id, input.observation_commit_seq
                )),
            },
        ),
    ]);
    let user =
        json!({"material_text":input.text,"source_event_kind":input.source_event_kind}).to_string();
    let message = ModelMessage {
        role: MessageRole::User,
        parts: vec![MessagePart::Text {
            text: user,
            source: Some(SourceAccess {
                reference: MediaReference::Original {
                    material_id: input.material_id,
                },
                channels: ChannelAccess {
                    visual: ChannelMode::Unavailable,
                    audio: ChannelMode::Unavailable,
                    source_text: ChannelMode::Original,
                },
            }),
        }],
    };
    Ok(InvocationDraft::chat(
        arguments,
        vec![message],
        OutputContract::JsonSchema {
            name: "nous_memory_formation".into(),
            schema: super::decoder::output_schema(),
        },
    ))
}
pub async fn prepare(
    plan: &InvocationPlan,
    attempt: AttemptContext,
    input: FormationInput,
) -> Result<PreparedFormation, ModelFailure> {
    plan.prepare(attempt, input, Default::default()).await
}
pub async fn execute(
    runtime: &ModelRuntime,
    prepared: PreparedFormation,
) -> ModelExecution<InvocationMetadata<FormationInput>, Vec<MemoryDraft>> {
    runtime
        .execute(prepared)
        .await
        .map(|output, _| match output {
            ModelOutput::Text(text) => super::decoder::parse_drafts(&text),
            _ => Err(ModelFailure::domain(Error::internal(
                "memory formation requires chat output",
            ))),
        })
}
