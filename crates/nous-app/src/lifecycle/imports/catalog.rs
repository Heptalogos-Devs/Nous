mod read;
use super::{input::ImportSourceScope, text, *};
use nous_domain::media::{SourceCoordinate, SourceRegion, SourceRegionId};
use std::collections::BTreeSet;

impl App {
    pub async fn freeze_import_catalog(
        &self,
        ctx: &CallContext,
        operation_id: &str,
        request: FreezeImportCatalogRequest,
        cancel: &CancellationToken,
    ) -> Result<OperationReceipt> {
        ctx.require_subject(&request.namespace_id, &request.subject_id)?;
        if request.inputs.is_empty()
            || request.inputs.len() > import_format::MAX_IMPORT_RECORDS
            || request
                .inputs
                .iter()
                .any(|d| d.format != ImportFormat::Text)
        {
            return Err(Error::invalid(
                "unit catalog requires 1 to 1000 format=text documents",
            ));
        }
        let key = operation_key(
            ctx,
            "import.catalog.freeze",
            &format!(
                "namespace:{}/subject:{}",
                request.namespace_id, request.subject_id
            ),
            operation_id,
            &request,
        )?;
        if let Some(receipt) = self.database.lookup_operation(&key).await? {
            return Ok(receipt);
        }
        let protection = self.objects.protect_references(cancel).await?;
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
            request.overrides,
        )?;
        let mut prepared = Vec::new();
        let mut total = 0usize;
        let mut count = 0usize;
        for (index, document) in request.inputs.iter().enumerate() {
            let (pending, content) = self
                .capture_import_document(
                    ctx,
                    &request.namespace_id,
                    document,
                    import_format::MAX_IMPORT_BYTES - total,
                    cancel,
                )
                .await
                .map_err(|error| error.with_context("input_index", index))?;
            total = total.saturating_add(pending.byte_len()?);
            let preview = text::split_content(
                content.map_err(|e| e.with_context("input_index", index))?,
                options.clone(),
                cancel,
            )
            .await?;
            count = count.saturating_add(preview.units.len());
            if count > import_format::MAX_IMPORT_RECORDS {
                return Err(Error::new(
                    ErrorCode::ResourceExhausted,
                    "unit catalog exceeds 1000 units",
                ));
            }
            prepared.push((pending, preview));
        }
        if count == 0 {
            return Err(Error::invalid("unit catalog has no text units"));
        }
        let mut tx = match self.database.begin_operation(key).await? {
            BeginOperation::Replay(receipt) => return Ok(receipt),
            BeginOperation::New(tx) => tx,
        };
        tx.retain_until_end(protection);
        tx.subject(&request.namespace_id, &request.subject_id)
            .await?;
        let scope = ImportSourceScope {
            namespace: &request.namespace_id,
            subject: &request.subject_id,
            original_job_id: None,
        };
        let mut documents = Vec::new();
        let mut sources = Vec::new();
        for (index, (pending, preview)) in prepared.into_iter().enumerate() {
            let (input, material) = self
                .register_import_source(&mut tx, &scope, index, pending)
                .await?;
            documents.push(ImportUnitDocumentPlan {
                input_source_id: input.id.clone(),
                effective_options: preview.effective_options,
                selected_strategy: preview.selected_strategy,
            });
            sources.push((input, material, preview.units));
        }
        let catalog = ImportUnitCatalog {
            id: ImportUnitCatalogId::new(),
            namespace_id: request.namespace_id,
            subject_id: request.subject_id,
            parent_catalog_id: request.parent_catalog_id,
            documents,
            original_unit_count: count,
            recorded_at: tx.time().recorded(),
            commit_seq: tx.commit_seq(),
        };
        tx.insert_import_unit_catalog(&catalog).await?;
        let mut unit_ids = Vec::new();
        let mut normalized = 0usize;
        for (input, material, drafts) in sources {
            for draft in drafts {
                let source_region = self
                    .freeze_text_region(
                        &mut tx,
                        &material,
                        draft.start_byte,
                        draft.end_byte,
                        Some(&draft.text),
                        cancel,
                    )
                    .await?;
                let mut context_region_ids = BTreeSet::new();
                if let Some(heading) = draft
                    .context
                    .get("heading_range")
                    .and_then(serde_json::Value::as_object)
                {
                    let start = heading
                        .get("start_byte")
                        .and_then(serde_json::Value::as_u64)
                        .ok_or_else(|| Error::internal("split context lacks heading start"))?;
                    let end = heading
                        .get("end_byte")
                        .and_then(serde_json::Value::as_u64)
                        .ok_or_else(|| Error::internal("split context lacks heading end"))?;
                    let region = self
                        .freeze_text_region(&mut tx, &material, start, end, None, cancel)
                        .await?;
                    context_region_ids.insert(region);
                }
                let unit = ImportTextUnit {
                    id: ImportUnitId::new(),
                    namespace_id: catalog.namespace_id.clone(),
                    subject_id: catalog.subject_id.clone(),
                    catalog_id: catalog.id.clone(),
                    ordinal: unit_ids.len(),
                    input_source_id: input.id.clone(),
                    source_region_id: source_region,
                    text: draft.text,
                    effective_strategy: draft.effective_strategy,
                    context: draft.context,
                    context_region_ids: context_region_ids.into_iter().collect(),
                    recorded_at: tx.time().recorded(),
                    commit_seq: tx.commit_seq(),
                };
                normalized = normalized.saturating_add(serde_json::to_vec(&unit)?.len());
                if normalized > import_format::MAX_IMPORT_BYTES {
                    return Err(Error::new(
                        ErrorCode::ResourceExhausted,
                        "frozen unit catalog exceeds 4 MiB",
                    ));
                }
                tx.insert_import_text_unit(&unit).await?;
                unit_ids.push(unit.id);
            }
        }
        tx.commit(json!({"catalog_id":catalog.id,"unit_ids":unit_ids,"unit_count":count}))
            .await
    }
    async fn freeze_text_region(
        &self,
        tx: &mut WriteTransaction,
        material: &Material,
        start: u64,
        end: u64,
        expected: Option<&str>,
        cancel: &CancellationToken,
    ) -> Result<SourceRegionId> {
        let actual = self
            .sources
            .verify_text_span(material, start, end, cancel)
            .await?;
        if expected.is_some_and(|text| text != actual) {
            return Err(Error::new(
                ErrorCode::Conflict,
                "frozen unit text differs from its exact original bytes",
            ));
        }
        let value = SourceRegion {
            id: SourceRegionId::new(),
            namespace_id: material.namespace_id.clone(),
            material_id: material.id.clone(),
            parent: None,
            coordinate: SourceCoordinate::TextSpan { start, end },
            coordinate_hash: String::new(),
            recorded_at: tx.time().recorded(),
            commit_seq: tx.commit_seq(),
        };
        Ok(tx.put_source_region(value).await?.id)
    }
}
