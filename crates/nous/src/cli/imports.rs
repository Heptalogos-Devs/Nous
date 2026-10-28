use crate::http::Operation;
use anyhow::{Context, Result, ensure};
use clap::Args;
use nous_domain::*;
use std::{io::Read, path::PathBuf};

#[derive(Clone, clap::ValueEnum)]
enum Strategy {
    Auto,
    Narrative,
    Factual,
    Quote,
    ChatLog,
}
impl From<Strategy> for ImportTextStrategy {
    fn from(value: Strategy) -> Self {
        match value {
            Strategy::Auto => Self::Auto,
            Strategy::Narrative => Self::Narrative,
            Strategy::Factual => Self::Factual,
            Strategy::Quote => Self::Quote,
            Strategy::ChatLog => Self::ChatLog,
        }
    }
}
#[derive(Args)]
pub struct PreviewArgs {
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
    strategy: Option<Strategy>,
    #[arg(long)]
    narrative_window_size: Option<usize>,
    #[arg(long)]
    narrative_overlap: Option<usize>,
    #[arg(long)]
    factual_target_size: Option<usize>,
    /// An explicitly supplied TimePoint JSON object; preview does not infer wall-clock time.
    #[arg(long)]
    reference_time: Option<String>,
}
impl PreviewArgs {
    pub(super) fn operation(self) -> Result<Operation> {
        Ok(Operation::PreviewTextImport(Box::new(self.into_request()?)))
    }
    fn into_request(self) -> Result<PreviewTextImportRequest> {
        Ok(PreviewTextImportRequest {
            namespace_id: self.namespace,
            subject_id: self.subject,
            document: document(self.file, self.material, ImportFormat::Text, None)?,
            overrides: Some(ImportTextSplitOverrides {
                strategy: self.strategy.map(Into::into),
                narrative_window_size: self.narrative_window_size,
                narrative_overlap: self.narrative_overlap,
                factual_target_size: self.factual_target_size,
                reference_time: self
                    .reference_time
                    .map(|value| serde_json::from_str(&value))
                    .transpose()?,
            }),
        })
    }
}

#[derive(Args)]
pub struct FreezeArgs {
    #[command(flatten)]
    input: PreviewArgs,
    #[arg(long)]
    operation_id: Option<String>,
    #[arg(long)]
    parent_catalog: Option<ImportUnitCatalogId>,
}
impl FreezeArgs {
    pub(super) fn operation(self) -> Result<Operation> {
        let preview = self.input.into_request()?;
        Ok(Operation::ImportCatalog(Box::new(
            crate::http::import_catalogs::ImportCatalogOperation::Freeze {
                operation_id: crate::cli::op_id(self.operation_id),
                request: FreezeImportCatalogRequest {
                    namespace_id: preview.namespace_id,
                    subject_id: preview.subject_id,
                    inputs: vec![preview.document],
                    overrides: preview.overrides,
                    parent_catalog_id: self.parent_catalog,
                },
            },
        )))
    }
}
#[derive(Args)]
pub struct CatalogArgs {
    #[arg(long)]
    namespace: NamespaceId,
    #[arg(long)]
    subject: SubjectId,
    #[arg(long)]
    catalog: ImportUnitCatalogId,
}
impl CatalogArgs {
    pub(super) fn operation(self) -> Result<Operation> {
        Ok(Operation::ImportCatalog(Box::new(
            crate::http::import_catalogs::ImportCatalogOperation::Inspect {
                request: ReadImportCatalogRequest {
                    namespace_id: self.namespace,
                    subject_id: self.subject,
                    catalog_id: self.catalog,
                },
            },
        )))
    }
}
#[derive(Args)]
pub struct RetryInputsArgs {
    #[command(flatten)]
    catalog: CatalogArgs,
    #[arg(long = "unit", required = true)]
    unit_ids: Vec<ImportUnitId>,
}
impl RetryInputsArgs {
    pub(super) fn operation(self) -> Result<Operation> {
        Ok(Operation::ImportCatalog(Box::new(
            crate::http::import_catalogs::ImportCatalogOperation::RetryInputs {
                request: RetryImportInputsRequest {
                    namespace_id: self.catalog.namespace,
                    subject_id: self.catalog.subject,
                    catalog_id: self.catalog.catalog,
                    unit_ids: self.unit_ids,
                },
            },
        )))
    }
}

pub(super) fn document(
    file: Option<PathBuf>,
    material: Option<MaterialId>,
    format: ImportFormat,
    source: Option<String>,
) -> Result<ImportDocument> {
    let (name, input) = if let Some(file) = file {
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
        (
            file.file_name()
                .map(|name| name.to_string_lossy().into_owned()),
            ImportDocumentInput::Inline { content },
        )
    } else {
        (
            None,
            ImportDocumentInput::Material {
                material_id: material.context("select an import file or admitted material")?,
            },
        )
    };
    Ok(ImportDocument {
        name,
        format,
        input,
        source,
    })
}
