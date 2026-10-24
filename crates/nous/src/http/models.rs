//! Model contract and invocation preview routes; all construction stays in the model owner.
use super::{ApiError, ApiState, Operation, decode, run};
use axum::{
    Json, Router,
    extract::{Extension, State, rejection::JsonRejection},
    routing::post,
};
use nous_app::model_management::{ModelContractRequest, ModelPreviewRequest};
use nous_domain::CallContext;
use serde_json::Value;

pub fn router() -> Router<ApiState> {
    Router::new()
        .route("/api/models/contract", post(contract))
        .route("/api/models/preview", post(preview))
}
async fn contract(
    State(state): State<ApiState>,
    Extension(ctx): Extension<CallContext>,
    body: Result<Json<ModelContractRequest>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    run(state, ctx, Operation::ModelContract(decode(body)?)).await
}
async fn preview(
    State(state): State<ApiState>,
    Extension(ctx): Extension<CallContext>,
    body: Result<Json<ModelPreviewRequest>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    run(state, ctx, Operation::ModelPreview(decode(body)?)).await
}
