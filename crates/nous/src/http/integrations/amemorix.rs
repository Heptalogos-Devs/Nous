use crate::http::*;
pub use nous_app::integrations::amemorix::AmemorixRequest;

pub(super) fn router() -> Router<ApiState> {
    Router::new().route("/api/integrations/amemorix", post(amemorix))
}
async fn amemorix(
    State(state): State<ApiState>,
    Extension(ctx): Extension<CallContext>,
    body: Result<Json<AmemorixRequest>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    run(state, ctx, Operation::Amemorix(Box::new(decode(body)?))).await
}
