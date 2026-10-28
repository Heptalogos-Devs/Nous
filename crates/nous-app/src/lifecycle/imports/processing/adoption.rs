use super::{models::Semantic, time::Temporal, *};
use crate::cognition::structured::StructuredGroupProvenance;
use nous_domain::media::MediaReference;

impl App {
    pub(super) async fn adopt_import_unit(
        &self,
        job: &JobRecord,
        plan: &UnitPlan,
        unit: &ImportTextUnit,
        semantic: Semantic,
        temporal: Temporal,
    ) -> Result<()> {
        let (namespace, subject) = job.subject_scope()?;
        let ctx = job.call_context(&plan.caller_id)?;
        let region = self
            .source_region(&ctx, subject, &unit.source_region_id)
            .await?;
        let compiled = compile::group(unit, &semantic)?;
        let (embedding, mut vector_error) = if plan.vectors == ImportVectorPolicy::Disabled {
            (None, None)
        } else {
            match self
                .embedding_configuration(namespace, subject)
                .await
                .and_then(|config| {
                    config.space()?;
                    Ok(config)
                }) {
                Ok(config) => (Some(config), None),
                Err(error) if plan.vectors == ImportVectorPolicy::MetadataAllowed => {
                    (None, Some(error))
                }
                Err(error) if error.code == ErrorCode::CapabilityUnavailable => {
                    let mut tx = self
                        .tasks
                        .operation("import.unit.index.configuration", &job.id)
                        .await?;
                    let mut current = tx.job_record(&job.id).await?;
                    if current.state != JobState::Running {
                        tx.rollback().await?;
                        return Ok(());
                    }
                    self.tasks
                        .change_in(
                            &mut tx,
                            &mut current,
                            JobState::Waiting,
                            Some(WaitCondition {
                                kind: "configuration".into(),
                                requires_attention: true,
                                resume: ResumeCondition::Ready,
                                details: json!({"resource":"import_vectors"}),
                            }),
                            Some(error),
                        )
                        .await?;
                    tx.commit(json!({"job_id":job.id})).await?;
                    return Ok(());
                }
                Err(error) => return Err(error),
            }
        };
        let mut tx = self.tasks.operation("import.unit.adopt", &job.id).await?;
        let mut current = tx.job_record(&job.id).await?;
        if current.state != JobState::Running || current.result.is_some() {
            tx.rollback().await?;
            return Ok(());
        }
        let subject_record = tx.subject(namespace, subject).await?;
        if !subject_record.capabilities.contains(&Capability::Memory) {
            return Err(Error::new(
                ErrorCode::CapabilityUnavailable,
                "subject has no memory capability",
            ));
        }
        let original = tx.material(namespace, &region.material_id).await?;
        let recorded = tx.time().recorded();
        let point = tx.time().point();
        let observation = Observation {
            id: ObservationId::new(),
            namespace_id: namespace.clone(),
            subject_id: subject.clone(),
            material_id: original.id.clone(),
            observed_at: None,
            origin: plan.origin.clone(),
            recorded_at: recorded.clone(),
            commit_seq: tx.commit_seq(),
            kind: ObservationKind::Import,
        };
        tx.insert_observation(&observation).await?;
        let memory = Memory {
            id: MemoryId::new(),
            namespace_id: namespace.clone(),
            subject_id: subject.clone(),
            revision: 1,
            title: unit
                .text
                .lines()
                .map(str::trim)
                .find(|line| !line.is_empty())
                .or_else(|| {
                    (!unit.context_region_ids.is_empty())
                        .then(|| {
                            unit.context
                                .get("scene_id")
                                .and_then(serde_json::Value::as_str)
                        })
                        .flatten()
                })
                .unwrap_or("导入文本")
                .chars()
                .take(120)
                .collect(),
            content: unit.text.clone(),
            epistemic_status: "reported".into(),
            observation_id: observation.id.clone(),
            material_id: original.id,
            model_call_id: None,
            source: MediaReference::SourceRegion {
                id: unit.source_region_id.clone(),
            },
            origin: MemoryOrigin::Imported,
            times: CognitiveTimes {
                event: temporal.event,
                observation: None,
                formation: point.clone(),
                validity: None,
                recorded: point.clone(),
            },
            recorded_at: recorded.clone(),
            deleted: false,
            lifecycle: MemoryLifecycle::new(point.coordinate),
            commit_seq: tx.commit_seq(),
        };
        tx.save_memory(&memory, None).await?;
        tx.append_memory_audit(&MemoryAudit {
            id: ulid::Ulid::generate().to_string(),
            namespace_id: namespace.clone(),
            subject_id: subject.clone(),
            memory_id: memory.id.clone(),
            action: "import".into(),
            actor: ctx.caller_id.clone(),
            reason: "frozen source unit admission".into(),
            previous_revision: 0,
            revision: 1,
            recorded_at: recorded.clone(),
            commit_seq: tx.commit_seq(),
            details: json!({"unit_id":unit.id,"catalog_id":unit.catalog_id}),
        })
        .await?;
        let cognition = if let Some((group, slots)) = compiled {
            let formation = if let Some(call_id) = &semantic.call_id {
                let call = tx.model_call(call_id).await?;
                let completed =
                    chrono::DateTime::parse_from_rfc3339(call.completed_at.as_deref().ok_or_else(
                        || Error::internal("semantic producer has no completion time"),
                    )?)
                    .map_err(Error::dependency)?;
                TimePoint {
                    domain: "unix_ms".into(),
                    coordinate: completed.timestamp_millis(),
                }
            } else {
                point
            };
            self.apply_structured_group_in(
                &mut tx,
                &group,
                &slots,
                StructuredGroupProvenance {
                    namespace_id: namespace.clone(),
                    subject_id: subject.clone(),
                    producer_call_id: semantic.call_id.clone(),
                    formation,
                },
            )
            .await?
            .into_iter()
            .map(|object| object.reference())
            .collect()
        } else {
            vec![]
        };
        let output = ImportUnitOutput {
            id: ImportUnitOutputId::new(),
            namespace_id: namespace.clone(),
            subject_id: subject.clone(),
            unit_id: unit.id.clone(),
            attempt_job_id: job.id.clone(),
            memory_ids: vec![memory.id.clone()],
            cognition,
            semantic_call_id: semantic.call_id,
            temporal_call_id: temporal.call_id,
            event_descriptions: semantic.events,
            event_range: temporal.event_range,
            temporal_error: temporal.error,
            recorded_at: recorded,
            commit_seq: tx.commit_seq(),
        };
        tx.insert_import_unit_output(&output).await?;
        let vector_job_ids = if let Some(config) = embedding {
            let targets = std::iter::once(CognitiveRef::memory(&memory.id, memory.revision))
                .chain(
                    output
                        .cognition
                        .iter()
                        .filter(|_| unit.effective_strategy != ImportTextStrategy::Quote)
                        .cloned(),
                )
                .collect::<Vec<_>>();
            match self
                .prepare_cognitive_embeddings_in(&mut tx, namespace, subject, targets, &config)
                .await
            {
                Ok(inputs) => self
                    .admit_embedding_inputs_in(&mut tx, namespace, subject, inputs, &config)
                    .await?
                    .jobs
                    .into_iter()
                    .collect(),
                Err(error)
                    if plan.vectors == ImportVectorPolicy::MetadataAllowed
                        && matches!(
                            error.code,
                            ErrorCode::CapabilityUnavailable | ErrorCode::ResourceExhausted
                        ) =>
                {
                    vector_error = Some(error);
                    vec![]
                }
                Err(error) => return Err(error),
            }
        } else {
            vec![]
        };
        let result = ImportUnitProcessResult {
            output_id: output.id.clone(),
            vector_job_ids,
            vector_error,
        };
        current.result = Some(json!(result));
        if plan.vectors == ImportVectorPolicy::Required {
            if self
                .tasks
                .suspend_until_in(
                    &mut tx,
                    &mut current,
                    result.vector_job_ids.clone(),
                    DependencyPolicy::AllSucceeded,
                )
                .await?
                == DependencyResolution::Ready
            {
                self.tasks
                    .complete_in(&mut tx, &mut current, json!(result))
                    .await?;
            }
        } else {
            self.tasks
                .complete_in(&mut tx, &mut current, json!(result))
                .await?;
        }
        tx.commit(json!({"job_id":job.id,"output_id":output.id}))
            .await?;
        Ok(())
    }
}
