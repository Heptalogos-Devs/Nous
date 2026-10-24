//! Immutable, scoped instruction resources.
use crate::{ConfigScope, Error, Result, validate_name};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PromptRef {
    pub scope: ConfigScope,
    pub id: String,
    pub revision: i64,
}
impl PromptRef {
    pub fn validate(&self) -> Result<()> {
        validate_name(&self.id, "prompt id")?;
        validate_scope(&self.scope)?;
        if self.revision <= 0 {
            return Err(Error::invalid(
                "prompt reference requires a positive exact revision",
            ));
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PromptRevision {
    pub reference: PromptRef,
    pub locale: Option<String>,
    pub body: String,
    pub author: String,
    pub reason: String,
    pub recorded_at: String,
}
impl PromptRevision {
    pub fn validate(&self) -> Result<()> {
        self.reference.validate()?;
        validate_body(&self.body)?;
        validate_locale(self.locale.as_deref())?;
        validate_name(&self.author, "prompt author")?;
        if self.reason.len() > 4096 {
            return Err(Error::invalid("prompt reason exceeds 4096 UTF-8 bytes"));
        }
        if self.recorded_at.is_empty()
            || self.recorded_at.len() > 64
            || self.recorded_at.chars().any(char::is_control)
        {
            return Err(Error::invalid("prompt recorded_at is invalid"));
        }
        Ok(())
    }
}
pub fn validate_prompt_body(body: &str) -> Result<()> {
    validate_body(body)
}
pub fn validate_prompt_locale(locale: Option<&str>) -> Result<()> {
    validate_locale(locale)
}
fn validate_body(body: &str) -> Result<()> {
    if body.trim().is_empty() || body.len() > 128 * 1024 {
        return Err(Error::invalid(
            "prompt body must contain 1..131072 UTF-8 bytes",
        ));
    }
    Ok(())
}
fn validate_locale(locale: Option<&str>) -> Result<()> {
    if let Some(locale) = locale
        && (locale.is_empty()
            || locale.len() > 64
            || !locale
                .split('-')
                .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_alphanumeric())))
    {
        return Err(Error::invalid(
            "prompt locale must be an ASCII language tag of at most 64 bytes",
        ));
    }
    Ok(())
}
fn validate_scope(scope: &ConfigScope) -> Result<()> {
    match scope {
        ConfigScope::Instance => Ok(()),
        ConfigScope::Namespace { namespace_id } => namespace_id.validate(),
        ConfigScope::Subject {
            namespace_id,
            subject_id,
        } => {
            namespace_id.validate()?;
            subject_id.validate()
        }
    }
}
