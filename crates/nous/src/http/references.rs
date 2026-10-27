use super::*;
pub use nous_app::references::ReferenceOperation;
pub(super) fn router() -> Router<ApiState> {
    Router::new().route("/api/references", post(references))
}
async fn references(
    State(state): State<ApiState>,
    Extension(ctx): Extension<CallContext>,
    body: Result<Json<ReferenceOperation>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    run(state, ctx, Operation::Reference(decode(body)?)).await
}
