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
        self.database.structured_history(&namespace, id).await
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
        for root in &closure.roots {
            match self.get_material(ctx, &root.material_id).await {
                Ok(material) => materials.push(material),
                Err(error) if error.code == ErrorCode::NotFound => {}
                Err(error) => return Err(error),
            }
        }
        let producer = match &object.producer_call_id {
            Some(id) => Some(self.read_model_call(ctx, id).await?),
            None => None,
        };
        Ok(json!({"object":object,"closure":closure,"materials":materials,"producer":producer}))
    }

    pub async fn concept_by_alias(
        &self,
        ctx: &CallContext,
        subject: &SubjectId,
        alias: &str,
    ) -> Result<StructuredObject> {
        let ns = self.database.subject_namespace(subject).await?;
        ctx.require_subject(&ns, subject)?;
        self.database.concept_by_alias(&ns, subject, alias).await
    }
}
