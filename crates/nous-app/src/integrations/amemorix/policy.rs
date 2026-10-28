use nous_domain::*;
use nous_foundation::configuration::{
    ConfigDefinition, ConfigKey, ConfigRegistryBuilder, ConfigWritePolicy,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const AMEMORIX: ConfigKey<AmemorixPolicy> = ConfigKey::new("integrations.amemorix");
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FilterMode {
    #[default]
    Blacklist,
    Whitelist,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct AmemorixPolicy {
    pub imports: super::imports::config::ImportFacadePolicy,
    pub enabled: bool,
    pub mode: FilterMode,
    pub chats: Vec<String>,
    pub excluded_source_types: Vec<String>,
    pub form_memory: bool,
    pub ingest_wait_ms: u64,
    pub search_mode: QueryMode,
}
impl Default for AmemorixPolicy {
    fn default() -> Self {
        Self {
            imports: Default::default(),
            enabled: true,
            mode: FilterMode::Blacklist,
            chats: vec![],
            excluded_source_types: vec![],
            form_memory: false,
            ingest_wait_ms: 45_000,
            search_mode: QueryMode::Hybrid,
        }
    }
}
impl AmemorixPolicy {
    pub fn permits(&self, respect: bool, chat: &str, group: &str, user: &str) -> bool {
        if !respect || !self.enabled || [chat, group, user].iter().all(|value| value.is_empty()) {
            return true;
        }
        let matched = self
            .chats
            .iter()
            .any(|pattern| match pattern.split_once(':') {
                Some(("stream", value)) => value == chat,
                Some(("group", value)) => value == group,
                Some(("user" | "private", value)) => value == user,
                Some(_) => false,
                None => [chat, group, user].contains(&pattern.as_str()),
            });
        match self.mode {
            FilterMode::Blacklist => !matched,
            FilterMode::Whitelist => matched,
        }
    }
    pub fn validate(&self) -> Result<()> {
        self.imports.validate()?;
        if !(1..=86_400_000).contains(&self.ingest_wait_ms) {
            return Err(Error::invalid(
                "A-Memorix ingest_wait_ms must be 1..86400000",
            ));
        }
        for value in self.chats.iter().chain(&self.excluded_source_types) {
            if value.trim().is_empty() || value.chars().any(char::is_control) {
                return Err(Error::invalid(
                    "A-Memorix filter entries must be nonempty tokens",
                ));
            }
        }
        Ok(())
    }
}
pub fn register_configuration(registry: &mut ConfigRegistryBuilder) -> Result<()> {
    registry.register(AMEMORIX,ConfigDefinition{owner:"amemorix".into(),description:"Chat retrieval preferences and source admission for the A-Memorix consumer.".into(),default:AmemorixPolicy::default(),scopes:vec![ConfigLevel::Instance,ConfigLevel::Namespace,ConfigLevel::Subject],apply_mode:ConfigApplyMode::Live,write_policy:ConfigWritePolicy::Scoped},AmemorixPolicy::validate)
}
