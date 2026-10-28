mod chat;
mod narrative;
pub mod policy;
mod split;
use crate::{App, import_format};
use nous_domain::*;
use nous_foundation::execution;
use sha2::{Digest, Sha256};
use tokio::io::AsyncReadExt;
use tokio_util::sync::CancellationToken;

impl App {
    pub async fn preview_text_import(
        &self,
        ctx: &CallContext,
        request: PreviewTextImportRequest,
        cancel: &CancellationToken,
    ) -> Result<TextImportPreview> {
        ctx.require_subject(&request.namespace_id, &request.subject_id)?;
        self.database
            .subject(&request.namespace_id, &request.subject_id)
            .await?;
        if request.document.format != ImportFormat::Text {
            return Err(Error::invalid(
                "text split preview requires document format=text",
            ));
        }
        let options = policy::effective(
            self.configuration
                .snapshot(
                    &ConfigScope::Subject {
                        namespace_id: request.namespace_id.clone(),
                        subject_id: request.subject_id.clone(),
                    },
                    false,
                )
                .await?
                .get(policy::IMPORT_TEXT)?,
            request.overrides,
        )?;
        let content = match request.document.input {
            ImportDocumentInput::Inline { content } => {
                check_length(content.len())?;
                content
            }
            ImportDocumentInput::Material { material_id } => {
                let mut original = self
                    .sources
                    .open_original(ctx, &material_id, cancel)
                    .await?;
                if original.material.namespace_id != request.namespace_id {
                    return Err(Error::invalid(
                        "preview material belongs to another namespace",
                    ));
                }
                let size =
                    usize::try_from(original.material.byte_len).map_err(Error::dependency)?;
                check_length(size)?;
                let mut bytes = Vec::with_capacity(size);
                (&mut original.reader)
                    .take(size as u64 + 1)
                    .read_to_end(&mut bytes)
                    .await?;
                if bytes.len() != size {
                    return Err(Error::new(
                        ErrorCode::DependencyUnavailable,
                        "preview material length changed",
                    ));
                }
                String::from_utf8(bytes).map_err(|error| {
                    Error::invalid("preview requires strict UTF-8 text").with_cause(&error)
                })?
            }
        };
        if content.trim().is_empty() {
            return Err(Error::invalid("preview text is empty"));
        }
        execution::blocking(cancel, move |local| {
            execution::check_cancelled(&local)?;
            let selected = split::select(&content, options.strategy);
            let mut drafts = split::Drafts::new(&content);
            match selected {
                ImportTextStrategy::Narrative => narrative::split(&mut drafts, &options)?,
                ImportTextStrategy::Factual => {
                    split::factual(&mut drafts, options.factual_target_size)?
                }
                ImportTextStrategy::Quote => split::quote(&mut drafts)?,
                ImportTextStrategy::ChatLog => chat::split(&mut drafts, &options)?,
                ImportTextStrategy::Auto => unreachable!("strategy selection resolves auto"),
            }
            execution::check_cancelled(&local)?;
            Ok(TextImportPreview {
                content_hash: format!("{:x}", Sha256::digest(content.as_bytes())),
                byte_len: content.len() as u64,
                effective_options: options,
                selected_strategy: selected,
                units: drafts.units,
                warnings: drafts.warnings,
            })
        })
        .await
    }
}
fn check_length(length: usize) -> Result<()> {
    if length > import_format::MAX_IMPORT_BYTES {
        return Err(Error::new(
            ErrorCode::ResourceExhausted,
            "preview text exceeds 4 MiB",
        ));
    }
    Ok(())
}
