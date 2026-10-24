//! Independent daily accounting identities for public resources and scoped roles.
use crate::{ConfigScope, Error, Result, validate_name};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ModelBudgetBucket {
    Provider { id: String },
    Model { id: String },
    Role { role: String, scope: ConfigScope },
}

impl ModelBudgetBucket {
    pub fn key(&self) -> Result<String> {
        match self {
            Self::Provider { id } | Self::Model { id } => validate_name(id, "budget resource")?,
            Self::Role { role, scope } => {
                validate_name(role, "budget role")?;
                match scope {
                    ConfigScope::Instance => {}
                    ConfigScope::Namespace { namespace_id } => namespace_id.validate()?,
                    ConfigScope::Subject {
                        namespace_id,
                        subject_id,
                    } => {
                        namespace_id.validate()?;
                        subject_id.validate()?;
                    }
                }
            }
        }
        Ok(serde_json::to_string(self)?)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ModelBudgetLimit {
    pub bucket: ModelBudgetBucket,
    pub max_calls_per_day: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelBudgetUsage {
    pub limit: ModelBudgetLimit,
    pub used: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelBudgetContext {
    pub provider_id: String,
    pub model_id: String,
    pub role: String,
    pub scope: ConfigScope,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelBudgetPlan {
    pub context: ModelBudgetContext,
    pub limits: Vec<ModelBudgetLimit>,
}

impl ModelBudgetContext {
    pub fn permits(&self, bucket: &ModelBudgetBucket) -> bool {
        match bucket {
            ModelBudgetBucket::Provider { id } => id == &self.provider_id,
            ModelBudgetBucket::Model { id } => id == &self.model_id,
            ModelBudgetBucket::Role { role, scope } => {
                role == &self.role
                    && match (scope, &self.scope) {
                        (ConfigScope::Instance, _) => true,
                        (
                            ConfigScope::Namespace { namespace_id: a },
                            ConfigScope::Namespace { namespace_id: b }
                            | ConfigScope::Subject {
                                namespace_id: b, ..
                            },
                        ) => a == b,
                        (
                            ConfigScope::Subject {
                                namespace_id: a,
                                subject_id: x,
                            },
                            ConfigScope::Subject {
                                namespace_id: b,
                                subject_id: y,
                            },
                        ) => a == b && x == y,
                        _ => false,
                    }
            }
        }
    }
}

pub fn validate_daily_call_limit(limit: u32) -> Result<()> {
    if limit > 100_000 {
        return Err(Error::invalid("daily model call limit must be 0..100000"));
    }
    Ok(())
}
