//! Configuration management uses the application owner's single typed request.
use super::{ApiError, ApiState, Operation, decode, run};
use axum::{
    Json, Router,
    extract::{Extension, State, rejection::JsonRejection},
    routing::get,
};
use nous_app::common::configuration::ConfigRequest;
use nous_domain::CallContext;
use serde_json::Value;

pub fn router() -> Router<ApiState> {
    Router::new().route("/api/config", get(show).post(execute))
}

async fn show(
    State(state): State<ApiState>,
    Extension(ctx): Extension<CallContext>,
) -> Result<Json<Value>, ApiError> {
    run(
        state,
        ctx,
        Operation::Configuration(ConfigRequest::Show { scope: None }),
    )
    .await
}
async fn execute(
    State(state): State<ApiState>,
    Extension(ctx): Extension<CallContext>,
    body: Result<Json<ConfigRequest>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    run(state, ctx, Operation::Configuration(decode(body)?)).await
}
