use crate::App;
use crate::import_format;
use nous_domain::*;
use nous_storage::operation_key;
use nous_storage::{BeginOperation, OperationReceipt};
use serde_json::json;

impl App {
    pub async fn import_text(
        &self,
        ctx: &CallContext,
        request: ImportRequest,
    ) -> Result<ImportResult> {
        ctx.require_subject(&request.namespace_id, &request.subject_id)?;
        self.database
            .subject(&request.namespace_id, &request.subject_id)
            .await?;
        let records =
            import_format::parse(&request.content, request.format, request.source.as_deref())?;
        let mut result = ImportResult {
            items: Vec::with_capacity(records.len()),
            accepted: 0,
            replayed: 0,
            rejected: 0,
        };
        for parsed in records {
            let item = match (parsed.record, parsed.error) {
                (Some(record), None) => match self.import_record(ctx, &request, record).await {
                    Ok((receipt, replayed)) => {
                        if replayed {
                            result.replayed += 1;
                        } else {
                            result.accepted += 1;
                        }
                        ImportItemResult {
                            index: parsed.index,
                            external_id: parsed.external_id,
                            status: if replayed { "replayed" } else { "accepted" }.into(),
                            receipt: Some(serde_json::to_value(receipt)?),
                            error: None,
                        }
                    }
                    Err(error) => {
                        result.rejected += 1;
                        ImportItemResult {
                            index: parsed.index,
                            external_id: parsed.external_id,
                            status: "rejected".into(),
                            receipt: None,
                            error: Some(error),
                        }
                    }
                },
                (_, error) => {
                    result.rejected += 1;
                    ImportItemResult {
                        index: parsed.index,
                        external_id: parsed.external_id,
                        status: "rejected".into(),
                        receipt: None,
                        error: Some(
                            error.unwrap_or_else(|| Error::invalid("import record is missing")),
                        ),
                    }
                }
            };
            result.items.push(item);
        }
        Ok(result)
    }

    async fn import_record(
        &self,
        ctx: &CallContext,
        request: &ImportRequest,
        record: ImportRecord,
    ) -> Result<(OperationReceipt, bool)> {
        validate_name(&record.external_id, "external_id")?;
        if record.text.trim().is_empty() || record.text.len() > 1024 * 1024 {
            return Err(Error::invalid("import text must contain 1 byte to 1 MiB"));
        }
        if !record.metadata.is_object()
            || record.tags.len() > 128
            || record
                .tags
                .iter()
                .any(|tag| validate_name(tag, "tag").is_err())
        {
            return Err(Error::invalid(
                "import metadata must be an object and tags must contain at most 128 valid names",
            ));
        }
        if record
            .source
            .as_ref()
            .is_some_and(|value| value.len() > 4096)
        {
            return Err(Error::invalid("import source exceeds 4096 bytes"));
        }
        if let Some(value) = &record.source_type {
            validate_name(value, "source_type")?;
        }
        if let Some(time) = &record.event_time {
            time.validate()?;
        }
        if let Some(time) = &record.observed_at {
            time.validate()?;
        }
        let scope = format!(
            "namespace:{}/subject:{}",
            request.namespace_id, request.subject_id
        );
        let mut payload = json!({"subject_id":request.subject_id,"form_memory":request.form_memory,"record":record});
        // serde_json's canonical object ordering preserves exact i64 time coordinates.
        payload.sort_all_objects();
        let key = operation_key(ctx, "import.record", &scope, &record.external_id, &payload)?;
        let published = self
            .objects
            .publish(record.text.as_bytes().to_vec())
            .await?;
        let mut tx = match self.database.begin_operation(key).await? {
            BeginOperation::Replay(receipt) => return Ok((receipt, true)),
            BeginOperation::New(tx) => tx,
        };
        let subject = tx
            .subject(&request.namespace_id, &request.subject_id)
            .await?;
        if !subject.capabilities.contains(&Capability::Memory) {
            return Err(Error::new(
                ErrorCode::CapabilityUnavailable,
                "subject has no memory capability",
            ));
        }
        let now = self.clock.utc();
        let recorded_at = now.to_rfc3339();
        let clock = TimePoint {
            domain: "unix_ms".into(),
            coordinate: now.timestamp_millis(),
        };
        let material = Material {
            id: MaterialId::new(),
            namespace_id: request.namespace_id.clone(),
            content_hash: published.digest,
            media_type: "text/plain; charset=utf-8".into(),
            byte_len: published.byte_len,
            source: record.source,
            event_time: record.event_time,
            recorded_at: recorded_at.clone(),
            commit_seq: tx.commit_seq(),
        };
        let observation = Observation {
            id: ObservationId::new(),
            namespace_id: request.namespace_id.clone(),
            subject_id: request.subject_id.clone(),
            material_id: material.id.clone(),
            observed_at: record.observed_at,
            recorded_at: recorded_at.clone(),
            commit_seq: tx.commit_seq(),
            kind: ObservationKind::Import,
        };
        let structured_preserved = record.structured.is_some();
        let details = MaterialDetails {
            source_type: record.source_type,
            tags: record.tags,
            metadata: record.metadata,
            structured: record.structured,
        };
        tx.insert_material(&material).await?;
        tx.set_material_details(&request.namespace_id, &material.id, &details)
            .await?;
        tx.insert_observation(&observation).await?;
        let memory = Memory {
            id: MemoryId::new(),
            namespace_id: request.namespace_id.clone(),
            subject_id: request.subject_id.clone(),
            revision: 1,
            title: record
                .text
                .lines()
                .next()
                .unwrap_or("Imported memory")
                .chars()
                .take(120)
                .collect(),
            content: record.text,
            epistemic_status: "reported".into(),
            observation_id: observation.id.clone(),
            material_id: material.id.clone(),
            model_call_id: None,
            origin: MemoryOrigin::Imported,
            times: CognitiveTimes {
                event: material.event_time.clone(),
                observation: observation.observed_at.clone(),
                formation: clock.clone(),
                validity: None,
                recorded: clock.clone(),
            },
            recorded_at: recorded_at.clone(),
            deleted: false,
            lifecycle: MemoryLifecycle::new(clock.coordinate),
            commit_seq: tx.commit_seq(),
        };
        tx.save_memory(&memory, None).await?;
        tx.append_memory_audit(&MemoryAudit {id:ulid::Ulid::generate().to_string(),namespace_id:memory.namespace_id.clone(),subject_id:memory.subject_id.clone(),memory_id:memory.id.clone(),action:"import".into(),actor:ctx.caller_id.clone(),reason:"faithful source admission".into(),previous_revision:0,revision:1,recorded_at:recorded_at.clone(),commit_seq:tx.commit_seq(),details:json!({"external_id":record.external_id,"origin":"imported","structured_preserved":structured_preserved})}).await?;
        let formation_job_id = if request.form_memory {
            let job = self.tasks.new_job(
                &tx,
                "memory.form",
                Some(request.namespace_id.clone()),
                Some(request.subject_id.clone()),
            );
            let id = job.id.clone();
            let run = FormationRun {
                job,
                observation_id: observation.id.clone(),
                material_id: material.id.clone(),
                subject_revision: subject.revision,
                material_hash: material.content_hash.clone(),
                memory_ids: vec![],
            };
            self.tasks.submit_in(&mut tx, &run.job).await?;
            tx.insert_formation(&run).await?;
            Some(id)
        } else {
            None
        };
        let receipt=tx.commit(json!({"external_id":record.external_id,"material_id":material.id,"observation_id":observation.id,"memory_id":memory.id,"formation_job_id":formation_job_id,"structured_preserved":structured_preserved})).await?;
        Ok((receipt, false))
    }

    pub async fn material_details(
        &self,
        ctx: &CallContext,
        id: &MaterialId,
    ) -> Result<MaterialDetails> {
        Ok(self.get_material(ctx, id).await?.details)
    }
}
