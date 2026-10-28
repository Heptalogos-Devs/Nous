use crate::{
    cli::op_id,
    http::Operation,
    http::maintenance::{
        Action, ChangeInput, EditInput, Format, ImportInput, ListInput, MaintainInput,
        MaintenanceOperation,
    },
};
use anyhow::{Context, Result, bail, ensure};
use clap::{Args, Subcommand};
use nous_domain::{MemoryId, NamespaceId, SubjectId};
use std::{io::Read, path::PathBuf};

#[derive(Args)]
pub struct ChangeArgs {
    id: MemoryId,
    #[arg(long, value_enum)]
    action: Action,
    #[arg(long)]
    expected_revision: i64,
    #[arg(long)]
    operation_id: Option<String>,
    #[arg(long)]
    strength: Option<f64>,
    #[arg(long)]
    hours: Option<f64>,
    #[arg(long, default_value = "")]
    reason: String,
    #[arg(long)]
    force: bool,
    /// JSON edit object containing title, content or epistemic_status.
    #[arg(long)]
    edit: Option<String>,
}
impl ChangeArgs {
    pub fn operation(self) -> Result<Operation> {
        ensure!(
            self.strength.is_none_or(f64::is_finite) && self.hours.is_none_or(f64::is_finite),
            "strength and hours must be finite numbers"
        );
        let edit: Option<EditInput> = self
            .edit
            .map(|value| serde_json::from_str(&value))
            .transpose()?;
        Ok(Operation::Maintenance(MaintenanceOperation::Change(
            self.id.to_string(),
            ChangeInput {
                operation_id: op_id(self.operation_id),
                action: self.action,
                expected_revision: self.expected_revision,
                strength: self.strength,
                hours: self.hours,
                reason: self.reason,
                force: self.force,
                edit,
            },
        )))
    }
}
#[derive(Subcommand)]
pub enum ImportCommand {
    /// Inspect local A-Memorix memory bundles.
    Bundle {
        #[command(subcommand)]
        command: crate::cli::bundle::BundleCommand,
    },
    /// Import faithful memories from source records; model-derived formation is optional.
    Text {
        #[arg(long)]
        operation_id: Option<String>,
        #[arg(long)]
        namespace: NamespaceId,
        #[arg(long)]
        subject: SubjectId,
        #[arg(long)]
        file: PathBuf,
        #[arg(long, value_enum)]
        format: Format,
        #[arg(long)]
        source: Option<String>,
        #[arg(long)]
        form_memory: bool,
        #[arg(long)]
        conversation_ref: Option<nous_domain::ConversationRef>,
        #[arg(long)]
        external_object_ref: Option<nous_domain::ExternalObjectRef>,
    },
}
impl ImportCommand {
    pub fn operation(self) -> Result<Operation> {
        let Self::Text {
            operation_id,
            namespace,
            subject,
            file,
            format,
            source,
            form_memory,
            conversation_ref,
            external_object_ref,
        } = self
        else {
            bail!("bundle inspection runs locally");
        };
        let mut bytes = Vec::new();
        std::fs::File::open(&file)
            .context("cannot open import file")?
            .take((nous_app::import_format::MAX_IMPORT_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .context("cannot read import file")?;
        ensure!(
            bytes.len() <= nous_app::import_format::MAX_IMPORT_BYTES,
            "import file exceeds 4 MiB"
        );
        let content = String::from_utf8(bytes).context("import file must contain UTF-8 text")?;
        Ok(Operation::Maintenance(MaintenanceOperation::Import(
            ImportInput {
                operation_id: op_id(operation_id),
                namespace_id: namespace.to_string(),
                subject_id: subject.to_string(),
                format,
                content,
                form_memory,
                source,
                origin: nous_domain::ObservationOrigin::from_refs(
                    conversation_ref,
                    external_object_ref,
                )?,
            },
        )))
    }
}
pub fn recycle(subject: SubjectId, limit: usize) -> Operation {
    Operation::Maintenance(MaintenanceOperation::Recycle(
        subject.to_string(),
        ListInput { limit },
    ))
}
pub fn audit(id: MemoryId, limit: usize) -> Operation {
    Operation::Maintenance(MaintenanceOperation::Audit(
        id.to_string(),
        ListInput { limit },
    ))
}
pub fn maintain(
    subject: SubjectId,
    limit: usize,
    operation_id: Option<String>,
    after: Option<MemoryId>,
    snapshot_seq: Option<i64>,
) -> Operation {
    Operation::Maintenance(MaintenanceOperation::Maintain(
        subject.to_string(),
        MaintainInput {
            operation_id: op_id(operation_id),
            limit,
            after: after.map(|id| id.to_string()),
            snapshot_seq,
        },
    ))
}
