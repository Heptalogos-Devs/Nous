//! Shared model-call reservation and recovery. Role owners adopt their own results.
use super::*;
use model::{ModelFailure, RetryClass};
use nous_storage::WriteTransaction;

pub(crate) struct CallRequest<'a> {
    pub job_id: &'a str,
    pub namespace_id: &'a NamespaceId,
    pub subject_id: &'a SubjectId,
    pub call_id: ModelCallId,
    pub resource_id: &'a str,
    pub max_calls_per_day: u32,
    pub request: Value,
}

pub(crate) struct CallReservation {
    pub transaction: WriteTransaction,
    /// None means the resource's daily budget is exhausted.
    pub call: Option<ModelCall>,
}

impl App {
    pub(crate) async fn reserve_call(
        &self,
        input: CallRequest<'_>,
    ) -> Result<Option<CallReservation>> {
        let Some(mut transaction) = self
            .worker_transaction(
                "model.reserve",
                input.call_id.as_str(),
                input.job_id,
                &input.request,
            )
            .await?
        else {
            return Ok(None);
        };
        let today = Utc::now().format("%Y-%m-%d").to_string();
        let call = if transaction
            .daily_model_calls(input.resource_id, &today)
            .await?
            >= i64::from(input.max_calls_per_day)
        {
            None
        } else {
            let call = ModelCall {
                id: input.call_id,
                job_id: input.job_id.into(),
                namespace_id: input.namespace_id.clone(),
                subject_id: input.subject_id.clone(),
                attempt: self.database.model_calls(input.job_id).await?.len() as i64 + 1,
                resource_id: input.resource_id.into(),
                request: input.request,
                response: None,
                state: ModelCallState::Started,
                error: None,
                cost_microunits: None,
                created_at: Utc::now().to_rfc3339(),
                completed_at: None,
            };
            transaction.insert_model_call(&call).await?;
            Some(call)
        };
        Ok(Some(CallReservation { transaction, call }))
    }

    pub(crate) async fn mark_interrupted_call(&self, mut call: ModelCall) -> Result<()> {
        if let Some(mut tx) = self
            .worker_transaction(
                "model.interrupted",
                call.id.as_str(),
                &call.job_id,
                &json!({"call_id":call.id}),
            )
            .await?
        {
            call.state = ModelCallState::Uncertain;
            call.error = Some(Error::new(
                ErrorCode::DependencyUnavailable,
                "host stopped before the remote response was saved; the previous request may have completed",
            ));
            call.completed_at = Some(Utc::now().to_rfc3339());
            tx.update_model_call(&call).await?;
            tx.commit(json!({"state":call.state})).await?;
        }
        Ok(())
    }

    pub(crate) async fn worker_transaction(
        &self,
        kind: &str,
        operation_id: &str,
        scope: &str,
        payload: &Value,
    ) -> Result<Option<WriteTransaction>> {
        let context = CallContext {
            caller_id: "nous-worker:v1".into(),
            scope: AccessScope::Admin,
        };
        let key = operation_key(&context, kind, scope, operation_id, payload)?;
        match self.database.begin_operation(key).await? {
            BeginOperation::Replay(_) => Ok(None),
            BeginOperation::New(tx) => Ok(Some(tx)),
        }
    }

    pub async fn read_model_call(&self, ctx: &CallContext, id: &ModelCallId) -> Result<ModelCall> {
        let call = self.database.model_call(id).await?;
        ctx.require_subject(&call.namespace_id, &call.subject_id)?;
        Ok(call)
    }
}

pub(crate) fn settle_call(
    call: &mut ModelCall,
    completed_at: String,
    response: Value,
    failure: Option<&ModelFailure>,
) {
    call.completed_at = Some(completed_at);
    call.response = Some(response);
    call.state = match failure {
        Some(failure) if failure.class() == RetryClass::Uncertain => ModelCallState::Uncertain,
        Some(_) => ModelCallState::Failed,
        None => ModelCallState::Succeeded,
    };
    call.error = failure.map(ModelFailure::to_domain_error);
}

pub(crate) fn budget_error() -> Error {
    Error::new(
        ErrorCode::ResourceExhausted,
        "daily model-call budget exhausted; waiting for the next UTC day",
    )
}

/// Preserve actual remote accounting after the role owner retires its source.
/// The current redacted request remains immutable and output bodies stay retired.
pub(crate) fn complete_redacted_call(current: &mut ModelCall, completed: &ModelCall) {
    let record = &completed.response.as_ref().unwrap_or(&Value::Null)["record"];
    let metadata: serde_json::Map<String, Value> = [
        "provider_model",
        "usage",
        "cost",
        "started_at",
        "completed_at",
        "status",
        "failure",
    ]
    .into_iter()
    .filter_map(|key| record.get(key).map(|value| (key.to_owned(), value.clone())))
    .collect();
    current.state = completed.state.clone();
    current.error = completed.error.clone();
    current.completed_at = completed.completed_at.clone();
    current.cost_microunits = completed.cost_microunits;
    current.response = Some(
        json!({"schema_version":1,"redacted":true,"reason":"source_removed","record":metadata}),
    );
}
