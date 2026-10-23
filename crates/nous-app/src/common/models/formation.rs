//! Basic Memory formation owns its source input and bounded draft contract.
use super::{
    ModelCallRecord, ModelExecution, ModelFailure, ModelResource, ModelRuntime, PreparedCall,
    PromptBinding, PromptSnapshot, ResourceMetadata, RetryClass,
    execution::{CallMetadata, PreparedRequest, ProviderOutput},
    resource::validate_chat,
};
use anyhow::Result as ConfigResult;
use genai::chat::{ChatMessage, ChatOptions, ChatRequest, JsonSpec};
use nous_domain::{
    ErrorCode, MaterialId, MemoryDraft, ModelRole, NamespaceId, ObservationId, SubjectId,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FormationConfig {
    pub resource: ModelResource,
    #[serde(default)]
    pub prompt: PromptBinding,
    #[serde(default = "input_limit")]
    pub max_input_bytes: usize,
    #[serde(default = "output_limit")]
    pub max_output_tokens: u32,
    #[serde(default = "daily_limit")]
    pub max_calls_per_day: u32,
    #[serde(default)]
    pub native_parameters: BTreeMap<String, Value>,
}
fn input_limit() -> usize {
    32 * 1024
}
fn output_limit() -> u32 {
    1200
}
fn daily_limit() -> u32 {
    20
}
impl FormationConfig {
    pub fn validate(&self) -> ConfigResult<()> {
        validate_chat(
            &self.resource,
            self.max_input_bytes,
            self.max_output_tokens,
            self.max_calls_per_day,
            &self.native_parameters,
        )
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Serialize)]
pub struct ModelCallMetadata {
    pub call_id: String,
    pub role: ModelRole,
    pub resource: ResourceMetadata,
    pub prompt: PromptSnapshot,
    pub input: FormationInput,
    pub rendered_user: String,
    pub parameters: Value,
    pub max_calls_per_day: u32,
}
impl CallMetadata for ModelCallMetadata {
    fn reservation_fields(&self) -> (&str, &str, u32) {
        (&self.call_id, &self.resource.id, self.max_calls_per_day)
    }
}
pub type PreparedFormation = PreparedCall<ModelCallMetadata>;
pub type FormationCallRecord = ModelCallRecord<ModelCallMetadata>;
pub type FormationExecution = ModelExecution<ModelCallMetadata, Vec<MemoryDraft>>;

impl ModelRuntime {
    pub fn memory_inspect(&self) -> Value {
        let Some(config) = &self.config.memory_formation else {
            return json!({"state":"waiting_configuration","reason":"memory formation resource is not configured"});
        };
        let mut value = self.resources.inspect(&config.resource);
        match self.prompts.read("memory_formation", &config.prompt) {
            Ok(prompt) => {
                value["prompt"] = json!(prompt);
            }
            Err(error) => {
                value["state"] = json!("waiting_configuration");
                value["reason"] = json!(error.message);
            }
        }
        value["max_input_bytes"] = json!(config.max_input_bytes);
        value["max_output_tokens"] = json!(config.max_output_tokens);
        value["max_calls_per_day"] = json!(config.max_calls_per_day);
        value["native_parameters"] = json!(config.native_parameters);
        value["cost"] = Value::Null;
        value
    }
    /// Prepare first, reserve this invocation in the shared ledger, then execute.
    pub fn prepare_memory(&self, input: FormationInput) -> Result<PreparedFormation, ModelFailure> {
        let config =
            self.config.memory_formation.as_ref().ok_or_else(|| {
                ModelFailure::waiting("memory formation resource is not configured")
            })?;
        if input.text.trim().is_empty()
            || input.material_commit_seq <= 0
            || input.observation_commit_seq <= 0
        {
            return Err(ModelFailure::new(
                RetryClass::Permanent,
                ErrorCode::InvalidArgument,
                "formation requires nonempty source text and positive source versions",
            ));
        }
        let arguments = super::PromptArguments::from([
            (
                "material_text".into(),
                super::PromptArgument {
                    value: json!(input.text),
                    source: Some(format!(
                        "material:{}@{}",
                        input.material_id, input.material_commit_seq
                    )),
                },
            ),
            (
                "source_event_kind".into(),
                super::PromptArgument {
                    value: json!(input.source_event_kind),
                    source: Some(format!(
                        "observation:{}@{}",
                        input.observation_id, input.observation_commit_seq
                    )),
                },
            ),
        ]);
        let prompt = self
            .prompts
            .render("memory_formation", &config.prompt, &arguments)
            .map_err(ModelFailure::prompt)?;
        let rendered_user = serde_json::to_string(
            &json!({"material_text":input.text,"source_event_kind":input.source_event_kind}),
        )
        .expect("formation source is serializable");
        if prompt.text.len() + rendered_user.len() > config.max_input_bytes {
            return Err(ModelFailure::new(
                RetryClass::Permanent,
                ErrorCode::ResourceExhausted,
                "rendered formation input exceeds max_input_bytes",
            ));
        }
        let target = self
            .resources
            .resolve(
                nous_domain::ModelRole::MemoryFormation,
                &super::ModelRequirements::structured_text(),
            )?
            .into_target();
        let request = ChatRequest::new(vec![ChatMessage::user(rendered_user.clone())])
            .with_system(prompt.text.clone());
        let mut options = ChatOptions::default()
            .with_max_tokens(config.max_output_tokens)
            .with_capture_raw_body(true)
            .with_response_format(JsonSpec::new("nous_memory_formation", output_schema()));
        if !config.native_parameters.is_empty() {
            options = options.with_extra_body(json!(config.native_parameters));
        }
        let metadata = ModelCallMetadata {
            call_id: uuid::Uuid::new_v4().to_string(),
            role: ModelRole::MemoryFormation,
            resource: (&config.resource).into(),
            prompt,
            input,
            rendered_user,
            parameters: json!({"max_output_tokens":config.max_output_tokens,"max_input_bytes":config.max_input_bytes,
                "response_format":"json_schema","native_parameters":config.native_parameters,"genai_chat_options":options}),
            max_calls_per_day: config.max_calls_per_day,
        };
        Ok(PreparedCall {
            metadata,
            target,
            request: PreparedRequest::Chat {
                request,
                options: Box::new(options),
            },
        })
    }
    pub async fn execute_memory(&self, prepared: PreparedFormation) -> FormationExecution {
        self.executor
            .invoke(prepared)
            .await
            .map(|output, _| match output {
                ProviderOutput::Text(text) => parse_drafts(&text),
                _ => Err(ModelFailure::new(
                    RetryClass::Permanent,
                    ErrorCode::Internal,
                    "memory formation requires chat output",
                )),
            })
    }
}
fn output_schema() -> Value {
    json!({"type":"object","additionalProperties":false,"required":["memories"],"properties":{
        "memories":{"type":"array","minItems":1,"maxItems":3,"items":{"type":"object","additionalProperties":false,
            "required":["title","content","epistemic_status"],"properties":{
                "title":{"type":"string","minLength":1,"maxLength":256},
                "content":{"type":"string","minLength":1,"maxLength":16384},
                "epistemic_status":{"type":"string","enum":["reported","inferred","uncertain"]}}}}}})
}

fn parse_drafts(text: &str) -> Result<Vec<MemoryDraft>, ModelFailure> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Output {
        memories: Vec<Draft>,
    }
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Draft {
        title: String,
        content: String,
        epistemic_status: String,
    }
    let output: Output = serde_json::from_str(text).map_err(|error| {
        let failure = ModelFailure::new(
            RetryClass::Permanent,
            ErrorCode::InvalidArgument,
            "model output does not match the formation JSON contract",
        );
        failure.with_source(Box::new(error))
    })?;
    if !(1..=3).contains(&output.memories.len())
        || output.memories.iter().any(|draft| {
            draft.title.trim().is_empty()
                || draft.title.chars().count() > 256
                || draft.content.trim().is_empty()
                || draft.content.chars().count() > 16384
                || !matches!(
                    draft.epistemic_status.as_str(),
                    "reported" | "inferred" | "uncertain"
                )
        })
    {
        return Err(ModelFailure::new(
            RetryClass::Permanent,
            ErrorCode::InvalidArgument,
            "model output must contain 1 to 3 bounded memories with valid epistemic status",
        ));
    }
    Ok(output
        .memories
        .into_iter()
        .map(|draft| MemoryDraft {
            title: draft.title,
            content: draft.content,
            epistemic_status: draft.epistemic_status,
        })
        .collect())
}
