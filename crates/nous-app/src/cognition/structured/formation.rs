use super::*;
use crate::common::models::structured::{
    RawStructuredProposal, SourceCatalogEntry, StructuredFormationInput,
};
use nous_storage::{StructuredFormationGroup, StructuredFormationRun, StructuredGroupState};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use tokio_util::sync::CancellationToken;
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
        let job = self
            .tasks
            .new_job("cognition.form", Some(namespace), Some(request.subject_id));
        let run = StructuredFormationRun { job, input };
        self.tasks.submit_in(&mut tx, &run.job).await?;
        tx.insert_structured_formation(&run).await?;
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
        let (namespace, subject) = run.job.subject_scope()?;
        ctx.require_subject(namespace, subject)?;
        let groups = self.database.structured_formation_groups(job_id).await?;
        Ok(json!({"run":run,"groups":groups}))
    }

    pub(crate) async fn run_structured_formation(
        &self,
        job: JobRecord,
        cancel: CancellationToken,
    ) -> Result<()> {
        let run = self.database.structured_formation(&job.id).await?;
        if run.is_retired() {
            return Ok(());
        }
        let input: StructuredFormationInput = serde_json::from_value(run.input.clone())?;
        let Some(call) = self
            .calls
            .invoke(
                &job.id,
                "proposal",
                &cancel,
                || async { self.model.prepare_structured(input) },
                |prepared| self.model.execute_structured(prepared),
            )
            .await?
        else {
            return Ok(());
        };
        let proposal: RawStructuredProposal = serde_json::from_value(
            call.response
                .as_ref()
                .ok_or_else(|| Error::internal("successful proposal call has no result"))?["result"]
                .clone(),
        )?;
        if let Some(mut tx) = self
            .worker_transaction(
                "cognition.proposal",
                &job.id,
                &job.id,
                &json!({"call_id":call.id}),
            )
            .await?
        {
            if tx.structured_formation(&job.id).await?.is_retired() {
                tx.rollback().await?;
                return Ok(());
            }
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
            tx.insert_structured_groups(&job.id, &call.id, &groups)
                .await?;
            tx.commit(json!({"call_id":call.id,"groups":groups.len()}))
                .await?;
        }
        self.adopt_structured_groups(&run, &call, &proposal).await
    }
}
