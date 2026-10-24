//! Role definitions are registered by consumers; resource bindings are Configuration values.
use super::{ModelCapability, ModelCatalog, ModelModality, RoleContract};
use crate::configuration::ConfigDefinition;
use crate::configuration::ConfigKey;
use crate::configuration::ConfigRegistryBuilder;
use crate::prompts::PromptBinding;
use crate::prompts::PromptDefinition;
use nous_domain::{ConfigApplyMode, ConfigLevel, Error, Result};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, sync::Arc};

pub const CATALOG: ConfigKey<ModelCatalog> = ConfigKey::new("models.catalog");
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct ModelsDeployment {
    pub catalog: ModelCatalog,
    pub roles: BTreeMap<String, RoleBinding>,
}
impl ModelsDeployment {
    pub fn validate(&self) -> Result<()> {
        self.catalog.validate()?;
        for binding in self.roles.values() {
            binding.validate()?;
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RoleBinding {
    pub routes: Vec<String>,
    #[serde(default)]
    pub prompt: PromptBinding,
    #[serde(default = "daily")]
    pub max_calls_per_day: u32,
    #[serde(default)]
    pub retry: RetryPolicy,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct RetryPolicy {
    pub max_attempts: u32,
    pub retry_uncertain: bool,
}
impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 5,
            retry_uncertain: false,
        }
    }
}
fn daily() -> u32 {
    20
}
impl RoleBinding {
    pub fn validate(&self) -> Result<()> {
        if self.routes.is_empty()
            || self.routes.len() > 4
            || self.max_calls_per_day > 100000
            || !(1..=16).contains(&self.retry.max_attempts)
        {
            return Err(Error::invalid(
                "role requires 1..4 routes and at most 100000 daily calls",
            ));
        }
        for (i, route) in self.routes.iter().enumerate() {
            super::catalog::identifier(route)?;
            if self.routes[..i].contains(route) {
                return Err(Error::invalid("role repeats an execution route"));
            }
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ModelRequirements {
    pub input_modalities: Vec<ModelModality>,
    pub capabilities: Vec<ModelCapability>,
}
impl ModelRequirements {
    pub fn structured_text() -> Self {
        Self {
            input_modalities: vec![ModelModality::Text],
            capabilities: vec![
                ModelCapability::TextGeneration,
                ModelCapability::StructuredOutput,
            ],
        }
    }
    pub fn text_embedding() -> Self {
        Self {
            input_modalities: vec![ModelModality::Text],
            capabilities: vec![ModelCapability::Embedding],
        }
    }
}
#[derive(Clone)]
pub struct RoleDefinition {
    pub id: &'static str,
    pub binding_key: ConfigKey<Option<RoleBinding>>,
    pub requirements: ModelRequirements,
    pub prompt: Option<PromptDefinition>,
    pub contract: RoleContract,
}
#[derive(Default)]
pub struct RoleRegistryBuilder {
    entries: BTreeMap<String, RoleDefinition>,
}
impl RoleRegistryBuilder {
    pub fn register(&mut self, role: RoleDefinition) -> Result<()> {
        super::catalog::identifier(role.id)?;
        if role.binding_key.path() != format!("models.roles.{}", role.id)
            || self.entries.contains_key(role.id)
        {
            return Err(Error::invalid(
                "duplicate role or binding key differs from registered identity",
            ));
        }
        self.entries.insert(role.id.into(), role);
        Ok(())
    }
    pub fn finish(self) -> RoleRegistry {
        RoleRegistry(Arc::new(self.entries))
    }
}
#[derive(Clone)]
pub struct RoleRegistry(Arc<BTreeMap<String, RoleDefinition>>);
impl RoleRegistry {
    pub fn get(&self, role: &str) -> Result<&RoleDefinition> {
        self.0
            .get(role)
            .ok_or_else(|| Error::invalid("model role is not registered"))
    }
    pub fn definitions(&self) -> impl Iterator<Item = &RoleDefinition> {
        self.0.values()
    }
    pub fn prompt_bindings(
        &self,
        deployment: &ModelsDeployment,
        entries: &[nous_domain::ConfigEntry],
    ) -> Result<Vec<(String, PromptBinding)>> {
        let mut bindings = Vec::new();
        for role in self.definitions() {
            let Some(prompt) = &role.prompt else {
                continue;
            };
            if let Some(binding) = deployment.roles.get(role.id) {
                bindings.push((prompt.id.into(), binding.prompt.clone()));
            }
            for entry in entries
                .iter()
                .filter(|entry| entry.key == role.binding_key.path())
            {
                for value in [&entry.active, &entry.desired].into_iter().flatten() {
                    if let Some(binding) =
                        serde_json::from_value::<Option<RoleBinding>>(value.clone())?
                    {
                        bindings.push((prompt.id.into(), binding.prompt));
                    }
                }
            }
        }
        Ok(bindings)
    }
}
pub fn register_configuration(
    builder: &mut ConfigRegistryBuilder,
    roles: &RoleRegistry,
) -> Result<()> {
    builder.register(
        CATALOG,
        ConfigDefinition {
            owner: "models".into(),
            description: "Provider, model and route catalog".into(),
            default: ModelCatalog::default(),
            scopes: vec![ConfigLevel::Instance],
            apply_mode: ConfigApplyMode::Live,
        },
        ModelCatalog::validate,
    )?;
    for role in roles.definitions() {
        builder.register(
            role.binding_key,
            ConfigDefinition {
                owner: "models".into(),
                description: format!("Execution binding for {}", role.id),
                default: None,
                scopes: vec![
                    ConfigLevel::Instance,
                    ConfigLevel::Namespace,
                    ConfigLevel::Subject,
                ],
                apply_mode: ConfigApplyMode::Live,
            },
            |binding| {
                if let Some(binding) = binding {
                    binding.validate()?;
                }
                Ok(())
            },
        )?;
    }
    let registered = roles.clone();
    builder.register_snapshot_validator("models", move |snapshot| {
        super::validate_snapshot(snapshot, &registered)
    })?;
    Ok(())
}
