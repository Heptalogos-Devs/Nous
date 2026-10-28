use super::*;
use nous_domain::media::PutMediaRequest;
use nous_foundation::{content::ReferenceLease, media::CapturedMedia};

pub(super) struct ImportSourceScope<'a> {
    pub namespace: &'a NamespaceId,
    pub subject: &'a SubjectId,
    pub original_job_id: Option<&'a str>,
}

pub(super) struct InputCatalog {
    inputs: Vec<PendingInput>,
    pub records: Vec<ParsedImportRecord>,
    _protection: ReferenceLease,
}
pub(super) struct PendingInput {
    name: Option<String>,
    format: ImportFormat,
    source: Option<String>,
    bytes: InputBytes,
}
pub(super) enum InputBytes {
    Captured(CapturedMedia),
    Material(Material),
}
impl PendingInput {
    pub(super) fn byte_len(&self) -> Result<usize> {
        usize::try_from(match &self.bytes {
            InputBytes::Captured(value) => value.object.byte_len,
            InputBytes::Material(value) => value.byte_len,
        })
        .map_err(Error::dependency)
    }
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
            let (pending, content) = self
                .capture_import_document(
                    ctx,
                    &request.namespace_id,
                    document,
                    import_format::MAX_IMPORT_BYTES - total,
                    cancel,
                )
                .await
                .map_err(|error| error.with_context("input_index", input_index))?;
            let length = match &pending.bytes {
                InputBytes::Captured(value) => value.object.byte_len,
                InputBytes::Material(value) => value.byte_len,
            };
            add_bytes(
                &mut total,
                usize::try_from(length).map_err(Error::dependency)?,
            )?;
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
            inputs.push(pending);
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
        let scope = ImportSourceScope {
            namespace,
            subject,
            original_job_id: Some(&job.id),
        };
        for (input_index, pending) in catalog.inputs.into_iter().enumerate() {
            let (source, _) = self
                .register_import_source(tx, &scope, input_index, pending)
                .await?;
            inputs.push(ImportInputRef {
                input_index,
                source_id: source.id,
            });
        }
        Ok((inputs, catalog.records))
    }
    pub(super) async fn capture_import_document(
        &self,
        ctx: &CallContext,
        namespace: &NamespaceId,
        document: &ImportDocument,
        max_bytes: usize,
        cancel: &CancellationToken,
    ) -> Result<(PendingInput, Result<String>)> {
        let read = self
            .read_import_document(ctx, namespace, document, max_bytes, cancel)
            .await?;
        let bytes = match read.material {
            Some(material) => InputBytes::Material(material),
            None => InputBytes::Captured(
                self.media
                    .capture_text(read.content.as_ref().map_err(Clone::clone)?, cancel)
                    .await?,
            ),
        };
        Ok((
            PendingInput {
                name: document.name.clone(),
                format: document.format,
                source: document.source.clone(),
                bytes,
            },
            read.content,
        ))
    }
    pub(super) async fn register_import_source(
        &self,
        tx: &mut WriteTransaction,
        scope: &ImportSourceScope<'_>,
        input_index: usize,
        pending: PendingInput,
    ) -> Result<(ImportInputSource, Material)> {
        let (namespace, subject) = (scope.namespace, scope.subject);
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
            material_id: material.id.clone(),
            content_hash: material.content_hash.clone(),
            byte_len: u64::try_from(material.byte_len).map_err(Error::dependency)?,
            original_job_id: scope.original_job_id.map(str::to_owned),
            original_input_index: input_index,
            recorded_at: tx.time().recorded(),
            commit_seq: tx.commit_seq(),
        };
        tx.insert_import_input_source(&source).await?;
        Ok((source, material))
    }
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
