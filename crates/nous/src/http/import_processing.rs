use super::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProcessImportInput {
    pub operation_id: String,
    pub request: nous_domain::ProcessImportUnitsRequest,
}
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReadImportOutputInput {
    pub namespace_id: nous_domain::NamespaceId,
    pub subject_id: nous_domain::SubjectId,
    pub output_id: nous_domain::ImportUnitOutputId,
}
pub(super) fn router() -> Router<ApiState> {
    Router::new()
        .route("/api/imports/process", post(process))
        .route("/api/imports/outputs/read", post(read_output))
}
async fn read_output(
    State(state): State<ApiState>,
    Extension(ctx): Extension<CallContext>,
    body: Result<Json<ReadImportOutputInput>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    run(state, ctx, Operation::ReadImportOutput(decode(body)?)).await
}
async fn process(
    State(state): State<ApiState>,
    Extension(ctx): Extension<CallContext>,
    body: Result<Json<ProcessImportInput>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    run(
        state,
        ctx,
        Operation::ProcessImportUnits(Box::new(decode(body)?)),
    )
    .await
}
