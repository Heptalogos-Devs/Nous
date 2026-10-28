use super::*;
use tokio::io::AsyncReadExt;

pub(super) struct ImportDocumentRead {
    pub material: Option<Material>,
    pub content: Result<String>,
}
impl App {
    /// One bounded, authorized reader feeds preview, record import and frozen unit catalogs.
    pub(super) async fn read_import_document(
        &self,
        ctx: &CallContext,
        namespace: &NamespaceId,
        document: &ImportDocument,
        max_bytes: usize,
        cancel: &CancellationToken,
    ) -> Result<ImportDocumentRead> {
        if document.name.as_ref().is_some_and(|name| {
            name.is_empty() || name.len() > 1024 || name.chars().any(char::is_control)
        }) {
            return Err(Error::invalid(
                "import document name must contain 1 to 1024 bytes without controls",
            ));
        }
        if document
            .source
            .as_ref()
            .is_some_and(|source| source.len() > 4096 || source.contains('\0'))
        {
            return Err(Error::invalid(
                "import document source exceeds 4096 bytes or contains NUL",
            ));
        }
        match &document.input {
            ImportDocumentInput::Inline { content } => {
                if content.is_empty() {
                    return Err(Error::invalid(
                        "inline import input must contain UTF-8 bytes",
                    ));
                }
                check_size(content.len(), max_bytes)?;
                Ok(ImportDocumentRead {
                    material: None,
                    content: Ok(content.clone()),
                })
            }
            ImportDocumentInput::Material { material_id } => {
                let mut source = self.sources.open_original(ctx, material_id, cancel).await?;
                if &source.material.namespace_id != namespace {
                    return Err(Error::invalid("import input belongs to another namespace"));
                }
                let size = usize::try_from(source.material.byte_len).map_err(Error::dependency)?;
                check_size(size, max_bytes)?;
                let mut bytes = Vec::with_capacity(size);
                (&mut source.reader)
                    .take(size as u64 + 1)
                    .read_to_end(&mut bytes)
                    .await?;
                if bytes.len() != size {
                    return Err(Error::new(
                        ErrorCode::DependencyUnavailable,
                        "import material length changed",
                    ));
                }
                let content = String::from_utf8(bytes).map_err(|error| {
                    Error::invalid("import material must contain strict UTF-8 text")
                        .with_cause(&error)
                });
                Ok(ImportDocumentRead {
                    material: Some(source.material),
                    content,
                })
            }
        }
    }
}
fn check_size(size: usize, max: usize) -> Result<()> {
    if size > max.min(import_format::MAX_IMPORT_BYTES) {
        return Err(Error::new(
            ErrorCode::ResourceExhausted,
            "combined import input exceeds 4 MiB",
        ));
    }
    Ok(())
}
