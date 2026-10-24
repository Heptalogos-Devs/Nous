//! Portable messages, output requirements and complete invocation snapshots.
use super::{ResolvedModel, ResourceMetadata};
use crate::configuration::ConfigSource;
use crate::prompts::PromptSnapshot;
use nous_domain::media::{ChannelAccess, MediaReference};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum OutputContract {
    Text,
    JsonSchema { name: String, schema: Value },
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MessageRole {
    System,
    User,
    Assistant,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SourceAccess {
    pub reference: MediaReference,
    pub channels: ChannelAccess,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum MediaData {
    Base64 { data: String },
    Url { url: String },
}
impl MediaData {
    pub fn from_bytes(bytes: &[u8]) -> Self {
        use base64::Engine;
        Self::Base64 {
            data: base64::engine::general_purpose::STANDARD.encode(bytes),
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum MessagePart {
    Text {
        text: String,
        source: Option<SourceAccess>,
    },
    Media {
        media_type: String,
        data: MediaData,
        source: Option<SourceAccess>,
    },
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ModelMessage {
    pub role: MessageRole,
    pub parts: Vec<MessagePart>,
}
impl ModelMessage {
    pub fn user(text: impl Into<String>) -> Self {
        Self {
            role: MessageRole::User,
            parts: vec![MessagePart::Text {
                text: text.into(),
                source: None,
            }],
        }
    }
    pub fn system(text: impl Into<String>) -> Self {
        Self {
            role: MessageRole::System,
            parts: vec![MessagePart::Text {
                text: text.into(),
                source: None,
            }],
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ModelInput {
    Chat {
        messages: Vec<ModelMessage>,
        output: OutputContract,
    },
    Embedding {
        texts: Vec<String>,
    },
}
#[derive(Debug, Clone)]
pub enum ModelOutput {
    Text(String),
    Vectors(Vec<Vec<f32>>),
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct InvocationEdits {
    /// A temporary instruction override; persistent revisions belong to Prompts.
    pub prompt: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutePreview {
    pub route: String,
    pub resource: ResourceMetadata,
    pub parameters: super::RouteParameters,
    pub actual_access: ChannelAccess,
    pub rejection: Option<nous_domain::Error>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InvocationPreview {
    pub role: String,
    pub config_digest: String,
    pub prompt: Option<PromptSnapshot>,
    pub input: ModelInput,
    pub output_schema_sha256: Option<String>,
    pub edits: InvocationEdits,
    pub routes: Vec<RoutePreview>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InvocationMetadata<T> {
    pub call_id: String,
    pub role: String,
    pub resource: ResourceMetadata,
    pub route: String,
    pub config_digest: String,
    pub config_sources: BTreeMap<String, ConfigSource>,
    pub input: T,
    pub preview: InvocationPreview,
    pub actual_access: ChannelAccess,
    pub plan: ResolvedModel,
}
impl<T: Serialize> super::CallMetadata for InvocationMetadata<T> {
    fn role(&self) -> &str {
        &self.role
    }
    fn reservation_fields(&self) -> (&str, &ResourceMetadata) {
        (&self.call_id, &self.resource)
    }
}
