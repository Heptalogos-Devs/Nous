use crate::{
    cli::op_id,
    http::{Operation, import_processing::ProcessImportInput},
};
use anyhow::Result;
use clap::Args;
use nous_domain::*;

#[derive(Clone, clap::ValueEnum)]
enum Vectors {
    Disabled,
    MetadataAllowed,
    Required,
}
impl From<Vectors> for ImportVectorPolicy {
    fn from(value: Vectors) -> Self {
        match value {
            Vectors::Disabled => Self::Disabled,
            Vectors::MetadataAllowed => Self::MetadataAllowed,
            Vectors::Required => Self::Required,
        }
    }
}
#[derive(Args)]
pub struct ProcessArgs {
    #[arg(long)]
    namespace: NamespaceId,
    #[arg(long)]
    subject: SubjectId,
    #[arg(long)]
    catalog: ImportUnitCatalogId,
    /// Repeat to select exact retained unit identities; omit to process the retained catalog.
    #[arg(long)]
    unit: Vec<ImportUnitId>,
    #[arg(long)]
    llm_enabled: bool,
    #[arg(long)]
    extract_time: bool,
    #[arg(long, value_enum, default_value = "metadata-allowed")]
    vectors: Vectors,
    #[arg(long)]
    operation_id: Option<String>,
}
impl ProcessArgs {
    pub(super) fn operation(self) -> Result<Operation> {
        Ok(Operation::ProcessImportUnits(Box::new(
            ProcessImportInput {
                operation_id: op_id(self.operation_id),
                request: ProcessImportUnitsRequest {
                    namespace_id: self.namespace,
                    subject_id: self.subject,
                    catalog_id: self.catalog,
                    unit_ids: (!self.unit.is_empty()).then_some(self.unit),
                    llm_enabled: self.llm_enabled,
                    extract_time: self.extract_time,
                    vectors: self.vectors.into(),
                },
            },
        )))
    }
}
#[derive(Args)]
pub struct OutputArgs {
    #[arg(long)]
    namespace: NamespaceId,
    #[arg(long)]
    subject: SubjectId,
    #[arg(long)]
    output: ImportUnitOutputId,
}
impl OutputArgs {
    pub(super) fn operation(self) -> Result<Operation> {
        Ok(Operation::ReadImportOutput(
            crate::http::import_processing::ReadImportOutputInput {
                namespace_id: self.namespace,
                subject_id: self.subject,
                output_id: self.output,
            },
        ))
    }
}
