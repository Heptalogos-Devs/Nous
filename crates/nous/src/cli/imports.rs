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
        Ok(Operation::PreviewTextImport(Box::new(
            PreviewTextImportRequest {
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
