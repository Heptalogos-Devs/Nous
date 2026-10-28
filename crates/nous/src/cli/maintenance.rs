use crate::{
    cli::op_id,
    http::Operation,
    http::maintenance::{
        Action, ChangeInput, EditInput, ImportInput, ListInput, MaintainInput, MaintenanceOperation,
    },
};
use anyhow::{Result, bail, ensure};
use clap::{Args, Subcommand};
use nous_domain::{ImportFormat, MaterialId, MemoryId, NamespaceId, SubjectId};
use std::path::PathBuf;

#[derive(Clone, clap::ValueEnum)]
pub enum Format {
    Json,
    Jsonl,
    Text,
}
impl From<Format> for ImportFormat {
    fn from(value: Format) -> Self {
        match value {
            Format::Json => Self::Json,
            Format::Jsonl => Self::Jsonl,
            Format::Text => Self::Text,
        }
    }
}

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
    /// Admit file documents with manifest dedupe and submit the selected processing tasks.
    AdmitFiles(crate::cli::imports::AdmitFilesArgs),
    /// Read exact adopted objects and model provenance for one unit attempt.
    Output(crate::cli::import_processing::OutputArgs),
    /// Process an already frozen unit catalog through the native task pipeline.
    Process(crate::cli::import_processing::ProcessArgs),
    /// Preview source-preserving text units without admitting data or calling models.
    Preview(crate::cli::imports::PreviewArgs),
    /// Freeze an immutable source unit catalog.
    Freeze(crate::cli::imports::FreezeArgs),
    /// Inspect retained source units and their frozen configuration.
    Catalog(crate::cli::imports::CatalogArgs),
    /// Read the exact inputs for retrying original UnitIds.
    RetryInputs(crate::cli::imports::RetryInputsArgs),
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
        #[arg(
            long,
            required_unless_present = "material",
            conflicts_with = "material"
        )]
        file: Option<PathBuf>,
        #[arg(long, required_unless_present = "file", conflicts_with = "file")]
        material: Option<MaterialId>,
        #[arg(long, value_enum)]
        format: Format,
        #[arg(long)]
        source: Option<String>,
        #[arg(long)]
        form_memory: bool,
        /// Complete after the actual formation jobs succeed; creates formation jobs.
        #[arg(long)]
        wait_formation: bool,
        #[arg(long)]
        conversation_ref: Option<nous_domain::ConversationRef>,
        #[arg(long)]
        external_object_ref: Option<nous_domain::ExternalObjectRef>,
    },
}
impl ImportCommand {
    pub fn operation(self) -> Result<Operation> {
        if let Self::AdmitFiles(args) = self {
            return args.operation();
        }
        if let Self::Freeze(args) = self {
            return args.operation();
        }
        if let Self::Catalog(args) = self {
            return args.operation();
        }
        if let Self::RetryInputs(args) = self {
            return args.operation();
        }
        if let Self::Process(args) = self {
            return args.operation();
        }
        if let Self::Output(args) = self {
            return args.operation();
        }
        if let Self::Preview(args) = self {
            return args.operation();
        }
        let Self::Text {
            operation_id,
            namespace,
            subject,
            file,
            material,
            format,
            source,
            form_memory,
            wait_formation,
            conversation_ref,
            external_object_ref,
        } = self
        else {
            bail!("bundle inspection runs locally");
        };
        let document = crate::cli::imports::document(file, material, format.into(), source)?;
        Ok(Operation::Maintenance(MaintenanceOperation::Import(
            ImportInput {
                operation_id: op_id(operation_id),
                namespace_id: namespace.to_string(),
                subject_id: subject.to_string(),
                inputs: vec![document],
                form_memory: form_memory || wait_formation,
                completion: if wait_formation {
                    nous_domain::ImportCompletion::Formed
                } else {
                    nous_domain::ImportCompletion::Accepted
                },
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
