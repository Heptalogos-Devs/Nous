//! Model management uses the same authorized operations as HTTP and MCP.
use super::cognition::ModelCommand;
use crate::http::Operation;
use anyhow::{Context, Result, bail, ensure};
use nous_app::{
    common::models::InvocationEdits,
    model_management::{ModelContractRequest, ModelPreviewRequest},
};
use std::path::Path;

pub(super) fn operation(command: ModelCommand) -> Result<Operation> {
    match command {
        ModelCommand::Contract { role, file, scope } => {
            Ok(Operation::ModelContract(ModelContractRequest {
                role,
                scope: scope.scope(),
                input: file.map(|path| read_input(&path)).transpose()?,
            }))
        }
        ModelCommand::Preview {
            role,
            file,
            prompt_file,
            scope,
        } => {
            let input = read_input(&file)?;
            let prompt = prompt_file.map(|path| read_text(&path)).transpose()?;
            Ok(Operation::ModelPreview(ModelPreviewRequest {
                role,
                scope: scope.scope(),
                input,
                edits: InvocationEdits { prompt },
            }))
        }
        _ => bail!("model command does not build a management operation"),
    }
}

fn read_input(path: &Path) -> Result<serde_json::Value> {
    serde_json::from_str(&read_text(path)?)
        .with_context(|| format!("model input {} must be JSON", path.display()))
}

fn read_text(path: &Path) -> Result<String> {
    let metadata =
        std::fs::metadata(path).with_context(|| format!("cannot read {}", path.display()))?;
    ensure!(
        metadata.len() <= 64 * 1024 * 1024,
        "model input file exceeds 64 MiB"
    );
    std::fs::read_to_string(path)
        .with_context(|| format!("{} must contain UTF-8 text", path.display()))
}
