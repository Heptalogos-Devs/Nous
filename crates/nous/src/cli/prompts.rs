//! Prompt resource commands use the same typed operations as HTTP and MCP.
use super::{op_id, scope::ScopeArgs};
use crate::http::Operation;
use anyhow::{Context, Result, ensure};
use clap::Subcommand;
use nous_app::administration::prompts::{
    EditedPromptActivation, PromptActivateRequest, PromptEditRequest, PromptRequest,
};
use nous_foundation::prompts::{PromptBinding, PromptEdit};
use std::{
    fs::File,
    io::Read,
    path::{Path, PathBuf},
};

#[derive(Subcommand)]
pub enum PromptCommand {
    /// List registered, file and inherited managed resources.
    Catalog {
        #[command(flatten)]
        scope: ScopeArgs,
        #[arg(long, default_value_t = 50)]
        limit: usize,
    },
    /// Read a default resource or an explicit file/managed selection.
    Read {
        #[arg(long)]
        id: Option<String>,
        #[arg(long, default_value = "{\"kind\":\"default\"}")]
        selection: String,
        #[command(flatten)]
        scope: ScopeArgs,
    },
    /// Read retained immutable revisions in this scope or an ancestor.
    History {
        #[arg(long)]
        id: String,
        #[arg(long)]
        resource_scope: Option<String>,
        #[arg(long, default_value_t = 50)]
        limit: usize,
        #[command(flatten)]
        scope: ScopeArgs,
    },
    /// Save an immutable revision, optionally activating it atomically.
    Edit {
        #[arg(long)]
        id: String,
        #[arg(long)]
        file: PathBuf,
        #[arg(long)]
        expected_revision: i64,
        #[arg(long)]
        locale: Option<String>,
        #[arg(long, default_value = "")]
        reason: String,
        #[arg(long)]
        operation_id: Option<String>,
        #[arg(long, requires = "binding_revision")]
        activate_role: Option<String>,
        #[arg(long, requires = "activate_role")]
        binding_revision: Option<i64>,
        #[command(flatten)]
        scope: ScopeArgs,
    },
    /// Activate an exact selection in a configured role binding.
    Activate {
        #[arg(long)]
        role: String,
        #[arg(long)]
        selection: String,
        #[arg(long)]
        expected_revision: i64,
        #[arg(long)]
        operation_id: Option<String>,
        #[command(flatten)]
        scope: ScopeArgs,
    },
    /// Render with the production renderer without making a model request.
    Preview {
        #[arg(long)]
        id: Option<String>,
        #[arg(long, default_value = "{\"kind\":\"default\"}")]
        selection: String,
        #[arg(long)]
        arguments_file: Option<PathBuf>,
        #[arg(long)]
        prompt_file: Option<PathBuf>,
        #[command(flatten)]
        scope: ScopeArgs,
    },
}
impl PromptCommand {
    pub fn operation(self) -> Result<Operation> {
        let request = match self {
            Self::Catalog { scope, limit } => PromptRequest::Catalog {
                scope: scope.scope(),
                limit,
            },
            Self::Read {
                id,
                selection,
                scope,
            } => PromptRequest::Read {
                scope: scope.scope(),
                id,
                selection: binding(&selection)?,
            },
            Self::History {
                id,
                resource_scope,
                limit,
                scope,
            } => PromptRequest::History {
                scope: scope.scope(),
                id,
                resource_scope: resource_scope
                    .map(|s| serde_json::from_str(&s))
                    .transpose()?,
                limit,
            },
            Self::Edit {
                id,
                file,
                expected_revision,
                locale,
                reason,
                operation_id,
                activate_role,
                binding_revision,
                scope,
            } => {
                let activate = activate_role.map(|role| EditedPromptActivation {
                    role,
                    expected_revision: binding_revision.expect("clap requires binding_revision"),
                });
                PromptRequest::Edit(PromptEditRequest {
                    operation_id: op_id(operation_id),
                    scope: scope.scope(),
                    edit: PromptEdit {
                        id,
                        expected_revision,
                        body: read_text(&file, 128 * 1024)?,
                        locale,
                        reason,
                    },
                    activate,
                })
            }
            Self::Activate {
                role,
                selection,
                expected_revision,
                operation_id,
                scope,
            } => PromptRequest::Activate(PromptActivateRequest {
                operation_id: op_id(operation_id),
                scope: scope.scope(),
                role,
                expected_revision,
                selection: binding(&selection)?,
            }),
            Self::Preview {
                id,
                selection,
                arguments_file,
                prompt_file,
                scope,
            } => PromptRequest::Preview {
                scope: scope.scope(),
                id,
                selection: binding(&selection)?,
                arguments: arguments_file
                    .map(|path| {
                        serde_json::from_str(&read_text(&path, 1024 * 1024)?)
                            .map_err(anyhow::Error::from)
                    })
                    .transpose()?
                    .unwrap_or_default(),
                prompt: prompt_file
                    .map(|path| read_text(&path, 128 * 1024))
                    .transpose()?,
            },
        };
        Ok(Operation::Prompt(request))
    }
}
fn binding(json: &str) -> Result<PromptBinding> {
    serde_json::from_str(json).context("selection must be a PromptBinding JSON object")
}
fn read_text(path: &Path, limit: usize) -> Result<String> {
    let mut text = String::new();
    File::open(path)
        .with_context(|| format!("cannot read {}", path.display()))?
        .take((limit + 1) as u64)
        .read_to_string(&mut text)
        .with_context(|| format!("{} must be UTF-8", path.display()))?;
    ensure!(
        text.len() <= limit,
        "input file exceeds {limit} UTF-8 bytes"
    );
    Ok(text)
}
