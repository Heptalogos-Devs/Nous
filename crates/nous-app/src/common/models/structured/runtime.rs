// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0 AND AGPL-3.0-only
// Adapted source commits, digests and licenses: ../structured-sources.json.
use super::super::{
    ModelCallRecord, ModelExecution, ModelFailure, ModelResource, ModelRuntime, PreparedCall,
    PromptBinding, PromptSnapshot, ResourceMetadata, RetryClass,
    execution::{CallMetadata, PreparedRequest, ProviderOutput},
    resource::validate_chat,
};
use super::{
    MAX_CHANGES, MAX_GROUPS, MAX_OUTPUT_BYTES, RawStructuredProposal, StructuredFormationInput,
    structured_output_schema,
};
use anyhow::Result as ConfigResult;
use genai::chat::{ChatMessage, ChatOptions, ChatRequest, JsonSpec};
use nous_domain as d;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StructuredFormationConfig {
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
        validate_chat(
            &self.resource,
            self.max_input_bytes,
            self.max_output_tokens,
            self.max_calls_per_day,
            &self.native_parameters,
        )?;
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct StructuredCallMetadata {
    pub call_id: String,
    pub role: d::ModelRole,
    pub resource: ResourceMetadata,
    pub prompt: PromptSnapshot,
    pub input: StructuredFormationInput,
    pub rendered_user: String,
    pub output_schema_sha256: String,
    pub output_schema: Value,
    pub parameters: Value,
    pub max_calls_per_day: u32,
}
impl CallMetadata for StructuredCallMetadata {
    fn reservation_fields(&self) -> (&str, &str, u32) {
        (&self.call_id, &self.resource.id, self.max_calls_per_day)
    }
}
pub type PreparedStructured = PreparedCall<StructuredCallMetadata>;
pub type StructuredCallRecord = ModelCallRecord<StructuredCallMetadata>;
pub type StructuredExecution = ModelExecution<StructuredCallMetadata, RawStructuredProposal>;

impl ModelRuntime {
    pub fn structured_config(&self) -> Option<&StructuredFormationConfig> {
        self.config.structured_formation.as_ref()
    }
    pub fn structured_inspect(&self) -> Value {
        let Some(config) = self.structured_config() else {
            return json!({"state":"waiting_configuration","reason":"structured formation resource is not configured"});
        };
        let mut value = self.resources.inspect(&config.resource);
        match self.prompts.read("structured_formation", &config.prompt) {
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
        value["max_groups"] = json!(MAX_GROUPS);
        value["max_changes"] = json!(MAX_CHANGES);
        value["cost"] = Value::Null;
        value
    }
    pub fn prepare_structured(
        &self,
        input: StructuredFormationInput,
    ) -> Result<PreparedStructured, ModelFailure> {
        let config = self.structured_config().ok_or_else(|| {
            ModelFailure::waiting("structured formation resource is not configured")
        })?;
        input.validate().map_err(ModelFailure::domain)?;
        let arguments = super::super::PromptArguments::from([
            (
                "input".into(),
                super::super::PromptArgument {
                    value: json!(input),
                    source: Some("structured_formation.input".into()),
                },
            ),
            (
                "sources".into(),
                super::super::PromptArgument {
                    value: json!(input.sources),
                    source: Some("structured_formation.source_catalog".into()),
                },
            ),
            (
                "slots".into(),
                super::super::PromptArgument {
                    value: json!(input.slots),
                    source: Some("structured_formation.target_slots".into()),
                },
            ),
            (
                "knowledge_cutoff".into(),
                super::super::PromptArgument {
                    value: json!(input.knowledge_cutoff),
                    source: Some("structured_formation.knowledge_cutoff".into()),
                },
            ),
        ]);
        let prompt = self
            .prompts
            .render("structured_formation", &config.prompt, &arguments)
            .map_err(ModelFailure::prompt)?;
        let output_schema = structured_output_schema(&input).map_err(|error| {
            ModelFailure::new(
                RetryClass::Permanent,
                d::ErrorCode::Internal,
                "structured output schema is unavailable",
            )
            .with_source(error.into_boxed_dyn_error())
        })?;
        let rendered_user =
            serde_json::to_string(&input).expect("typed structured input is serializable");
        let schema_bytes =
            serde_json::to_vec(&output_schema).expect("output schema is serializable");
        if prompt.text.len() + rendered_user.len() + schema_bytes.len() > config.max_input_bytes {
            return Err(ModelFailure::new(
                RetryClass::Permanent,
                d::ErrorCode::ResourceExhausted,
                "structured prompt, source catalog and output schema exceed max_input_bytes",
            ));
        }
        let target = self
            .resources
            .resolve(
                nous_domain::ModelRole::StructuredFormation,
                &super::super::ModelRequirements::structured_text(),
            )?
            .into_target();
        let mut options = ChatOptions::default()
            .with_max_tokens(config.max_output_tokens)
            .with_capture_raw_body(true)
            .with_response_format(JsonSpec::new(
                "nous_structured_formation",
                output_schema.clone(),
            ));
        if !config.native_parameters.is_empty() {
            options = options.with_extra_body(json!(config.native_parameters));
        }
        let request = ChatRequest::new(vec![ChatMessage::user(rendered_user.clone())])
            .with_system(prompt.text.clone());
        let metadata = StructuredCallMetadata {
            call_id: uuid::Uuid::new_v4().to_string(),
            role: d::ModelRole::StructuredFormation,
            resource: (&config.resource).into(),
            prompt,
            input,
            rendered_user,
            output_schema_sha256: format!("{:x}", Sha256::digest(schema_bytes)),
            output_schema,
            parameters: json!({"max_output_tokens":config.max_output_tokens,"max_input_bytes":config.max_input_bytes,
                "native_parameters":config.native_parameters,"max_groups":MAX_GROUPS,"max_changes":MAX_CHANGES,"max_output_bytes":MAX_OUTPUT_BYTES}),
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
    pub async fn execute_structured(&self, prepared: PreparedStructured) -> StructuredExecution {
        self.executor
            .invoke(prepared)
            .await
            .map(|output, _| match output {
                ProviderOutput::Text(text) => parse_proposal(&text),
                _ => Err(ModelFailure::new(
                    RetryClass::Permanent,
                    d::ErrorCode::Internal,
                    "structured formation requires chat output",
                )),
            })
    }
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
        ModelFailure::new(
            RetryClass::Permanent,
            d::ErrorCode::InvalidArgument,
            "model output does not match the typed structured proposal contract",
        )
        .with_source(Box::new(error))
    })?;
    proposal.validate_envelope().map_err(ModelFailure::domain)?;
    Ok(proposal)
}
