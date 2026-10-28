use super::*;
use nous_storage::{BeginOperation, operation_key};

impl App {
    pub(super) async fn accept_import_record(
        &self,
        job: &JobRecord,
        plan: &ImportPlan,
        parsed: &ParsedImportRecord,
        record: ImportRecord,
        cancel: &CancellationToken,
    ) -> Result<()> {
        let (namespace, subject_id) = job.subject_scope()?;
        let ctx = job.call_context(&plan.caller_id)?;
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
        let scope = format!("namespace:{}/subject:{}", namespace, subject_id);
        let mut payload =
            json!({"subject_id":subject_id,"form_memory":plan.form_memory,"record":record});
        // serde_json's canonical object ordering preserves exact i64 time coordinates.
        payload.sort_all_objects();
        let key = operation_key(&ctx, "import.record", &scope, &record.external_id, &payload)?;
        validate_origin(&record)?;
        let published = self
            .objects
            .publish_bytes(record.text.as_bytes(), 1024 * 1024, cancel)
            .await?;
        let mut tx = match self.database.begin_operation(key).await? {
            BeginOperation::Replay(receipt) => {
                let admission: ImportAdmission = serde_json::from_value(receipt.result)?;
                return self
                    .import_progress(
                        job,
                        plan,
                        parsed,
                        Some((admission, receipt.commit_seq, true)),
                        None,
                    )
                    .await;
            }
            BeginOperation::New(tx) => tx,
        };
        let Some(mut current) = current_for(&mut tx, &job.id, parsed.index).await? else {
            tx.rollback().await?;
            return Ok(());
        };
        let subject = tx.subject(namespace, subject_id).await?;
        published.protect_in(&mut tx);
        if !subject.capabilities.contains(&Capability::Memory) {
            return Err(Error::new(
                ErrorCode::CapabilityUnavailable,
                "subject has no memory capability",
            ));
        }
        let recorded_at = tx.time().recorded();
        let clock = tx.time().point();
        let material = Material {
            id: MaterialId::new(),
            namespace_id: namespace.clone(),
            content_hash: published.digest,
            media_type: "text/plain; charset=utf-8".into(),
            metadata: nous_domain::media::MediaMetadata::Text,
            byte_len: published.byte_len,
            source: record.source,
            event_time: record.event_time,
            recorded_at: recorded_at.clone(),
            commit_seq: tx.commit_seq(),
        };
        let observation = Observation {
            id: ObservationId::new(),
            namespace_id: namespace.clone(),
            subject_id: subject_id.clone(),
            material_id: material.id.clone(),
            observed_at: record.observed_at,
            origin: record.origin,
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
        tx.set_material_details(namespace, &material.id, &details)
            .await?;
        tx.insert_observation(&observation).await?;
        let memory = Memory {
            id: MemoryId::new(),
            namespace_id: namespace.clone(),
            subject_id: subject_id.clone(),
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
            source: nous_domain::media::MediaReference::Original {
                material_id: material.id.clone(),
            },
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
        let formation_job_id = if plan.form_memory {
            let job = self.tasks.new_job(
                &tx,
                "memory.form",
                Some(namespace.clone()),
                Some(subject_id.clone()),
            );
            let id = job.id.clone();
            let run = FormationRun {
                job,
                observation_id: observation.id.clone(),
                material_id: material.id.clone(),
                source: nous_domain::media::MediaReference::Original {
                    material_id: material.id.clone(),
                },
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
        let admission = ImportAdmission {
            external_id: record.external_id,
            material_id: material.id,
            observation_id: observation.id,
            memory_id: memory.id,
            formation_job_id,
            structured_preserved,
        };
        let sequence = tx.commit_seq();
        self.append_import_progress(
            &mut tx,
            &mut current,
            parsed,
            plan.input_source(parsed)?,
            Some((admission.clone(), sequence, false)),
            None,
        )
        .await?;
        tx.commit(json!(admission)).await?;
        Ok(())
    }
}
fn validate_origin(record: &ImportRecord) -> Result<()> {
    if let Some(origin) = &record.origin {
        origin.validate()?;
    }
    Ok(())
}
