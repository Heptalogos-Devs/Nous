use super::*;
use nous_domain::media::PutMediaRequest;
use nous_foundation::{content::ReferenceLease, media::CapturedMedia};
use tokio::io::AsyncReadExt;

pub(super) struct InputCatalog {
    inputs: Vec<PendingInput>,
    pub records: Vec<ParsedImportRecord>,
    _protection: ReferenceLease,
}
struct PendingInput {
    name: Option<String>,
    format: ImportFormat,
    source: Option<String>,
    bytes: InputBytes,
}
enum InputBytes {
    Captured(CapturedMedia),
    Material(Material),
}

impl App {
    pub(super) async fn freeze_import_inputs(
        &self,
        ctx: &CallContext,
        request: &ImportRequest,
        cancel: &CancellationToken,
    ) -> Result<InputCatalog> {
        if request.inputs.is_empty() || request.inputs.len() > import_format::MAX_IMPORT_RECORDS {
            return Err(Error::invalid(
                "import requires an ordered nonempty input catalog with at most 1000 documents",
            ));
        }
        let protection = self.objects.protect_references(cancel).await?;
        let mut total = 0usize;
        let mut normalized = 0usize;
        let mut records = Vec::new();
        let mut inputs = Vec::new();
        for (input_index, document) in request.inputs.iter().enumerate() {
            validate_document(document)?;
            let (bytes, content) = match &document.input {
                ImportDocumentInput::Inline { content } => {
                    if content.is_empty() {
                        return Err(
                            Error::invalid("inline import input must contain UTF-8 bytes")
                                .with_context("input_index", input_index),
                        );
                    }
                    add_bytes(&mut total, content.len())?;
                    let captured = self.media.capture_text(content, cancel).await?;
                    (InputBytes::Captured(captured), Ok(content.clone()))
                }
                ImportDocumentInput::Material { material_id } => {
                    let mut source = self.sources.open_original(ctx, material_id, cancel).await?;
                    if source.material.namespace_id != request.namespace_id {
                        return Err(Error::invalid("import input belongs to another namespace")
                            .with_context("input_index", input_index));
                    }
                    let size =
                        usize::try_from(source.material.byte_len).map_err(Error::dependency)?;
                    add_bytes(&mut total, size)?;
                    let mut bytes = Vec::new();
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
                    (InputBytes::Material(source.material), content)
                }
            };
            let parsed = content.and_then(|content| {
                import_format::parse(&content, document.format, document.source.as_deref())
            });
            let parsed = match parsed {
                Ok(records) => records,
                Err(error) if error.code == ErrorCode::ResourceExhausted => {
                    return Err(error.with_context("input_index", input_index));
                }
                Err(error) => vec![ParsedImportRecord {
                    index: 0,
                    input_index,
                    record_index: None,
                    external_id: None,
                    record: None,
                    error: Some(error.with_context("input_index", input_index)),
                }],
            };
            for mut parsed in parsed {
                parsed.input_index = input_index;
                parsed.index = records.len();
                if let Some(record) = &mut parsed.record
                    && record.origin.is_none()
                {
                    record.origin = request.origin.clone();
                }
                normalized = normalized.saturating_add(serde_json::to_vec(&parsed)?.len());
                if normalized > import_format::MAX_IMPORT_BYTES
                    || records.len() >= import_format::MAX_IMPORT_RECORDS
                {
                    return Err(Error::new(
                        ErrorCode::ResourceExhausted,
                        "combined import catalog exceeds 4 MiB or 1000 records",
                    ));
                }
                records.push(parsed);
            }
            inputs.push(PendingInput {
                name: document.name.clone(),
                format: document.format,
                source: document.source.clone(),
                bytes,
            });
        }
        Ok(InputCatalog {
            inputs,
            records,
            _protection: protection,
        })
    }

    pub(super) async fn register_import_inputs(
        &self,
        tx: &mut WriteTransaction,
        job: &JobRecord,
        catalog: InputCatalog,
    ) -> Result<(Vec<ImportInputRef>, Vec<ParsedImportRecord>)> {
        let (namespace, subject) = job.subject_scope()?;
        tx.retain_until_end(catalog._protection);
        let mut inputs = Vec::new();
        for (input_index, pending) in catalog.inputs.into_iter().enumerate() {
            let material = match pending.bytes {
                InputBytes::Material(material) => material,
                InputBytes::Captured(captured) => {
                    self.register_captured_material(
                        tx,
                        PutMediaRequest {
                            namespace_id: namespace.clone(),
                            owner_subject_id: Some(subject.clone()),
                            media_type: captured.media_type.clone(),
                            declared_byte_len: Some(captured.object.byte_len as u64),
                            expected_content_hash: Some(captured.object.digest.clone()),
                            source: pending.source,
                            event_time: None,
                        },
                        captured,
                    )
                    .await?
                }
            };
            if !tx
                .subject_has_material_access(namespace, subject, &material.id)
                .await?
            {
                tx.context().require_namespace(namespace)?;
                tx.retain_material_output(namespace, subject, &material.id)
                    .await?;
            }
            let source = ImportInputSource {
                id: ImportInputSourceId::new(),
                namespace_id: namespace.clone(),
                subject_id: subject.clone(),
                name: pending.name,
                format: pending.format,
                material_id: material.id,
                content_hash: material.content_hash,
                byte_len: u64::try_from(material.byte_len).map_err(Error::dependency)?,
                original_job_id: job.id.clone(),
                original_input_index: input_index,
                recorded_at: tx.time().recorded(),
                commit_seq: tx.commit_seq(),
            };
            tx.insert_import_input_source(&source).await?;
            inputs.push(ImportInputRef {
                input_index,
                source_id: source.id,
            });
        }
        Ok((inputs, catalog.records))
    }
}
fn validate_document(document: &ImportDocument) -> Result<()> {
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
    Ok(())
}
fn add_bytes(total: &mut usize, length: usize) -> Result<()> {
    *total = total.saturating_add(length);
    if *total > import_format::MAX_IMPORT_BYTES {
        return Err(Error::new(
            ErrorCode::ResourceExhausted,
            "combined import input exceeds 4 MiB",
        ));
    }
    Ok(())
}
