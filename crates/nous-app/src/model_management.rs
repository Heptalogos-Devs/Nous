//! Authorized read-only access to the model owner's actual invocation contracts and builder.
use crate::{App, common::models::InvocationEdits};
use nous_domain::{CallContext, ConfigScope, Result, validate_name};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ModelContractRequest {
    pub role: String,
    pub scope: Option<ConfigScope>,
    pub input: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ModelPreviewRequest {
    pub role: String,
    pub scope: Option<ConfigScope>,
    pub input: Value,
    #[serde(default)]
    pub edits: InvocationEdits,
}

impl App {
    pub async fn model_contract(
        &self,
        ctx: &CallContext,
        request: ModelContractRequest,
    ) -> Result<Value> {
        validate_name(&request.role, "model role")?;
        let scope = self.model_management_scope(ctx, request.scope).await?;
        Ok(json!(
            self.model
                .contract(&scope, &request.role, request.input)
                .await?
        ))
    }
    pub async fn model_preview(
        &self,
        ctx: &CallContext,
        request: ModelPreviewRequest,
    ) -> Result<Value> {
        validate_name(&request.role, "model role")?;
        let scope = self.model_management_scope(ctx, request.scope).await?;
        let preview = self
            .model
            .preview(&scope, &request.role, request.input, request.edits)
            .await
            .map_err(|error| error.to_domain_error())?;
        Ok(json!(preview))
    }
    async fn model_management_scope(
        &self,
        ctx: &CallContext,
        scope: Option<ConfigScope>,
    ) -> Result<ConfigScope> {
        let scope = scope.unwrap_or_else(|| ConfigScope::for_caller(ctx));
        scope.require_access(ctx)?;
        let mut view = self.database.read_view().await?;
        view.validate_configuration_scope(&scope).await?;
        view.finish().await?;
        Ok(scope)
    }
}
