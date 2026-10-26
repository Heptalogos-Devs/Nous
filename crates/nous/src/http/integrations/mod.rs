mod amemorix;
use crate::http::ApiState;
pub use amemorix::AmemorixRequest;
use axum::Router;

pub(super) fn router() -> Router<ApiState> {
    amemorix::router()
}
