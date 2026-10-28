use crate::{ConversationRef, Error, Result, validate_name};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PackageDescriptor {
    pub id: String,
    pub name: String,
    pub version: Option<String>,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub author: String,
}
impl PackageDescriptor {
    pub fn validate(&self) -> Result<()> {
        validate_name(&self.id, "package ID")?;
        validate_name(&self.name, "package name")?;
        if let Some(version) = &self.version {
            validate_name(version, "package release version")?;
        }
        if self.description.len() > 16 * 1024
            || self.author.len() > 1024
            || self.description.contains('\0')
            || self.author.contains('\0')
        {
            return Err(Error::invalid(
                "package description or author exceeds its text bound",
            ));
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BundleContentLevel {
    Knowledge,
    Full,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum BundlePlacement {
    Global,
    Chat { conversation_ref: ConversationRef },
}
impl BundlePlacement {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Global => "global",
            Self::Chat { .. } => "chat",
        }
    }
    pub fn conversation(&self) -> Option<&ConversationRef> {
        match self {
            Self::Global => None,
            Self::Chat { conversation_ref } => Some(conversation_ref),
        }
    }
}
