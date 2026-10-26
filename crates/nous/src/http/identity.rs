//! Typed identity requests share one application use case across transports.
use super::*;
use nous_app::administration::identity::IdentityRequest;

pub(super) fn router() -> Router<ApiState> {
    Router::new().route("/api/identity", post(identity))
}
async fn identity(
    State(state): State<ApiState>,
    Extension(ctx): Extension<CallContext>,
    input: Result<Json<IdentityRequest>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    run(state, ctx, Operation::Identity(decode(input)?)).await
}
