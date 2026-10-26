//! Captured origin identities remain opaque and retain the host's exact spelling.
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
macro_rules! origin_ref {
    ($name:ident) => {
        #[derive(
            Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, schemars::JsonSchema,
        )]
        #[serde(transparent)]
        pub struct $name(pub String);
        impl $name {
            pub fn new(value: impl Into<String>) -> Result<Self> {
                let value = Self(value.into());
                value.validate()?;
                Ok(value)
            }
            pub fn as_str(&self) -> &str {
                &self.0
            }
            pub fn validate(&self) -> Result<()> {
                validate_origin_ref(&self.0)
            }
        }
        impl std::str::FromStr for $name {
            type Err = Error;
            fn from_str(value: &str) -> Result<Self> {
                Self::new(value.to_owned())
            }
        }
        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(&self.0)
            }
        }
        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: serde::Deserializer<'de>>(
                deserializer: D,
            ) -> std::result::Result<Self, D::Error> {
                Self::new(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
            }
        }
    };
}
origin_ref!(ConversationRef);
origin_ref!(ExternalObjectRef);
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ObservationOrigin {
    pub conversation_ref: Option<ConversationRef>,
    pub external_object_ref: Option<ExternalObjectRef>,
}
impl ObservationOrigin {
    pub fn validate(&self) -> Result<()> {
        if self.conversation_ref.is_none() && self.external_object_ref.is_none() {
            return Err(Error::invalid(
                "observation origin needs a captured conversation or external object reference",
            ));
        }
        if let Some(reference) = &self.conversation_ref {
            reference.validate()?;
        }
        if let Some(reference) = &self.external_object_ref {
            reference.validate()?;
        }
        Ok(())
    }
    pub fn from_refs(
        conversation_ref: Option<ConversationRef>,
        external_object_ref: Option<ExternalObjectRef>,
    ) -> Result<Option<Self>> {
        if conversation_ref.is_none() && external_object_ref.is_none() {
            return Ok(None);
        }
        let value = Self {
            conversation_ref,
            external_object_ref,
        };
        value.validate()?;
        Ok(Some(value))
    }
}
fn validate_origin_ref(value: &str) -> Result<()> {
    if value.is_empty() || value.len() > 1024 || value.chars().any(char::is_control) {
        return Err(Error::invalid(
            "origin reference must contain 1..1024 UTF-8 bytes without control characters",
        ));
    }
    Ok(())
}
