//! Prompt resource loading and strict bounded rendering share one owner.
//! Minijinja Template/Environment APIs own template parsing and execution.
//! Instance-root resource loading adapted from Nous-previous model/prompts.ts
//! (Apache-2.0; source commit and license in models/structured-sources.json).
//! Source SHA-256: 466a564983b23377e5b4f99fca1f8375ed5ec9615bd8706297ffc953ed94aefb.
//! MiniJinja API source: commit 6ba9aded9da6d3d4b8de7d13b64c4d9d70b671c4,
//! environment.rs SHA-256 ed7c7f62a2ed35cb3a404f3d19063d40bf9c019b07fce6ff3ca48051a2e2d67e;
//! template.rs SHA-256 a467968cf73d9d67d61f3af44953a225cd930a721631f19ca3f50bb0259a73af.
use crate::common::instance::InstancePaths;
use minijinja::{
    Environment, UndefinedBehavior,
    value::{Serde, Value as TemplateValue},
};
use nous_domain::{Error, ErrorCode, Result};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::File,
    io::{Read, Write},
    path::PathBuf,
};

const MAX_TEMPLATE_BYTES: usize = 128 * 1024;
const MAX_RENDERED_BYTES: usize = 256 * 1024;

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct PromptBinding {
    /// Editable UTF-8 asset beneath the instance prompts directory.
    pub file: Option<PathBuf>,
    pub locale: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromptArgument {
    pub value: Value,
    pub source: Option<String>,
}
pub type PromptArguments = BTreeMap<String, PromptArgument>;
/// A bound managed asset and its actual bytes' immutable identity.
#[derive(Debug, Clone, Serialize)]
pub struct ManagedPromptReference {
    pub relative_path: PathBuf,
    pub path: PathBuf,
    pub sha256: String,
    pub byte_len: u64,
}
#[derive(Debug, Clone, Serialize)]
pub struct PromptTemplate {
    pub id: String,
    pub revision: Option<String>,
    pub locale: Option<String>,
    pub file: Option<PathBuf>,
    pub text: String,
    pub sha256: String,
}
#[derive(Debug, Clone, Serialize)]
pub struct PromptSnapshot {
    pub id: String,
    /// Managed product revision; direct file bindings are identified by content hash.
    pub revision: Option<String>,
    pub locale: Option<String>,
    pub file: Option<PathBuf>,
    pub template: String,
    pub template_sha256: String,
    pub arguments: PromptArguments,
    pub text: String,
    pub sha256: String,
}
#[derive(Clone)]
pub struct Prompts {
    paths: InstancePaths,
}
impl Prompts {
    pub fn new(paths: InstancePaths) -> Self {
        Self { paths }
    }
    /// Render and preview use identical assets, arguments, strictness and limits.
    pub fn render(
        &self,
        id: &str,
        binding: &PromptBinding,
        arguments: &PromptArguments,
    ) -> Result<PromptSnapshot> {
        let asset = self.read(id, binding)?;
        let source = asset.text;
        if serde_json::to_vec(arguments)?.len() > 1024 * 1024 {
            return Err(Error::new(
                ErrorCode::ResourceExhausted,
                "prompt arguments exceed 1 MiB",
            ));
        }
        let mut env = Environment::new();
        env.set_undefined_behavior(UndefinedBehavior::Strict);
        env.set_fuel(Some(50000));
        env.set_recursion_limit(32);
        let template = env
            .template_from_named_str(id, &source)
            .map_err(|error| Error::invalid(format!("prompt template is invalid: {error}")))?;
        let context: BTreeMap<_, _> = arguments
            .iter()
            .map(|(name, arg)| (name, &arg.value))
            .collect();
        let mut output = BoundedOutput(Vec::new());
        template
            .render_captured_to(TemplateValue::from(Serde(context)), &mut output)
            .map_err(|error| Error::invalid(format!("prompt rendering failed: {error}")))?;
        let text = String::from_utf8(output.0)
            .map_err(|_| Error::internal("prompt renderer returned invalid UTF-8"))?;
        if text.trim().is_empty() {
            return Err(Error::invalid("rendered prompt must be nonempty"));
        }
        Ok(PromptSnapshot {
            id: id.into(),
            revision: None,
            locale: binding.locale.clone(),
            file: binding.file.clone(),
            template_sha256: digest(&source),
            template: source,
            arguments: arguments.clone(),
            sha256: digest(&text),
            text,
        })
    }
    pub fn preview(
        &self,
        id: &str,
        binding: &PromptBinding,
        arguments: &PromptArguments,
    ) -> Result<PromptSnapshot> {
        self.render(id, binding, arguments)
    }
    pub fn read(&self, id: &str, binding: &PromptBinding) -> Result<PromptTemplate> {
        let text = self.load_source(id, binding)?;
        Ok(PromptTemplate {
            id: id.into(),
            revision: None,
            locale: binding.locale.clone(),
            file: binding.file.clone(),
            sha256: digest(&text),
            text,
        })
    }
    pub fn managed_reference(
        &self,
        id: &str,
        binding: &PromptBinding,
    ) -> Result<Option<ManagedPromptReference>> {
        let Some(file) = &binding.file else {
            return Ok(None);
        };
        let asset = self.read(id, binding)?;
        let path = self.managed_path(file).map_err(|_| {
            Error::new(
                ErrorCode::CapabilityUnavailable,
                "prompt resource is outside its managed directory",
            )
        })?;
        let relative_path = file
            .components()
            .filter_map(|part| match part {
                std::path::Component::Normal(value) => Some(value),
                _ => None,
            })
            .collect();
        Ok(Some(ManagedPromptReference {
            relative_path,
            path,
            sha256: asset.sha256,
            byte_len: asset.text.len() as u64,
        }))
    }
    fn managed_path(&self, file: &std::path::Path) -> anyhow::Result<PathBuf> {
        let path = self.paths.resolve(file)?;
        if !file.starts_with("prompts")
            || !path
                .canonicalize()?
                .starts_with(self.paths.prompts.canonicalize()?)
        {
            anyhow::bail!("prompt file must be inside the instance prompts directory");
        }
        Ok(path)
    }
    fn load_source(&self, id: &str, binding: &PromptBinding) -> Result<String> {
        let text = if let Some(file) = &binding.file {
            let read = || -> anyhow::Result<String> {
                let path = self.managed_path(file)?;
                let handle = File::open(path)?;
                if !handle.metadata()?.is_file() {
                    anyhow::bail!("prompt must be a regular file");
                }
                let mut text = String::new();
                handle
                    .take((MAX_TEMPLATE_BYTES + 1) as u64)
                    .read_to_string(&mut text)?;
                Ok(text)
            };
            read().map_err(|_| {
                Error::new(
                    ErrorCode::CapabilityUnavailable,
                    "bound prompt file is missing, unreadable or outside the prompt directory",
                )
            })?
        } else {
            match id {
                "memory_formation" => include_str!("../prompts/memory-formation.md"),
                "structured_formation" => include_str!("../prompts/structured-formation.md"),
                _ => {
                    return Err(Error::new(
                        ErrorCode::CapabilityUnavailable,
                        "prompt resource is not bound to a file or bundled asset",
                    ));
                }
            }
            .to_owned()
        };
        if text.trim().is_empty() || text.len() > MAX_TEMPLATE_BYTES {
            return Err(Error::new(
                ErrorCode::CapabilityUnavailable,
                "prompt template must contain 1..131072 UTF-8 bytes",
            ));
        }
        Ok(text)
    }
}
fn digest(text: &str) -> String {
    format!("{:x}", Sha256::digest(text.as_bytes()))
}
struct BoundedOutput(Vec<u8>);
impl Write for BoundedOutput {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if self.0.len().saturating_add(bytes.len()) > MAX_RENDERED_BYTES {
            return Err(std::io::Error::other("rendered prompt exceeds 256 KiB"));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
