use super::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WaitJobs {
    pub ids: Vec<String>,
    pub timeout_ms: u64,
}
pub(super) fn router()->Router<ApiState> {
    Router::new().route("/api/jobs/wait",post(wait))
}
async fn wait(State(state):State<ApiState>,Extension(ctx):Extension<CallContext>,body:Result<Json<WaitJobs>,JsonRejection>)->Result<Json<Value>,ApiError> {
    let request=decode(body)?;
    Ok(Json(json!(state.app.wait_jobs(&ctx,&request.ids,request.timeout_ms,&state.shutdown.child_token()).await?)))
}
