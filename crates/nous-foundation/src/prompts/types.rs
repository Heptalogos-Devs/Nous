use nous_domain::{ConfigScope, Error, PromptRef, Result, validate_prompt_locale};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeMap, path::PathBuf};

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum PromptBinding {
    #[default]
    Default,
    File {
        file: PathBuf,
        locale: Option<String>,
    },
    Managed {
        reference: PromptRef,
    },
}
impl PromptBinding {
    pub fn validate(&self) -> Result<()> {
        match self {
            Self::Default => Ok(()),
            Self::File { file, locale } => {
                if !file.starts_with("prompts")
                    || !file
                        .components()
                        .all(|c| matches!(c, std::path::Component::Normal(_)))
                {
                    return Err(Error::invalid(
                        "prompt file must be a relative path inside prompts/",
                    ));
                }
                validate_prompt_locale(locale.as_deref())
            }
            Self::Managed { reference } => reference.validate(),
        }
    }
    pub fn validate_for_scope(&self, scope: &ConfigScope) -> Result<()> {
        self.validate()?;
        if let Self::Managed { reference } = self {
            validate_inheritance(scope, &reference.scope)?;
        }
        Ok(())
    }
}
pub fn validate_inheritance(current: &ConfigScope, source: &ConfigScope) -> Result<()> {
    let allowed = match (source, current) {
        (ConfigScope::Instance, _) => true,
        (
            ConfigScope::Namespace { namespace_id: a },
            ConfigScope::Namespace { namespace_id: b },
        ) => a == b,
        (
            ConfigScope::Namespace { namespace_id: a },
            ConfigScope::Subject {
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
    };
    if !allowed {
        return Err(Error::new(
            nous_domain::ErrorCode::Forbidden,
            "prompt selection must refer to the current scope or an ancestor",
        ));
    }
    Ok(())
}
pub fn hierarchy(scope: &ConfigScope) -> Vec<ConfigScope> {
    let mut scopes = vec![ConfigScope::Instance];
    if let ConfigScope::Subject { namespace_id, .. } = scope {
        scopes.push(ConfigScope::Namespace {
            namespace_id: namespace_id.clone(),
        });
    }
    if !matches!(scope, ConfigScope::Instance) {
        scopes.push(scope.clone());
    }
    scopes
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PromptArgument {
    pub value: Value,
    pub source: Option<String>,
}
pub type PromptArguments = BTreeMap<String, PromptArgument>;
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PromptEdit {
    pub id: String,
    pub expected_revision: i64,
    pub body: String,
    pub locale: Option<String>,
    #[serde(default)]
    pub reason: String,
}
#[derive(Clone)]
pub struct PromptDefinition {
    pub id: &'static str,
    pub template: &'static str,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptTemplate {
    pub id: String,
    pub binding: PromptBinding,
    pub locale: Option<String>,
    pub text: String,
    pub sha256: String,
    pub variables: Vec<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptSnapshot {
    pub resource: PromptTemplate,
    pub template: String,
    pub template_sha256: String,
    pub arguments: PromptArguments,
    pub text: String,
    pub sha256: String,
    pub overridden: bool,
}
#[derive(Debug, Clone, Serialize)]
pub struct ManagedPromptReference {
    pub relative_path: PathBuf,
    pub path: PathBuf,
    pub sha256: String,
    pub byte_len: u64,
}
#[derive(Debug, Clone, Serialize)]
pub struct PromptCatalogEntry {
    pub id: String,
    pub binding: PromptBinding,
    pub locale: Option<String>,
    pub byte_len: Option<usize>,
    pub variables: Vec<String>,
    pub error: Option<Error>,
}
#[derive(Debug, Clone, Serialize)]
pub struct PromptCatalog {
    pub resources: Vec<PromptCatalogEntry>,
    pub complete: bool,
}
