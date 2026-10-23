//! Memory formation, reading and retrieval transport mapping.
use axum::{
    Json, Router,
    extract::{Extension, Path, State, rejection::JsonRejection},
    routing::{get, post},
};
use nous_app::App;
use nous_domain::{CallContext, Error};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::api::{ApiError, ApiState, Operation, decode, run};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FormMemory {
    pub operation_id: String,
    pub subject_id: String,
    pub observation_id: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DeleteMemory {
    pub operation_id: String,
    pub expected_revision: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct QueryInput {
    pub subject_id: String,
    #[serde(default)]
    pub text: String,
    pub reference: Option<String>,
    pub source: Option<String>,
    pub time: Option<QueryTimeFilter>,
    #[serde(default = "default_limit")]
    pub limit: usize,
    pub min_commit_seq: Option<i64>,
}
fn default_limit() -> usize {
    10
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct QueryTimeFilter {
    /// event, observation, formation, recorded, or validity.
    pub axis: String,
    pub range: QueryTimeRange,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct QueryTimeRange {
    pub domain: String,
    pub start: Option<i64>,
    pub end: Option<i64>,
}

#[derive(Clone)]
pub enum CognitionOperation {
    Form(FormMemory),
    Get(String),
    History(String),
    Delete(String, DeleteMemory),
    Query(QueryInput),
    Source(String),
    ModelStatus,
}

impl CognitionOperation {
    pub async fn execute(&self, app: &App, ctx: &CallContext) -> Result<Value, Error> {
        match self {
            Self::Form(input) => Ok(json!(
                app.form_memory(
                    ctx,
                    &input.operation_id,
                    &input.subject_id.parse()?,
                    &input.observation_id.parse()?
                )
                .await?
            )),
            Self::Get(id) => Ok(json!(app.get_memory(ctx, &id.parse()?).await?)),
            Self::History(id) => Ok(json!(app.memory_history(ctx, &id.parse()?).await?)),
            Self::Delete(id, input) => Ok(json!(
                app.delete_memory(
                    ctx,
                    &input.operation_id,
                    &id.parse()?,
                    input.expected_revision
                )
                .await?
            )),
            Self::Query(input) => {
                let query: nous_domain::QueryRequest = serde_json::from_value(json!(input))
                    .map_err(|error| Error::invalid(error.to_string()))?;
                Ok(json!(app.query(ctx, query).await?))
            }
            Self::Source(id) => Ok(json!(app.memory_source(ctx, &id.parse()?).await?)),
            Self::ModelStatus => app.model_status(ctx).await,
        }
    }
    pub fn request(&self) -> (&'static str, String, Option<Value>) {
        match self {
            Self::Form(input) => ("POST", "/v1/memories/form".into(), Some(json!(input))),
            Self::Get(id) => ("GET", format!("/v1/memories/{id}"), None),
            Self::History(id) => ("GET", format!("/v1/memories/{id}/history"), None),
            Self::Delete(id, input) => (
                "POST",
                format!("/v1/memories/{id}/delete"),
                Some(json!(input)),
            ),
            Self::Query(input) => ("POST", "/v1/query".into(), Some(json!(input))),
            Self::Source(id) => ("GET", format!("/v1/memories/{id}/source"), None),
            Self::ModelStatus => ("GET", "/v1/models/status".into(), None),
        }
    }
}

pub fn router() -> Router<ApiState> {
    Router::new()
        .route("/v1/memories/form", post(form))
        .route("/v1/memories/{memory}", get(read))
        .route("/v1/memories/{memory}/history", get(history))
        .route("/v1/memories/{memory}/delete", post(delete))
        .route("/v1/memories/{memory}/source", get(source))
        .route("/v1/query", post(query))
        .route("/v1/models/status", get(model_status))
}
async fn form(
    State(s): State<ApiState>,
    Extension(c): Extension<CallContext>,
    body: Result<Json<FormMemory>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    run(
        s,
        c,
        Operation::Cognition(CognitionOperation::Form(decode(body)?)),
    )
    .await
}
async fn read(
    State(s): State<ApiState>,
    Extension(c): Extension<CallContext>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    run(s, c, Operation::Cognition(CognitionOperation::Get(id))).await
}
async fn history(
    State(s): State<ApiState>,
    Extension(c): Extension<CallContext>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    run(s, c, Operation::Cognition(CognitionOperation::History(id))).await
}
async fn delete(
    State(s): State<ApiState>,
    Extension(c): Extension<CallContext>,
    Path(id): Path<String>,
    body: Result<Json<DeleteMemory>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    run(
        s,
        c,
        Operation::Cognition(CognitionOperation::Delete(id, decode(body)?)),
    )
    .await
}
async fn query(
    State(s): State<ApiState>,
    Extension(c): Extension<CallContext>,
    body: Result<Json<QueryInput>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    run(
        s,
        c,
        Operation::Cognition(CognitionOperation::Query(decode(body)?)),
    )
    .await
}
async fn source(
    State(s): State<ApiState>,
    Extension(c): Extension<CallContext>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    run(s, c, Operation::Cognition(CognitionOperation::Source(id))).await
}
async fn model_status(
    State(s): State<ApiState>,
    Extension(c): Extension<CallContext>,
) -> Result<Json<Value>, ApiError> {
    run(s, c, Operation::Cognition(CognitionOperation::ModelStatus)).await
}
