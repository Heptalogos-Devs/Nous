use super::{ApiError, ApiState, Operation, run};
use axum::{
    Json, Router,
    extract::{Extension, Path, State},
    routing::get,
};
use nous_domain::{CallContext, SubjectDiagnosticsRequest};
use serde_json::Value;

pub fn router() -> Router<ApiState> {
    Router::new().route("/api/subjects/{subject}/diagnostics", get(read))
}
async fn read(
    State(state): State<ApiState>,
    Extension(ctx): Extension<CallContext>,
    Path(subject): Path<String>,
) -> Result<Json<Value>, ApiError> {
    run(
        state,
        ctx,
        Operation::SubjectDiagnostics(SubjectDiagnosticsRequest {
            subject_id: subject.parse()?,
        }),
    )
    .await
}
