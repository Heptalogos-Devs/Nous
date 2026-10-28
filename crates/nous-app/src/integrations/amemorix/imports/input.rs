use nous_domain::*;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ImportInput {
    pub action: String,
    #[serde(flatten)]
    pub parameters: Map<String, Value>,
}
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Create {
    pub operation_id: Option<String>,
    pub content: Option<String>,
    pub name: Option<String>,
    #[serde(default)]
    pub inputs: Vec<ImportFileDocument>,
    pub input_mode: Option<String>,
    pub llm_enabled: Option<bool>,
    pub strategy_override: Option<ImportTextStrategy>,
    #[serde(default)]
    pub chat_log: bool,
    pub chat_reference_time: Option<String>,
    pub reference_time: Option<TimePoint>,
    pub scope_type: Option<String>,
    #[serde(default)]
    pub chat_id: String,
    pub vectors: Option<ImportVectorPolicy>,
    pub dedupe_policy: Option<String>,
    #[serde(default)]
    pub force: bool,
    #[serde(default)]
    pub clear_manifest: bool,
    pub file_concurrency: Option<usize>,
    pub chunk_concurrency: Option<usize>,
    pub narrative_window_size: Option<usize>,
    pub narrative_overlap: Option<usize>,
    pub factual_target_size: Option<usize>,
}
impl Create {
    pub fn overrides(&self) -> Result<ImportTextSplitOverrides> {
        let reference = self
            .chat_reference_time
            .as_deref()
            .map(|value| {
                let parsed = chrono::DateTime::parse_from_rfc3339(value).map_err(|error| {
                    Error::invalid("chat_reference_time requires an explicit RFC3339 offset")
                        .with_cause(&error)
                })?;
                Ok::<_, Error>(TimePoint {
                    domain: "unix_ms".into(),
                    coordinate: parsed.timestamp_millis(),
                })
            })
            .transpose()?;
        Ok(ImportTextSplitOverrides {
            strategy: if self.chat_log {
                Some(ImportTextStrategy::ChatLog)
            } else {
                self.strategy_override
            },
            narrative_window_size: self.narrative_window_size,
            narrative_overlap: self.narrative_overlap,
            factual_target_size: self.factual_target_size,
            reference_time: self.reference_time.clone().or(reference),
        })
    }
    pub fn origin(&self) -> Result<Option<ObservationOrigin>> {
        let scope = self
            .scope_type
            .as_deref()
            .unwrap_or(if self.chat_id.is_empty() {
                "global"
            } else {
                "chat"
            });
        match (scope, self.chat_id.trim()) {
            ("global", "") => Ok(None),
            ("chat", value) if !value.is_empty() => {
                ObservationOrigin::from_refs(Some(ConversationRef::new(value.to_owned())?), None)
            }
            _ => Err(Error::invalid(
                "import scope_type and chat_id do not describe the same scope",
            )),
        }
    }
}
