use super::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AdmitImportFilesInput {
    pub operation_id: String,
    pub request: nous_domain::AdmitImportFilesRequest,
}

pub(super) fn router() -> Router<ApiState> {
    Router::new().route("/api/imports/files", post(admit))
}

async fn admit(
    State(state): State<ApiState>,
    Extension(ctx): Extension<CallContext>,
    body: Result<Json<AdmitImportFilesInput>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    run(
        state,
        ctx,
        Operation::AdmitImportFiles(Box::new(decode(body)?)),
    )
    .await
}
