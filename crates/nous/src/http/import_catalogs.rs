use crate::http::{ApiError, ApiState, Operation, decode, run};
use axum::{
    Json, Router,
    extract::{Extension, State, rejection::JsonRejection},
    routing::post,
};
use futures::{FutureExt, future::BoxFuture};
use nous_app::App;
use nous_domain::*;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum ImportCatalogOperation {
    Freeze {
        operation_id: String,
        request: FreezeImportCatalogRequest,
    },
    Inspect {
        request: ReadImportCatalogRequest,
    },
    RetryInputs {
        request: RetryImportInputsRequest,
    },
}
impl ImportCatalogOperation {
    pub fn execute<'a>(
        &'a self,
        app: &'a App,
        ctx: &'a CallContext,
    ) -> BoxFuture<'a, Result<Value>> {
        match self {
            Self::Freeze {
                operation_id,
                request,
            } => async move {
                let cancel = tokio_util::sync::CancellationToken::new();
                let _guard = cancel.clone().drop_guard();
                Ok(json!(
                    app.freeze_import_catalog(ctx, operation_id, request.clone(), &cancel)
                        .await?
                ))
            }
            .boxed(),
            Self::Inspect { request } => async move {
                let cancel = tokio_util::sync::CancellationToken::new();
                let _guard = cancel.clone().drop_guard();
                Ok(json!(
                    app.import_catalog(ctx, request.clone(), &cancel).await?
                ))
            }
            .boxed(),
            Self::RetryInputs { request } => async move {
                let cancel = tokio_util::sync::CancellationToken::new();
                let _guard = cancel.clone().drop_guard();
                Ok(json!(
                    app.retry_import_inputs(ctx, request.clone(), &cancel)
                        .await?
                ))
            }
            .boxed(),
        }
    }
}
pub fn router() -> Router<ApiState> {
    Router::new().route("/api/imports/catalogs", post(handle))
}
async fn handle(
    State(state): State<ApiState>,
    Extension(ctx): Extension<CallContext>,
    body: std::result::Result<Json<ImportCatalogOperation>, JsonRejection>,
) -> std::result::Result<Json<Value>, ApiError> {
    run(
        state,
        ctx,
        Operation::ImportCatalog(Box::new(decode(body)?)),
    )
    .await
}
