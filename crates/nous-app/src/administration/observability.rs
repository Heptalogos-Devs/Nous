use crate::App;
use nous_domain::{CallContext, Result, TraceId};
use nous_storage::TraceCursor;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TraceRequest {
    pub trace_id: TraceId,
    pub cursor: Option<TraceCursor>,
    #[serde(default = "page_limit")]
    pub limit: usize,
}
fn page_limit() -> usize {
    100
}

impl App {
    pub async fn trace_records(&self, ctx: &CallContext, request: &TraceRequest) -> Result<Value> {
        ctx.require_admin()?;
        Ok(json!(
            self.database
                .trace_records(&request.trace_id, request.cursor.as_ref(), request.limit)
                .await?
        ))
    }
}
