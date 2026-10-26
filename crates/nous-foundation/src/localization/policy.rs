use crate::configuration::{ConfigDefinition, ConfigKey, ConfigRegistryBuilder, ConfigWritePolicy};
use nous_domain::{ConfigApplyMode, ConfigLevel, Error, Result};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const LOCALIZATION: ConfigKey<LocalizationPolicy> = ConfigKey::new("localization");

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct LocalizationPolicy {
    pub default_locale: String,
}
impl Default for LocalizationPolicy {
    fn default() -> Self {
        Self {
            default_locale: "zh-CN".into(),
        }
    }
}
impl LocalizationPolicy {
    pub fn validate(&self) -> Result<()> {
        let locale: unic_langid::LanguageIdentifier =
            self.default_locale.parse().map_err(|error| {
                Error::invalid("invalid default language identifier").with_cause(&error)
            })?;
        if !super::AVAILABLE.contains(&locale.to_string().as_str()) {
            return Err(Error::invalid("default language has no packaged resources")
                .with_context("locale", self.default_locale.clone()));
        }
        Ok(())
    }
}
pub fn register_configuration(registry: &mut ConfigRegistryBuilder) -> Result<()> {
    registry.register(
        LOCALIZATION,
        ConfigDefinition {
            owner: "localization".into(),
            description: "Default language for user-facing presentation.".into(),
            default: LocalizationPolicy::default(),
            scopes: vec![
                ConfigLevel::Instance,
                ConfigLevel::Namespace,
                ConfigLevel::Subject,
            ],
            apply_mode: ConfigApplyMode::Live,
            write_policy: ConfigWritePolicy::Scoped,
        },
        LocalizationPolicy::validate,
    )
}
