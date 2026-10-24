//! Prompt operations share the application's resource and activation contracts.
use super::{ApiError, ApiState, Operation, decode, run};
use axum::{
    Json, Router,
    extract::{Extension, State, rejection::JsonRejection},
    routing::post,
};
use nous_app::administration::prompts::PromptRequest;
use nous_domain::CallContext;
use serde_json::Value;
pub fn router() -> Router<ApiState> {
    Router::new().route("/api/prompts", post(request))
}
async fn request(
    State(state): State<ApiState>,
    Extension(ctx): Extension<CallContext>,
    body: Result<Json<PromptRequest>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    run(state, ctx, Operation::Prompt(decode(body)?)).await
}
