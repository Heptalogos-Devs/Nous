//! Memory operations and the shared cognition query transport.
use axum::{
    Json, Router,
    extract::{Extension, Path, State, rejection::JsonRejection},
    routing::{get, post},
};
use nous_app::App;
use nous_domain::{CallContext, Error, QueryRequest};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::http::{ApiError, ApiState, Operation, decode, run};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FormMemory {
    pub operation_id: String,
    pub subject_id: String,
    pub observation_id: String,
    pub source: Option<nous_domain::media::MediaReference>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DeleteMemory {
    pub operation_id: String,
    pub expected_revision: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EmbeddingSync {
    pub operation_id: String,
    pub subject_id: String,
    #[serde(default = "embedding_limit")]
    pub limit: usize,
}
fn embedding_limit() -> usize {
    5000
}

#[derive(Clone)]
pub enum CognitionOperation {
    Form(FormMemory),
    Get(String),
    History(String),
    Delete(String, DeleteMemory),
    Query(Box<QueryRequest>),
    Plan(Box<QueryRequest>),
    Manual,
    Source { id: String, revision: Option<i64> },
    ModelStatus,
    EmbeddingSync(EmbeddingSync),
    EmbeddingRun(String),
    ModelCall(String),
}

impl CognitionOperation {
    pub async fn execute(&self, app: &App, ctx: &CallContext) -> Result<Value, Error> {
        match self {
            Self::Form(input) => Ok(json!(
                app.form_memory(
                    ctx,
                    &input.operation_id,
                    &input.subject_id.parse()?,
                    &input.observation_id.parse()?,
                    input.source.clone()
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
            Self::Query(input) => Ok(json!(app.query(ctx, *input.clone()).await?)),
            Self::Plan(input) => Ok(json!(app.plan_query(ctx, *input.clone()).await?)),
            Self::Manual => Ok(json!({"text":nous_domain::nousql::MANUAL})),
            Self::Source { id, revision } => Ok(json!(
                app.memory_source(ctx, &id.parse()?, *revision).await?
            )),
            Self::ModelStatus => app.model_status(ctx).await,
            Self::EmbeddingSync(input) => Ok(json!(
                app.sync_embeddings(
                    ctx,
                    &input.operation_id,
                    &input.subject_id.parse()?,
                    input.limit
                )
                .await?
            )),
            Self::EmbeddingRun(id) => Ok(json!(app.get_embedding_run(ctx, id).await?)),
            Self::ModelCall(id) => Ok(json!(app.read_model_call(ctx, &id.parse()?).await?)),
        }
    }
    pub fn request(&self) -> (&'static str, String, Option<Value>) {
        match self {
            Self::Form(input) => ("POST", "/api/memories/form".into(), Some(json!(input))),
            Self::Get(id) => ("GET", format!("/api/memories/{id}"), None),
            Self::History(id) => ("GET", format!("/api/memories/{id}/history"), None),
            Self::Delete(id, input) => (
                "POST",
                format!("/api/memories/{id}/delete"),
                Some(json!(input)),
            ),
            Self::Query(input) => ("POST", "/api/query".into(), Some(json!(input))),
            Self::Plan(input) => ("POST", "/api/query/plan".into(), Some(json!(input))),
            Self::Manual => ("GET", "/api/query/manual".into(), None),
            Self::Source { id, revision } => (
                "GET",
                match revision {
                    Some(revision) => format!("/api/memories/{id}/source?revision={revision}"),
                    None => format!("/api/memories/{id}/source"),
                },
                None,
            ),
            Self::ModelStatus => ("GET", "/api/models/status".into(), None),
            Self::EmbeddingSync(input) => {
                ("POST", "/api/embeddings/sync".into(), Some(json!(input)))
            }
            Self::EmbeddingRun(id) => ("GET", format!("/api/embeddings/runs/{id}"), None),
            Self::ModelCall(id) => ("GET", format!("/api/models/calls/{id}"), None),
        }
    }
}

pub fn router() -> Router<ApiState> {
    Router::new()
        .route("/api/memories/form", post(form))
        .route("/api/memories/{memory}", get(read))
        .route("/api/memories/{memory}/history", get(history))
        .route("/api/memories/{memory}/delete", post(delete))
        .route("/api/memories/{memory}/source", get(source))
        .route("/api/query", post(query))
        .route("/api/query/plan", post(plan))
        .route("/api/query/manual", get(manual))
        .route("/api/models/status", get(model_status))
        .route("/api/embeddings/sync", post(embedding_sync))
        .route("/api/embeddings/runs/{id}", get(embedding_run))
        .route("/api/models/calls/{id}", get(model_call))
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
    body: Result<Json<QueryRequest>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    run(
        s,
        c,
        Operation::Cognition(CognitionOperation::Query(Box::new(decode(body)?))),
    )
    .await
}
async fn plan(
    State(state): State<ApiState>,
    Extension(ctx): Extension<CallContext>,
    body: Result<Json<QueryRequest>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    run(
        state,
        ctx,
        Operation::Cognition(CognitionOperation::Plan(Box::new(decode(body)?))),
    )
    .await
}
async fn manual(
    State(state): State<ApiState>,
    Extension(ctx): Extension<CallContext>,
) -> Result<Json<Value>, ApiError> {
    run(state, ctx, Operation::Cognition(CognitionOperation::Manual)).await
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceRevision {
    revision: Option<i64>,
}
async fn source(
    State(s): State<ApiState>,
    Extension(c): Extension<CallContext>,
    Path(id): Path<String>,
    query: Result<axum::extract::Query<SourceRevision>, axum::extract::rejection::QueryRejection>,
) -> Result<Json<Value>, ApiError> {
    let query = query
        .map_err(|error| Error::invalid("invalid source revision query").with_cause(&error))?
        .0;
    run(
        s,
        c,
        Operation::Cognition(CognitionOperation::Source {
            id,
            revision: query.revision,
        }),
    )
    .await
}
async fn model_status(
    State(s): State<ApiState>,
    Extension(c): Extension<CallContext>,
) -> Result<Json<Value>, ApiError> {
    run(s, c, Operation::Cognition(CognitionOperation::ModelStatus)).await
}

async fn embedding_sync(
    State(s): State<ApiState>,
    Extension(c): Extension<CallContext>,
    body: Result<Json<EmbeddingSync>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    run(
        s,
        c,
        Operation::Cognition(CognitionOperation::EmbeddingSync(decode(body)?)),
    )
    .await
}
async fn embedding_run(
    State(s): State<ApiState>,
    Extension(c): Extension<CallContext>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    run(
        s,
        c,
        Operation::Cognition(CognitionOperation::EmbeddingRun(id)),
    )
    .await
}
async fn model_call(
    State(s): State<ApiState>,
    Extension(c): Extension<CallContext>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    run(
        s,
        c,
        Operation::Cognition(CognitionOperation::ModelCall(id)),
    )
    .await
}
