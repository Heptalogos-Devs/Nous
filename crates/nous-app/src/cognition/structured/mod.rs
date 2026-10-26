//! Typed cognitive commands use the common authority and source contracts.
use crate::App;
use nous_domain::*;
use nous_storage::operation_key;
use nous_storage::{BeginOperation, OperationReceipt};
use serde_json::{Value, json};
mod adoption;
mod formation;
mod lineage;
pub mod model;
pub use formation::{FormStructuredRequest, FormationTarget};
impl App {
    pub async fn create_structured(
        &self,
        ctx: &CallContext,
        operation_id: &str,
        input: CreateStructuredRequest,
    ) -> Result<OperationReceipt> {
        ctx.require_subject(&input.namespace_id, &input.subject_id)?;
        if input.expected_revision != 0 {
            return Err(Error::invalid("creation requires expected_revision=0"));
        }
        if input.producer_call_id.is_some() {
            return Err(Error::invalid(
                "model provenance is assigned by the formation worker",
            ));
        }
        let scope = format!(
            "namespace:{}/subject:{}",
            input.namespace_id, input.subject_id
        );
        let key = operation_key(
            ctx,
            "cognition.create",
            &scope,
            operation_id,
            &serde_json::to_value(&input)?,
        )?;
        let mut tx = match self.database.begin_operation(key).await? {
            BeginOperation::Replay(receipt) => return Ok(receipt),
            BeginOperation::New(tx) => tx,
        };
        let subject = tx.subject(&input.namespace_id, &input.subject_id).await?;
        if !subject.capabilities.contains(&Capability::Memory) {
            return Err(Error::new(
                ErrorCode::CapabilityUnavailable,
                "subject has no memory capability",
            ));
        }
        let mut times = input.times;
        times.recorded = tx.time().point();
        let object = StructuredObject {
            id: CognitiveId::new(),
            namespace_id: input.namespace_id,
            subject_id: input.subject_id,
            revision: 1,
            body: input.body,
            evidence: input.evidence,
            times,
            producer_call_id: None,
            recorded_at: tx.time().recorded(),
            commit_seq: tx.commit_seq(),
            deleted: false,
        };
        tx.save_structured(&object, None).await?;
        tx.commit(serde_json::to_value(&object)?).await
    }

    pub async fn revise_structured(
        &self,
        ctx: &CallContext,
        operation_id: &str,
        input: ReviseStructuredRequest,
    ) -> Result<OperationReceipt> {
        let namespace = self.database.structured_namespace(&input.id).await?;
        let previous = self
            .database
            .structured_object(&namespace, &input.id)
            .await?;
        ctx.require_subject(&namespace, &previous.subject_id)?;
        if input.producer_call_id.is_some() {
            return Err(Error::invalid(
                "model provenance is assigned by the formation worker",
            ));
        }
        let scope = format!("namespace:{namespace}/subject:{}", previous.subject_id);
        let key = operation_key(
            ctx,
            "cognition.revise",
            &scope,
            operation_id,
            &serde_json::to_value(&input)?,
        )?;
        let mut tx = match self.database.begin_operation(key).await? {
            BeginOperation::Replay(receipt) => return Ok(receipt),
            BeginOperation::New(tx) => tx,
        };
        let mut current = tx.structured_object(&namespace, &input.id).await?;
        if current.deleted {
            return Err(Error::new(
                ErrorCode::Conflict,
                "restore the cognitive object before revising it",
            ));
        }
        current.revision = input
            .expected_revision
            .checked_add(1)
            .ok_or_else(|| Error::invalid("revision overflow"))?;
        current.body = input.body;
        current.evidence = input.evidence;
        current.times = input.times;
        current.times.recorded = tx.time().point();
        current.recorded_at = tx.time().recorded();
        current.commit_seq = tx.commit_seq();
        current.producer_call_id = None;
        tx.save_structured(&current, Some(input.expected_revision))
            .await?;
        tx.commit(serde_json::to_value(current)?).await
    }

    pub async fn get_structured(
        &self,
        ctx: &CallContext,
        id: &CognitiveId,
        revision: Option<i64>,
    ) -> Result<StructuredObject> {
        let namespace = self.database.structured_namespace(id).await?;
        let object = if let Some(revision) = revision {
            self.database
                .structured_revision(&namespace, id, revision)
                .await?
        } else {
            self.database.structured_object(&namespace, id).await?
        };
        ctx.require_subject(&namespace, &object.subject_id)?;
        if revision.is_none() && object.deleted {
            return Err(Error::new(
                ErrorCode::NotFound,
                "cognitive object is deleted",
            ));
        }
        self.database
            .require_source_read(
                ctx,
                &namespace,
                &object.subject_id,
                &RetrievalTarget::Cognition(object.reference()),
            )
            .await?;
        Ok(object)
    }

    pub async fn structured_history(
        &self,
        ctx: &CallContext,
        id: &CognitiveId,
    ) -> Result<Vec<StructuredObject>> {
        let namespace = self.database.structured_namespace(id).await?;
        let object = self.database.structured_object(&namespace, id).await?;
        ctx.require_subject(&namespace, &object.subject_id)?;
        let history = self.database.structured_history(&namespace, id).await?;
        for object in &history {
            self.database
                .require_source_read(
                    ctx,
                    &namespace,
                    &object.subject_id,
                    &RetrievalTarget::Cognition(object.reference()),
                )
                .await?;
        }
        Ok(history)
    }

    pub async fn set_structured_deleted(
        &self,
        ctx: &CallContext,
        operation_id: &str,
        id: &CognitiveId,
        expected_revision: i64,
        deleted: bool,
    ) -> Result<OperationReceipt> {
        let namespace = self.database.structured_namespace(id).await?;
        let object = self.database.structured_object(&namespace, id).await?;
        ctx.require_subject(&namespace, &object.subject_id)?;
        let scope = format!("namespace:{namespace}/subject:{}", object.subject_id);
        let key = operation_key(
            ctx,
            "cognition.lifecycle",
            &scope,
            operation_id,
            &json!({"id":id,"expected_revision":expected_revision,"deleted":deleted}),
        )?;
        let mut tx = match self.database.begin_operation(key).await? {
            BeginOperation::Replay(receipt) => return Ok(receipt),
            BeginOperation::New(tx) => tx,
        };
        let mut object = tx.structured_object(&namespace, id).await?;
        if object.revision != expected_revision {
            return Err(Error::new(
                ErrorCode::Conflict,
                "cognitive revision changed",
            ));
        }
        if deleted {
            let targets = tx
                .cognition_source_dependents(&namespace, &object.subject_id, &object.reference())
                .await?;
            tx.invalidate_context_history(&namespace, &object.subject_id, &targets)
                .await?;
        }
        object.revision = expected_revision
            .checked_add(1)
            .ok_or_else(|| Error::invalid("revision overflow"))?;
        object.deleted = deleted;
        object.commit_seq = tx.commit_seq();
        object.recorded_at = tx.time().recorded();
        object.times.recorded = tx.time().point();
        object.producer_call_id = None;
        tx.save_structured(&object, Some(expected_revision)).await?;
        tx.commit(serde_json::to_value(object)?).await
    }

    pub async fn structured_source(
        &self,
        ctx: &CallContext,
        id: &CognitiveId,
        revision: Option<i64>,
    ) -> Result<Value> {
        let object = self.get_structured(ctx, id, revision).await?;
        let closure = self
            .database
            .source_closure(&object.namespace_id, id, Some(object.revision))
            .await?;
        let mut materials = Vec::new();
        let source_context = ctx.narrow(AccessScope::Subject {
            namespace_id: object.namespace_id.clone(),
            subject_id: object.subject_id.clone(),
        })?;
        for root in &closure.roots {
            match self
                .sources
                .original_metadata(
                    &source_context,
                    &root.material_id,
                    &tokio_util::sync::CancellationToken::new(),
                )
                .await
            {
                Ok(source) => materials.push(nous_domain::media::MediaOriginal {
                    material: source.material,
                    details: source.details,
                    details_redacted: false,
                }),
                Err(error) if error.code == ErrorCode::NotFound => {}
                Err(error) => return Err(error),
            }
        }
        let mut media_sources = Vec::new();
        let mut expanded_media = std::collections::BTreeSet::new();
        for node in &closure.nodes {
            if let EvidenceSource::MemoryRevision {
                memory_id,
                revision,
            } = &node.source
            {
                let memory = match self
                    .database
                    .memory_revision(&object.namespace_id, memory_id, *revision)
                    .await
                {
                    Ok(memory) => memory,
                    Err(error) if error.code == ErrorCode::NotFound => continue,
                    Err(error) => return Err(error),
                };
                match self
                    .sources
                    .lineage_for_event(
                        &source_context,
                        &object.namespace_id,
                        &object.subject_id,
                        &memory.source,
                        &memory.observation_id,
                    )
                    .await
                {
                    Ok(lineage) => {
                        expanded_media.insert(memory.source.clone());
                        media_sources.push(
                        json!({"memory_id":memory.id,"revision":memory.revision,"source":lineage}),
                    );
                    }
                    Err(error) if error.code == ErrorCode::NotFound => {}
                    Err(error) => return Err(error),
                }
            }
        }
        for node in &closure.nodes {
            if let EvidenceSource::Media { reference } = &node.source
                && expanded_media.insert(reference.clone())
            {
                match self
                    .sources
                    .lineage(
                        &source_context,
                        &object.namespace_id,
                        &object.subject_id,
                        reference,
                    )
                    .await
                {
                    Ok(lineage) => {
                        media_sources.push(json!({"reference":reference,"source":lineage}))
                    }
                    Err(error) if error.code == ErrorCode::NotFound => {}
                    Err(error) => return Err(error),
                }
            }
        }
        let producer = match &object.producer_call_id {
            Some(id) => match self.read_model_call(ctx, id).await {
                Ok(call) => Some(call),
                Err(error) if matches!(error.code, ErrorCode::Forbidden | ErrorCode::NotFound) => {
                    None
                }
                Err(error) => return Err(error),
            },
            None => None,
        };
        Ok(
            json!({"object":object,"closure":closure,"materials":materials,"media_sources":media_sources,"producer":producer}),
        )
    }

    pub async fn concept_by_alias(
        &self,
        ctx: &CallContext,
        subject: &SubjectId,
        alias: &str,
    ) -> Result<StructuredObject> {
        let ns = self.database.subject_namespace(subject).await?;
        ctx.require_subject(&ns, subject)?;
        let object = self.database.concept_by_alias(&ns, subject, alias).await?;
        self.database
            .require_source_read(
                ctx,
                &ns,
                subject,
                &RetrievalTarget::Cognition(object.reference()),
            )
            .await?;
        Ok(object)
    }
}
