//! Prompt edits and role activation share the caller's Authority operation.
use crate::App;
use nous_domain::{CallContext, ConfigScope, Error, ErrorCode, Result};
use nous_foundation::{
    configuration::{ConfigChangeSet, ConfigEdit},
    prompts::{PromptArguments, PromptBinding, PromptEdit},
};
use nous_storage::{BeginOperation, OperationReceipt, WriteTransaction, operation_key};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EditedPromptActivation {
    pub role: String,
    pub expected_revision: i64,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PromptEditRequest {
    pub operation_id: String,
    pub scope: Option<ConfigScope>,
    pub edit: PromptEdit,
    pub activate: Option<EditedPromptActivation>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PromptActivateRequest {
    pub operation_id: String,
    pub scope: Option<ConfigScope>,
    pub role: String,
    pub expected_revision: i64,
    pub selection: PromptBinding,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
#[schemars(extend("type"="object"))]
pub enum PromptRequest {
    Catalog {
        scope: Option<ConfigScope>,
        #[serde(default = "limit")]
        limit: usize,
    },
    Read {
        scope: Option<ConfigScope>,
        id: Option<String>,
        selection: PromptBinding,
    },
    History {
        scope: Option<ConfigScope>,
        resource_scope: Option<ConfigScope>,
        id: String,
        #[serde(default = "limit")]
        limit: usize,
    },
    Edit(PromptEditRequest),
    Activate(PromptActivateRequest),
    Preview {
        scope: Option<ConfigScope>,
        id: Option<String>,
        selection: PromptBinding,
        #[serde(default)]
        arguments: PromptArguments,
        prompt: Option<String>,
    },
}
fn limit() -> usize {
    50
}
fn scope(ctx: &CallContext, scope: Option<ConfigScope>) -> Result<ConfigScope> {
    let scope = scope.unwrap_or_else(|| ConfigScope::for_caller(ctx));
    scope.require_access(ctx)?;
    Ok(scope)
}
impl App {
    pub async fn prompt_request(&self, ctx: &CallContext, request: PromptRequest) -> Result<Value> {
        let prompts = self.model.prompts();
        match request {
            PromptRequest::Catalog {
                scope: requested,
                limit,
            } => Ok(json!(
                prompts.catalog(&scope(ctx, requested)?, limit).await?
            )),
            PromptRequest::Read {
                scope: requested,
                id,
                selection,
            } => Ok(json!(
                prompts
                    .read(
                        &scope(ctx, requested)?,
                        id.as_deref().unwrap_or(""),
                        &selection
                    )
                    .await?
            )),
            PromptRequest::History {
                scope: requested,
                resource_scope,
                id,
                limit,
            } => {
                let viewing = scope(ctx, requested)?;
                let source = resource_scope.unwrap_or_else(|| viewing.clone());
                Ok(json!(prompts.history(&viewing, &source, &id, limit).await?))
            }
            PromptRequest::Preview {
                scope: requested,
                id,
                selection,
                arguments,
                prompt,
            } => Ok(json!(
                prompts
                    .preview(
                        &scope(ctx, requested)?,
                        id.as_deref().unwrap_or(""),
                        &selection,
                        &arguments,
                        prompt.as_deref()
                    )
                    .await?
            )),
            PromptRequest::Edit(request) => Ok(json!(self.edit_prompt(ctx, request).await?)),
            PromptRequest::Activate(request) => {
                Ok(json!(self.activate_prompt(ctx, request).await?))
            }
        }
    }
    pub async fn edit_prompt(
        &self,
        ctx: &CallContext,
        request: PromptEditRequest,
    ) -> Result<OperationReceipt> {
        let viewing = scope(ctx, request.scope.clone())?;
        let key = operation_key(
            ctx,
            "prompt.edit",
            &serde_json::to_string(&viewing)?,
            &request.operation_id,
            &json!(request),
        )?;
        let mut tx = match self.database.begin_operation(key).await? {
            BeginOperation::Replay(receipt) => return Ok(receipt),
            BeginOperation::New(tx) => tx,
        };
        let revision = self
            .model
            .prompts()
            .edit_in(&mut tx, ctx, &viewing, request.edit)
            .await?;
        let change = if let Some(activate) = request.activate {
            Some(
                self.bind_prompt_in(
                    &mut tx,
                    &viewing,
                    &activate.role,
                    activate.expected_revision,
                    PromptBinding::Managed {
                        reference: revision.reference.clone(),
                    },
                )
                .await?,
            )
        } else {
            None
        };
        let receipt = tx
            .commit(json!({"revision":revision,"activation":change}))
            .await?;
        if let Some(change) = change {
            self.configuration.committed(&change);
            self.tasks.notify_wake();
        }
        Ok(receipt)
    }
    pub async fn activate_prompt(
        &self,
        ctx: &CallContext,
        request: PromptActivateRequest,
    ) -> Result<OperationReceipt> {
        let viewing = scope(ctx, request.scope.clone())?;
        let key = operation_key(
            ctx,
            "prompt.activate",
            &serde_json::to_string(&viewing)?,
            &request.operation_id,
            &json!(request),
        )?;
        let mut tx = match self.database.begin_operation(key).await? {
            BeginOperation::Replay(receipt) => return Ok(receipt),
            BeginOperation::New(tx) => tx,
        };
        let change = self
            .bind_prompt_in(
                &mut tx,
                &viewing,
                &request.role,
                request.expected_revision,
                request.selection,
            )
            .await?;
        let receipt = tx.commit(json!(change)).await?;
        self.configuration.committed(&change);
        self.tasks.notify_wake();
        Ok(receipt)
    }
    async fn bind_prompt_in(
        &self,
        tx: &mut WriteTransaction,
        scope: &ConfigScope,
        role: &str,
        expected_revision: i64,
        selection: PromptBinding,
    ) -> Result<ConfigChangeSet> {
        let definition = self.model.resources().roles().get(role)?;
        let prompt = definition.prompt.as_ref().ok_or_else(|| {
            Error::new(
                ErrorCode::CapabilityUnavailable,
                "this role does not consume an instruction template",
            )
        })?;
        self.model
            .prompts()
            .read_in(tx, scope, prompt.id, &selection)
            .await?;
        let snapshot = self
            .configuration
            .snapshot_in_transaction(tx, scope, false)
            .await?;
        let mut binding = snapshot.get(definition.binding_key)?.ok_or_else(|| {
            Error::new(
                ErrorCode::CapabilityUnavailable,
                "configure a role binding before activating an instruction",
            )
        })?;
        binding.prompt = selection;
        self.configuration
            .change_in(
                tx,
                scope,
                &[ConfigEdit::Set {
                    key: definition.binding_key.path().into(),
                    expected_revision,
                    value: json!(binding),
                }],
            )
            .await
    }
}
