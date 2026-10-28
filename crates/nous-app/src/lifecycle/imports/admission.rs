use super::{
    catalog::CatalogSource,
    input::{ImportSourceScope, PendingInput},
    processing::FilePublicationIntent,
    *,
};
use nous_storage::ImportFileLookup;

struct PreparedFile {
    pending: PendingInput,
    identity: Option<String>,
    key: Option<ImportManifestKey>,
    candidate: Option<ImportFileSuccess>,
    success: Option<ImportFileSuccess>,
    preview: Option<TextImportPreview>,
}
impl App {
    pub async fn admit_import_files(
        &self,
        ctx: &CallContext,
        operation_id: &str,
        request: AdmitImportFilesRequest,
        cancel: &CancellationToken,
    ) -> Result<OperationReceipt> {
        ctx.require_subject(&request.namespace_id, &request.subject_id)?;
        if let Some(origin) = &request.origin {
            origin.validate()?;
        }
        if request.inputs.is_empty() || request.inputs.len() > import_format::MAX_IMPORT_RECORDS {
            return Err(Error::invalid(
                "file admission requires an ordered nonempty document catalog",
            ));
        }
        for input in &request.inputs {
            if input.document.format != ImportFormat::Text {
                return Err(Error::new(
                    ErrorCode::CapabilityUnavailable,
                    "file admission currently requires its text unit processor; JSON retains its own parser contract",
                ));
            }
            if input.file_identity.as_ref().is_some_and(|identity| {
                identity.is_empty()
                    || identity.len() > 4096
                    || identity.chars().any(char::is_control)
            }) {
                return Err(Error::invalid(
                    "file identity must contain 1..4096 bytes without controls",
                ));
            }
        }
        let key = operation_key(
            ctx,
            "import.files.admit",
            &format!(
                "namespace:{}/subject:{}",
                request.namespace_id, request.subject_id
            ),
            operation_id,
            &request,
        )?;
        loop {
            nous_foundation::execution::check_cancelled(cancel)?;
            if let Some(receipt) = self.database.lookup_operation(&key).await? {
                return Ok(receipt);
            }
            let protection = self.objects.protect_references(cancel).await?;
            let prepared = self.prepare_import_files(ctx, &request, cancel).await?;
            let mut tx = match self.database.begin_operation(key.clone()).await? {
                BeginOperation::Replay(receipt) => return Ok(receipt),
                BeginOperation::New(tx) => tx,
            };
            tx.retain_until_end(protection);
            if request.clear_manifest {
                tx.clear_import_manifest_heads(
                    &request.namespace_id,
                    &request.subject_id,
                    &request.origin,
                )
                .await?;
            }
            let mut stale = false;
            if !request.force && !request.clear_manifest {
                for file in &prepared {
                    if let Some(key) = &file.key {
                        let current = tx
                            .eligible_import_file_success(
                                ctx,
                                &ImportFileLookup {
                                    namespace: &request.namespace_id,
                                    subject: &request.subject_id,
                                    origin: &request.origin,
                                    policy: request.dedupe_policy,
                                    key,
                                    content_hash: file.pending.content_hash(),
                                    vectors: request.vectors,
                                },
                            )
                            .await?;
                        if current.as_ref().map(|fact| &fact.id)
                            != file.candidate.as_ref().map(|fact| &fact.id)
                        {
                            stale = true;
                            break;
                        }
                    }
                }
            }
            if stale {
                tx.rollback().await?;
                continue;
            }
            let scope = ImportSourceScope {
                namespace: &request.namespace_id,
                subject: &request.subject_id,
                original_job_id: None,
            };
            let mut outcomes = Vec::new();
            let mut sources = Vec::new();
            let mut intents = Vec::new();
            for (index, file) in prepared.into_iter().enumerate() {
                let (input, material) = self
                    .register_import_source(&mut tx, &scope, index, file.pending)
                    .await?;
                let decision = if let Some(success) = file.success {
                    ImportFileAdmissionDecision::Skip {
                        success_id: success.id,
                    }
                } else {
                    intents.push(FilePublicationIntent {
                        input_source_id: input.id.clone(),
                        file_identity: file.identity,
                        dedupe_policy: request.dedupe_policy,
                    });
                    sources.push(CatalogSource {
                        input: input.clone(),
                        material,
                        preview: file.preview.ok_or_else(|| {
                            Error::internal("new file has no actual split preparation")
                        })?,
                    });
                    ImportFileAdmissionDecision::Process
                };
                outcomes.push(ImportFileAdmission {
                    input_index: index,
                    input_source_id: input.id,
                    decision,
                });
            }
            let (catalog_id, processing_job_id) = if sources.is_empty() {
                (None, None)
            } else {
                let (catalog, units) = self
                    .freeze_catalog_in(
                        &mut tx,
                        &request.namespace_id,
                        &request.subject_id,
                        None,
                        sources,
                        cancel,
                    )
                    .await?;
                let job = self
                    .submit_processing_in(
                        &mut tx,
                        ProcessImportUnitsRequest {
                            namespace_id: request.namespace_id.clone(),
                            subject_id: request.subject_id.clone(),
                            catalog_id: catalog.id.clone(),
                            unit_ids: Some(units.into_iter().map(|unit| unit.id).collect()),
                            llm_enabled: request.llm_enabled,
                            extract_time: request.extract_time,
                            vectors: request.vectors,
                            origin: request.origin.clone(),
                            retry_parent_job_id: None,
                        },
                        intents,
                    )
                    .await?;
                (Some(catalog.id), Some(job.id))
            };
            return tx
                .commit(json!(ImportFilesAdmission {
                    inputs: outcomes,
                    catalog_id,
                    processing_job_id
                }))
                .await;
        }
    }

    async fn prepare_import_files(
        &self,
        ctx: &CallContext,
        request: &AdmitImportFilesRequest,
        cancel: &CancellationToken,
    ) -> Result<Vec<PreparedFile>> {
        let options = text::policy::effective(
            self.configuration
                .snapshot(
                    &ConfigScope::Subject {
                        namespace_id: request.namespace_id.clone(),
                        subject_id: request.subject_id.clone(),
                    },
                    false,
                )
                .await?
                .get(text::policy::IMPORT_TEXT)?,
            request.overrides.clone(),
        )?;
        let mut files = Vec::new();
        let mut total = 0usize;
        for (index, input) in request.inputs.iter().enumerate() {
            let (pending, content) = self
                .capture_import_document(
                    ctx,
                    &request.namespace_id,
                    &input.document,
                    import_format::MAX_IMPORT_BYTES - total,
                    cancel,
                )
                .await?;
            total = total.saturating_add(pending.byte_len()?);
            let content = content.map_err(|error| error.with_context("input_index", index))?;
            let key = request
                .dedupe_policy
                .key(pending.content_hash(), input.file_identity.as_deref());
            let mut candidate = None;
            let mut success = None;
            if !request.force
                && !request.clear_manifest
                && let Some(key) = &key
            {
                let mut view = self.database.read_view().await?;
                candidate = view
                    .eligible_import_file_success(
                        ctx,
                        &ImportFileLookup {
                            namespace: &request.namespace_id,
                            subject: &request.subject_id,
                            origin: &request.origin,
                            policy: request.dedupe_policy,
                            key,
                            content_hash: pending.content_hash(),
                            vectors: request.vectors,
                        },
                    )
                    .await?;
                let previous = if let Some(fact) = &candidate {
                    Some(
                        view.import_input_source(
                            &request.namespace_id,
                            &request.subject_id,
                            &fact.input_source_id,
                        )
                        .await?,
                    )
                } else {
                    None
                };
                view.finish().await?;
                if let Some(previous) = previous {
                    let document = ImportDocument {
                        name: None,
                        format: ImportFormat::Text,
                        input: ImportDocumentInput::Material {
                            material_id: previous.material_id,
                        },
                        source: None,
                    };
                    match self
                        .read_import_document(
                            ctx,
                            &request.namespace_id,
                            &document,
                            import_format::MAX_IMPORT_BYTES,
                            cancel,
                        )
                        .await
                    {
                        Ok(read) if read.content.as_ref().is_ok_and(|text| text == &content) => {
                            success = candidate.clone()
                        }
                        Ok(_) => {}
                        Err(error)
                            if matches!(error.code, ErrorCode::Forbidden | ErrorCode::NotFound) => {
                        }
                        Err(error) => return Err(error),
                    }
                }
            }
            let preview = if success.is_none() {
                Some(text::split_content(content, options.clone(), cancel).await?)
            } else {
                None
            };
            files.push(PreparedFile {
                pending,
                identity: input.file_identity.clone(),
                key,
                candidate,
                success,
                preview,
            });
        }
        Ok(files)
    }
}
