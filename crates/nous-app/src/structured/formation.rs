//! Durable structured proposals use the shared call ledger and queue.
use super::*;
use crate::calls::{CallRequest, budget_error, complete_redacted_call, settle_call};
use crate::model::{
    RetryClass,
    structured::{RawStructuredProposal, SourceCatalogEntry, StructuredFormationInput},
};
use nous_storage::{
    JobEnvelope, StructuredFormationGroup, StructuredFormationRun, StructuredGroupState,
};
use std::collections::{BTreeSet, HashMap};

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FormationTarget {
    pub key: String,
    pub kind: CognitiveKind,
    pub existing: Option<CognitiveRef>,
}
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FormStructuredRequest {
    pub subject_id: SubjectId,
    pub sources: Vec<EvidenceSource>,
    pub targets: Vec<FormationTarget>,
}

impl App {
    pub async fn form_structured(
        &self,
        ctx: &CallContext,
        operation_id: &str,
        request: FormStructuredRequest,
    ) -> Result<OperationReceipt> {
        let namespace = self.database.subject_namespace(&request.subject_id).await?;
        ctx.require_subject(&namespace, &request.subject_id)?;
        if request.sources.is_empty()
            || request.sources.len() > 128
            || request.targets.is_empty()
            || request.targets.len() > 32
        {
            return Err(Error::invalid(
                "structured formation needs 1..128 sources and 1..32 target slots",
            ));
        }
        let source_context = CallContext {
            caller_id: ctx.caller_id.clone(),
            scope: AccessScope::Subject {
                namespace_id: namespace.clone(),
                subject_id: request.subject_id.clone(),
            },
        };
        let mut sources = Vec::new();
        let mut identities = BTreeSet::new();
        for (ordinal, source) in request.sources.iter().enumerate() {
            source.validate()?;
            if !identities.insert(source) {
                return Err(Error::invalid("duplicate formation source"));
            }
            sources.push(
                self.formation_source(
                    &source_context,
                    &namespace,
                    &request.subject_id,
                    format!("s{ordinal}"),
                    source.clone(),
                )
                .await?,
            );
        }
        let mut slots = Vec::new();
        for target in &request.targets {
            let (id, expected_revision) = if let Some(reference) = &target.existing {
                reference.validate()?;
                if reference.kind != target.kind {
                    return Err(Error::invalid(
                        "target kind differs from existing reference",
                    ));
                }
                (reference.id.parse()?, reference.revision)
            } else {
                (CognitiveId::new(), 0)
            };
            slots.push(StructuredTargetSlot {
                key: target.key.clone(),
                kind: target.kind,
                id,
                expected_revision,
            });
        }
        let scope = format!("namespace:{namespace}/subject:{}", request.subject_id);
        let key = operation_key(
            ctx,
            "cognition.form",
            &scope,
            operation_id,
            &serde_json::to_value(&request)?,
        )?;
        let mut tx = match self.database.begin_operation(key).await? {
            BeginOperation::Replay(receipt) => return Ok(receipt),
            BeginOperation::New(tx) => tx,
        };
        let subject = tx.subject(&namespace, &request.subject_id).await?;
        if !subject.capabilities.contains(&Capability::Memory) {
            return Err(Error::new(
                ErrorCode::CapabilityUnavailable,
                "subject has no memory capability",
            ));
        }
        for slot in &slots {
            if slot.expected_revision > 0 {
                let current = tx.structured_object(&namespace, &slot.id).await?;
                if current.subject_id != request.subject_id
                    || current.revision != slot.expected_revision
                    || current.deleted
                    || current.kind() != slot.kind
                {
                    return Err(Error::new(
                        ErrorCode::Conflict,
                        "formation target is not the current live scoped revision",
                    ));
                }
            }
        }
        let input = StructuredFormationInput {
            namespace_id: namespace.clone(),
            subject_id: request.subject_id.clone(),
            knowledge_cutoff: tx.commit_seq() - 1,
            sources,
            slots,
        };
        input.validate()?;
        let input = serde_json::to_value(input)?;
        if serde_json::to_vec(&input)?.len() > 1024 * 1024 {
            return Err(Error::new(
                ErrorCode::ResourceExhausted,
                "structured input exceeds one MiB",
            ));
        }
        let now = Utc::now().to_rfc3339();
        let job_id = ulid::Ulid::generate().to_string();
        let run = StructuredFormationRun {
            job_id: job_id.clone(),
            namespace_id: namespace.clone(),
            subject_id: request.subject_id.clone(),
            state: FormationState::Queued,
            input,
            output: None,
            model_call_id: None,
            error: None,
            created_at: now.clone(),
            updated_at: now,
        };
        tx.insert_structured_formation(&run).await?;
        tx.enqueue(JobEnvelope {
            id: job_id,
            kind: "cognition.form".into(),
            namespace_id: namespace.to_string(),
            subject_id: Some(request.subject_id.to_string()),
            expected_revision: Some(subject.revision),
            payload: json!({"schema_version":1}),
        })
        .await?;
        tx.commit(json!(run)).await
    }

    async fn formation_source(
        &self,
        ctx: &CallContext,
        namespace: &NamespaceId,
        subject: &SubjectId,
        key: String,
        source: EvidenceSource,
    ) -> Result<SourceCatalogEntry> {
        let mut event_kind = None;
        let text = match &source {
            EvidenceSource::Material { material_id } => {
                self.get_material(ctx, material_id).await?.text
            }
            EvidenceSource::Observation { observation_id } => {
                let observation = self.database.observation(namespace, observation_id).await?;
                if observation.subject_id != *subject {
                    return Err(Error::new(
                        ErrorCode::Forbidden,
                        "source observation belongs to another subject",
                    ));
                }
                event_kind = Some(observation.kind);
                let material = self.get_material(ctx, &observation.material_id).await?;
                serde_json::to_string(&json!({"observation":observation,"material":material}))?
            }
            EvidenceSource::MemoryRevision {
                memory_id,
                revision,
            } => {
                let memory = self
                    .database
                    .memory_history(namespace, memory_id)
                    .await?
                    .into_iter()
                    .find(|m| m.revision == *revision)
                    .ok_or_else(|| {
                        Error::new(ErrorCode::NotFound, "source Memory revision not found")
                    })?;
                if memory.subject_id != *subject {
                    return Err(Error::new(
                        ErrorCode::Forbidden,
                        "source Memory belongs to another subject",
                    ));
                }
                if memory.deleted {
                    return Err(Error::invalid(
                        "deleted Memory cannot supply a formation input",
                    ));
                }
                serde_json::to_string(&memory)?
            }
            EvidenceSource::CognitiveRevision { reference } => {
                if reference.kind == CognitiveKind::Memory {
                    return Err(Error::invalid("use memory_revision for Memory inputs"));
                }
                let object = self
                    .get_structured(ctx, &reference.id.parse()?, Some(reference.revision))
                    .await?;
                if object.kind() != reference.kind || object.deleted {
                    return Err(Error::invalid("source cognition kind or lifecycle differs"));
                }
                serde_json::to_string(&object)?
            }
        };
        Ok(SourceCatalogEntry {
            key,
            source,
            text,
            source_event_kind: event_kind,
            truncated: false,
        })
    }

    pub async fn structured_formation_run(&self, ctx: &CallContext, job_id: &str) -> Result<Value> {
        let run = self.database.structured_formation(job_id).await?;
        ctx.require_subject(&run.namespace_id, &run.subject_id)?;
        let groups = self.database.structured_formation_groups(job_id).await?;
        Ok(json!({"run":run,"groups":groups}))
    }

    pub(crate) async fn process_structured_formation(&self, job_id: &str) -> Result<()> {
        let mut run = self.database.structured_formation(job_id).await?;
        if matches!(
            run.state,
            FormationState::Succeeded | FormationState::Cancelled | FormationState::Failed
        ) {
            return Ok(());
        }
        if run.output.is_some() {
            return self.adopt_structured_groups(&run).await;
        }
        if let Some(id) = &run.model_call_id {
            let call = self.database.model_call(id).await?;
            if call.state == ModelCallState::Started {
                self.mark_interrupted_call(call).await?;
            }
        }
        let input: StructuredFormationInput = serde_json::from_value(run.input.clone())?;
        let prepared = match self.model.prepare_structured(input) {
            Ok(prepared) => prepared,
            Err(failure) => {
                let state = if failure.class() == RetryClass::WaitingConfiguration {
                    FormationState::WaitingForConfiguration
                } else {
                    FormationState::Failed
                };
                return self
                    .structured_formation_state(&run, state, failure.to_domain_error())
                    .await;
            }
        };
        let metadata = &prepared.metadata;
        let Some(reservation) = self
            .reserve_call(CallRequest {
                job_id,
                namespace_id: &run.namespace_id,
                subject_id: &run.subject_id,
                call_id: metadata.call_id.parse()?,
                resource_id: &metadata.resource.id,
                max_calls_per_day: metadata.max_calls_per_day,
                request: serde_json::to_value(metadata)?,
            })
            .await?
        else {
            return Ok(());
        };
        let mut tx = reservation.transaction;
        if tx.structured_formation(job_id).await?.is_retired() {
            tx.rollback().await?;
            return Ok(());
        }
        let Some(mut call) = reservation.call else {
            run.state = FormationState::WaitingForBudget;
            run.error = Some(budget_error());
            run.updated_at = Utc::now().to_rfc3339();
            tx.update_structured_formation(&run).await?;
            tx.commit(json!({"state":run.state})).await?;
            return Ok(());
        };
        run.state = FormationState::Running;
        run.error = None;
        run.model_call_id = Some(call.id.clone());
        run.updated_at = call.created_at.clone();
        tx.update_structured_formation(&run).await?;
        tx.commit(json!({"model_call_id":call.id})).await?;
        let execution = self.model.execute_structured(prepared).await;
        let (proposal, failure) = match execution.result {
            Ok(proposal) => (Some(proposal), None),
            Err(failure) => (None, Some(failure)),
        };
        settle_call(
            &mut call,
            execution.record.completed_at.clone(),
            json!({"schema_version":1,"record":execution.record,"proposal":proposal}),
            failure.as_ref(),
        );
        if let Some(failure) = &failure {
            run.state = if failure.class() == RetryClass::Permanent || call.attempt >= 5 {
                FormationState::Failed
            } else {
                FormationState::Queued
            };
            run.error = Some(failure.to_domain_error());
        } else {
            run.output = Some(json!(proposal));
        }
        run.updated_at = call.completed_at.clone().expect("settled timestamp");
        if let Some(mut tx) = self
            .worker_transaction(
                "model.complete",
                call.id.as_str(),
                job_id,
                &json!({"call_id":call.id}),
            )
            .await?
        {
            if tx.structured_formation(job_id).await?.is_retired() {
                let mut retired = tx.model_call(&call.id).await?;
                complete_redacted_call(&mut retired, &call);
                tx.update_model_call(&retired).await?;
                tx.commit(json!({"call_id":call.id,"state":retired.state,"source_retired":true}))
                    .await?;
                return Ok(());
            }
            tx.update_model_call(&call).await?;
            tx.update_structured_formation(&run).await?;
            if let Some(proposal) = &proposal {
                let groups = proposal
                    .groups
                    .iter()
                    .enumerate()
                    .map(|(ordinal, group)| StructuredFormationGroup {
                        key: group.key.clone(),
                        ordinal: ordinal as i64,
                        depends_on: group.depends_on.clone(),
                        state: StructuredGroupState::Pending,
                        result: None,
                        error: None,
                        commit_seq: None,
                    })
                    .collect::<Vec<_>>();
                tx.insert_structured_groups(job_id, &groups).await?;
            }
            tx.commit(json!({"model_call_id":call.id,"state":call.state}))
                .await?;
        }
        if let Some(failure) = failure {
            if run.state == FormationState::Failed {
                return Ok(());
            }
            tokio::time::sleep(std::time::Duration::from_secs(
                failure.info.retry_after_seconds.unwrap_or(2).min(30),
            ))
            .await;
            return Err(failure.to_domain_error());
        }
        self.adopt_structured_groups(&run).await
    }

    async fn structured_formation_state(
        &self,
        run: &StructuredFormationRun,
        state: FormationState,
        error: Error,
    ) -> Result<()> {
        if let Some(mut tx) = self
            .worker_transaction(
                "cognition.formation.state",
                &ulid::Ulid::generate().to_string(),
                &run.job_id,
                &json!({"state":state,"error":error}),
            )
            .await?
        {
            let mut current = tx.structured_formation(&run.job_id).await?;
            if current.is_retired() {
                tx.rollback().await?;
                return Ok(());
            }
            current.state = state;
            current.error = Some(error);
            current.updated_at = Utc::now().to_rfc3339();
            tx.update_structured_formation(&current).await?;
            tx.commit(json!({"state":current.state})).await?;
        }
        Ok(())
    }

    async fn adopt_structured_groups(&self, run: &StructuredFormationRun) -> Result<()> {
        if self
            .database
            .structured_formation(&run.job_id)
            .await?
            .is_retired()
        {
            return Ok(());
        }
        let input: StructuredFormationInput = serde_json::from_value(run.input.clone())?;
        let proposal: RawStructuredProposal = serde_json::from_value(
            run.output
                .clone()
                .ok_or_else(|| Error::internal("structured output missing"))?,
        )?;
        let slots = input
            .slots
            .iter()
            .map(|s| (s.key.clone(), s))
            .collect::<HashMap<_, _>>();
        let owners = proposal
            .groups
            .iter()
            .flat_map(|g| {
                g.changes
                    .iter()
                    .map(move |c| (c.slot_key.clone(), g.key.clone()))
            })
            .collect::<HashMap<_, _>>();
        let call_id = run
            .model_call_id
            .as_ref()
            .ok_or_else(|| Error::internal("structured output has no producer"))?;
        let call = self.database.model_call(call_id).await?;
        let formed = chrono::DateTime::parse_from_rfc3339(
            call.completed_at
                .as_deref()
                .ok_or_else(|| Error::internal("producer timestamp missing"))?,
        )
        .map_err(|e| Error::internal(e.to_string()))?
        .timestamp_millis();
        let mut ancestors = HashMap::<String, BTreeSet<String>>::new();
        for group in &proposal.groups {
            let mut allowed = BTreeSet::from([group.key.clone()]);
            for dependency in &group.depends_on {
                allowed.insert(dependency.clone());
                if let Some(previous) = ancestors.get(dependency) {
                    allowed.extend(previous.clone());
                }
            }
            ancestors.insert(group.key.clone(), allowed.clone());
            let states = self
                .database
                .structured_formation_groups(&run.job_id)
                .await?;
            let mut state = states
                .iter()
                .find(|s| s.key == group.key)
                .cloned()
                .ok_or_else(|| Error::internal("proposal group missing"))?;
            if state.state != StructuredGroupState::Pending {
                continue;
            }
            let failed_dependency = group.depends_on.iter().any(|key| {
                states
                    .iter()
                    .any(|s| &s.key == key && s.state != StructuredGroupState::Applied)
            });
            let result = if failed_dependency {
                Err(Error::new(
                    ErrorCode::Conflict,
                    "dependent proposal group was not applied",
                ))
            } else {
                match group.compile(&input) {
                    Err(error) => Err(error),
                    Ok(compiled) => {
                        self.apply_structured_group(
                            run, &compiled, &slots, &owners, &allowed, call_id, formed,
                        )
                        .await
                    }
                }
            };
            if let Err(error) = result {
                if matches!(
                    error.code,
                    ErrorCode::Internal | ErrorCode::DependencyUnavailable
                ) {
                    return Err(error);
                }
                state.state = if failed_dependency {
                    StructuredGroupState::Skipped
                } else if error.code == ErrorCode::Conflict {
                    StructuredGroupState::Conflict
                } else {
                    StructuredGroupState::Rejected
                };
                state.error = Some(error);
                if let Some(mut tx) = self
                    .worker_transaction(
                        "cognition.group.reject",
                        &format!("{}:{}", run.job_id, group.key),
                        &run.job_id,
                        &json!({"group":group.key}),
                    )
                    .await?
                {
                    if tx.structured_formation(&run.job_id).await?.is_retired() {
                        tx.rollback().await?;
                        return Ok(());
                    }
                    state.commit_seq = Some(tx.commit_seq());
                    tx.update_structured_group(&run.job_id, &state).await?;
                    tx.commit(json!(state)).await?;
                }
            }
        }
        if let Some(mut tx) = self
            .worker_transaction(
                "cognition.formation.finish",
                &run.job_id,
                &run.job_id,
                &json!({"job_id":run.job_id}),
            )
            .await?
        {
            if tx.structured_formation(&run.job_id).await?.is_retired() {
                tx.rollback().await?;
                return Ok(());
            }
            let mut current = tx.structured_formation(&run.job_id).await?;
            current.state = FormationState::Succeeded;
            current.error = None;
            current.updated_at = Utc::now().to_rfc3339();
            tx.update_structured_formation(&current).await?;
            tx.commit(json!({"state":current.state})).await?;
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    async fn apply_structured_group(
        &self,
        run: &StructuredFormationRun,
        group: &StructuredProposalGroup,
        slots: &HashMap<String, &StructuredTargetSlot>,
        owners: &HashMap<String, String>,
        allowed: &BTreeSet<String>,
        call_id: &ModelCallId,
        formed: i64,
    ) -> Result<()> {
        group.validate()?;
        let outputs = slots
            .iter()
            .map(|(key, slot)| Ok((slot.output_reference()?, key)))
            .collect::<Result<BTreeMap<_, _>>>()?;
        for change in &group.changes {
            for source in change.sources() {
                if let Some(reference) = source.cognitive_ref()
                    && let Some(key) = outputs.get(&reference)
                {
                    let owner = owners.get(*key).ok_or_else(|| {
                        Error::invalid("proposal refers to an unused target slot")
                    })?;
                    if !allowed.contains(owner) {
                        return Err(Error::invalid(
                            "cross-group target reference needs a declared dependency",
                        ));
                    }
                }
            }
        }
        let Some(mut tx) = self
            .worker_transaction(
                "cognition.group.apply",
                &format!("{}:{}", run.job_id, group.key),
                &run.job_id,
                &json!({"group":group.key}),
            )
            .await?
        else {
            return Ok(());
        };
        if tx.structured_formation(&run.job_id).await?.is_retired() {
            tx.rollback().await?;
            return Ok(());
        }
        let mut pending = group.changes.iter().collect::<Vec<_>>();
        let pending_targets = group
            .changes
            .iter()
            .map(|c| slots[&c.slot_key].output_reference())
            .collect::<Result<BTreeSet<_>>>()?;
        let mut saved = BTreeSet::new();
        let mut objects = Vec::new();
        while !pending.is_empty() {
            let position = pending
                .iter()
                .position(|change| {
                    change
                        .sources()
                        .iter()
                        .filter_map(EvidenceSource::cognitive_ref)
                        .all(|r| !pending_targets.contains(&r) || saved.contains(&r))
                })
                .ok_or_else(|| {
                    Error::invalid("proposal group's object references contain a cycle")
                })?;
            let change = pending.remove(position);
            let slot = slots[&change.slot_key];
            if slot.expected_revision > 0
                && tx
                    .structured_object(&run.namespace_id, &slot.id)
                    .await?
                    .deleted
            {
                return Err(Error::new(
                    ErrorCode::Conflict,
                    "formation target was deleted",
                ));
            }
            let object = StructuredObject {
                id: slot.id.clone(),
                namespace_id: run.namespace_id.clone(),
                subject_id: run.subject_id.clone(),
                revision: slot.expected_revision + 1,
                body: change.body.clone(),
                evidence: change.evidence.clone(),
                times: CognitiveTimes {
                    event: None,
                    observation: None,
                    formation: TimePoint {
                        domain: "unix_ms".into(),
                        coordinate: formed,
                    },
                    validity: None,
                    recorded: cognition::utc_point(),
                },
                producer_call_id: Some(call_id.clone()),
                recorded_at: Utc::now().to_rfc3339(),
                commit_seq: tx.commit_seq(),
                deleted: false,
            };
            tx.save_structured(
                &object,
                (slot.expected_revision > 0).then_some(slot.expected_revision),
            )
            .await?;
            saved.insert(object.reference());
            objects.push(object);
        }
        let mut state = tx
            .structured_formation_groups(&run.job_id)
            .await?
            .into_iter()
            .find(|s| s.key == group.key)
            .ok_or_else(|| Error::internal("proposal group missing"))?;
        state.state = StructuredGroupState::Applied;
        state.commit_seq = Some(tx.commit_seq());
        state.result = Some(json!({"objects":objects}));
        tx.update_structured_group(&run.job_id, &state).await?;
        tx.commit(json!(state)).await?;
        Ok(())
    }
}
