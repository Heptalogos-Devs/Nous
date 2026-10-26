use super::*;
pub use nous_app::cognition::read::RetrievalSourceRequest;

pub(super) fn router() -> Router<ApiState> {
    Router::new().route("/api/sources/read", post(read))
}
async fn read(
    State(state): State<ApiState>,
    Extension(ctx): Extension<CallContext>,
    body: Result<Json<RetrievalSourceRequest>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    run(state, ctx, Operation::ReadSource(decode(body)?)).await
}
