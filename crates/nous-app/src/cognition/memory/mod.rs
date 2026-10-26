mod decoder;
pub mod model;
use crate::App;
use nous_domain::media::MediaReference;
use nous_domain::*;
use nous_storage::operation_key;
use nous_storage::{BeginOperation, OperationReceipt};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

impl App {
    pub async fn form_memory(
        &self,
        ctx: &CallContext,
        operation_id: &str,
        subject_id: &SubjectId,
        observation_id: &ObservationId,
        source: Option<MediaReference>,
    ) -> Result<OperationReceipt> {
        let namespace = self.database.subject_namespace(subject_id).await?;
        ctx.require_subject(&namespace, subject_id)?;
        let key = operation_key(
            ctx,
            "memory.form",
            &format!("namespace:{namespace}/subject:{subject_id}"),
            operation_id,
            &json!({"subject_id":subject_id,"observation_id":observation_id,"source":source}),
        )?;
        let mut tx = match self.database.begin_operation(key).await? {
            BeginOperation::Replay(receipt) => return Ok(receipt),
            BeginOperation::New(tx) => tx,
        };
        let subject = tx.subject(&namespace, subject_id).await?;
        if !subject.capabilities.contains(&Capability::Memory) {
            return Err(Error::new(
                ErrorCode::CapabilityUnavailable,
                "subject has no memory capability",
            ));
        }
        let observation = tx.observation(&namespace, observation_id).await?;
        if observation.subject_id != *subject_id {
            return Err(Error::new(
                ErrorCode::Forbidden,
                "observation belongs to another subject",
            ));
        }
        let material = tx.material(&namespace, &observation.material_id).await?;
        let source = source.unwrap_or_else(|| MediaReference::Original {
            material_id: material.id.clone(),
        });
        let job = self.tasks.new_job(
            &tx,
            "memory.form",
            Some(namespace),
            Some(subject_id.clone()),
        );
        let run = FormationRun {
            job,
            observation_id: observation.id,
            material_id: material.id,
            source,
            subject_revision: subject.revision,
            material_hash: material.content_hash,
            memory_ids: vec![],
        };
        self.tasks.submit_in(&mut tx, &run.job).await?;
        tx.insert_formation(&run).await?;
        tx.commit(json!(run)).await
    }
    pub async fn get_memory(&self, ctx: &CallContext, id: &MemoryId) -> Result<Memory> {
        let namespace = self.database.memory_namespace(id).await?;
        let memory = self.database.memory(&namespace, id).await?;
        ctx.require_subject(&namespace, &memory.subject_id)?;
        if memory.deleted {
            return Err(Error::new(ErrorCode::NotFound, "memory is deleted"));
        }
        Ok(memory)
    }
    pub async fn memory_history(&self, ctx: &CallContext, id: &MemoryId) -> Result<Vec<Memory>> {
        let namespace = self.database.memory_namespace(id).await?;
        let memory = self.database.memory(&namespace, id).await?;
        ctx.require_subject(&namespace, &memory.subject_id)?;
        self.database.memory_history(&namespace, id).await
    }
    pub async fn delete_memory(
        &self,
        ctx: &CallContext,
        operation_id: &str,
        id: &MemoryId,
        expected_revision: i64,
    ) -> Result<OperationReceipt> {
        self.apply_memory_change(
            ctx,
            operation_id,
            id,
            MemoryChange {
                action: MemoryAction::Trash,
                expected_revision,
                strength: None,
                hours: None,
                reason: String::new(),
                force: false,
                edit: None,
            },
            "memory.delete",
            json!({"memory_id":id,"expected_revision":expected_revision}),
        )
        .await
    }
    pub async fn memory_source(&self, ctx: &CallContext, id: &MemoryId) -> Result<MemorySource> {
        let memory = self.get_memory(ctx, id).await?;
        let observation = self
            .database
            .observation(&memory.namespace_id, &memory.observation_id)
            .await?;
        let source = self
            .sources
            .lineage(
                ctx,
                &memory.namespace_id,
                &memory.subject_id,
                &memory.source,
            )
            .await?;
        let model_call = match &memory.model_call_id {
            Some(id) => Some(self.calls.read(ctx, id).await?),
            None => None,
        };
        let embedding_provenance = self
            .database
            .embeddings_for_target(
                &memory.namespace_id,
                &memory.subject_id,
                &RetrievalTarget::Cognition(CognitiveRef::memory(id, memory.revision)),
            )
            .await?
            .iter()
            .map(crate::retrieval::provenance)
            .collect();
        Ok(MemorySource {
            memory,
            observation,
            source,
            model_call,
            embedding_provenance,
        })
    }
    pub async fn model_status(&self, ctx: &CallContext) -> Result<Value> {
        ctx.require_admin()?;
        self.model.inspect(&ConfigScope::Instance).await
    }

    pub(crate) async fn run_memory_formation(
        &self,
        job: JobRecord,
        cancel: CancellationToken,
    ) -> Result<()> {
        let run = self.database.formation(&job.id).await?;
        let (namespace, subject) = job.subject_scope()?;
        let current = self.database.subject(namespace, subject).await?;
        let observation = self
            .database
            .observation(namespace, &run.observation_id)
            .await?;
        let material = self.database.material(namespace, &run.material_id).await?;
        let source = self
            .sources
            .resolve(
                &job.call_context("nous-worker")?,
                namespace,
                subject,
                &run.source,
            )
            .await?;
        if current.revision != run.subject_revision
            || material.content_hash != run.material_hash
            || observation.subject_id != *subject
            || observation.material_id != run.material_id
            || !source.roots.contains(&run.material_id)
        {
            return Err(Error::new(ErrorCode::Conflict, "formation input changed"));
        }
        let input = FormationInput {
            namespace_id: namespace.clone(),
            subject_id: subject.clone(),
            source: run.source.clone(),
            source_access: source.access,
            material_id: run.material_id.clone(),
            material_commit_seq: material.commit_seq,
            observation_id: run.observation_id.clone(),
            observation_commit_seq: observation.commit_seq,
            source_event_kind: observation.kind,
            text: self.sources.text(&source, &cancel).await?,
        };
        let plan = self.model.plan(
            ConfigScope::Subject {
                namespace_id: namespace.clone(),
                subject_id: subject.clone(),
            },
            model::ROLE,
            cancel.clone(),
        );
        let Some(call) = self
            .calls
            .invoke(
                &job.id,
                "formation",
                &cancel,
                |attempt| model::prepare(&plan, attempt, input.clone()),
                |prepared| model::execute(&self.model, prepared),
            )
            .await?
        else {
            return Ok(());
        };
        let Some(mut tx) = self
            .worker_transaction(
                &job,
                "memory.adopt",
                &job.id,
                &job.id,
                &json!({"call_id":call.id}),
            )
            .await?
        else {
            return Ok(());
        };
        let mut run = tx.formation(&job.id).await?;
        if matches!(run.job.state, JobState::Succeeded | JobState::Cancelled) {
            tx.rollback().await?;
            return Ok(());
        }
        let material = tx.material(namespace, &run.material_id).await?;
        let observation = tx.observation(namespace, &run.observation_id).await?;
        if tx.subject(namespace, subject).await?.revision != run.subject_revision
            || material.content_hash != run.material_hash
        {
            return Err(Error::new(
                ErrorCode::Conflict,
                "formation input changed before adoption",
            ));
        }
        let saved = tx.model_call(&call.id).await?;
        if saved.state != ModelCallState::Succeeded {
            return Err(Error::new(
                ErrorCode::Conflict,
                "model result is unavailable",
            ));
        }
        let drafts: Vec<MemoryDraft> = serde_json::from_value(
            saved
                .response
                .as_ref()
                .ok_or_else(|| Error::internal("successful call has no output"))?["result"]
                .clone(),
        )?;
        let formed = chrono::DateTime::parse_from_rfc3339(
            saved
                .completed_at
                .as_deref()
                .ok_or_else(|| Error::internal("call completion timestamp is missing"))?,
        )
        .map_err(|e| Error::internal(e.to_string()))?
        .timestamp_millis();
        run.memory_ids.clear();
        for draft in drafts {
            let memory = Memory {
                id: MemoryId::new(),
                namespace_id: namespace.clone(),
                subject_id: subject.clone(),
                revision: 1,
                title: draft.title,
                content: draft.content,
                epistemic_status: draft.epistemic_status,
                observation_id: observation.id.clone(),
                material_id: material.id.clone(),
                source: run.source.clone(),
                model_call_id: Some(saved.id.clone()),
                origin: MemoryOrigin::Model,
                times: CognitiveTimes {
                    event: material.event_time.clone(),
                    observation: observation.observed_at.clone(),
                    formation: TimePoint {
                        domain: "unix_ms".into(),
                        coordinate: formed,
                    },
                    validity: None,
                    recorded: tx.time().point(),
                },
                recorded_at: tx.time().recorded(),
                deleted: false,
                lifecycle: MemoryLifecycle::new(formed),
                commit_seq: tx.commit_seq(),
            };
            tx.save_memory(&memory, None).await?;
            run.memory_ids.push(memory.id);
        }
        tx.update_formation(&run).await?;
        let result = json!({"memory_ids":run.memory_ids});
        self.tasks
            .complete_in(&mut tx, &mut run.job, result.clone())
            .await?;
        tx.commit(result).await?;
        Ok(())
    }
}
