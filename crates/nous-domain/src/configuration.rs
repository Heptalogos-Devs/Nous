//! Immutable configuration states; effective inheritance belongs to Configuration.
use crate::{AccessScope, CallContext, Error, NamespaceId, Result, SubjectId, validate_name};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum ConfigLevel {
    Instance,
    Namespace,
    Subject,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ConfigScope {
    Instance,
    Namespace {
        namespace_id: NamespaceId,
    },
    Subject {
        namespace_id: NamespaceId,
        subject_id: SubjectId,
    },
}

impl ConfigScope {
    pub fn require_access(&self, ctx: &CallContext) -> Result<()> {
        match self {
            Self::Instance => ctx.require_admin(),
            Self::Namespace { namespace_id } => ctx.require_namespace(namespace_id),
            Self::Subject {
                namespace_id,
                subject_id,
            } => ctx.require_subject(namespace_id, subject_id),
        }
    }
    pub fn for_caller(ctx: &CallContext) -> Self {
        match &ctx.scope {
            AccessScope::Admin => Self::Instance,
            AccessScope::Namespace { namespace_id } => Self::Namespace {
                namespace_id: namespace_id.clone(),
            },
            AccessScope::Subject {
                namespace_id,
                subject_id,
            } => Self::Subject {
                namespace_id: namespace_id.clone(),
                subject_id: subject_id.clone(),
            },
        }
    }
    pub fn level(&self) -> ConfigLevel {
        match self {
            Self::Instance => ConfigLevel::Instance,
            Self::Namespace { .. } => ConfigLevel::Namespace,
            Self::Subject { .. } => ConfigLevel::Subject,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ConfigApplyMode {
    Live,
    Restart,
}

/// `None` removes this scope's override and resumes inherited configuration.
/// Restart changes may retain an older active override until activation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConfigEntry {
    pub scope: ConfigScope,
    pub key: String,
    pub revision: i64,
    pub active: Option<Value>,
    pub desired: Option<Value>,
    pub apply_mode: ConfigApplyMode,
    pub recorded_at: String,
    pub commit_seq: i64,
}

impl ConfigEntry {
    pub fn validate(&self) -> Result<()> {
        validate_name(&self.key, "configuration key")?;
        validate_name(&self.recorded_at, "configuration recorded_at")?;
        if self.key != self.key.trim() {
            return Err(Error::invalid(
                "configuration key must not contain outer whitespace",
            ));
        }
        if self.revision <= 0 || self.revision != self.commit_seq {
            return Err(Error::invalid(
                "configuration revision must equal its positive commit sequence",
            ));
        }
        if self.apply_mode == ConfigApplyMode::Live && self.active != self.desired {
            return Err(Error::invalid(
                "live configuration must have equal active and desired values",
            ));
        }
        Ok(())
    }
}
