//! Lifecycle, maintenance and bounded text-import protocol mapping.
use crate::http::{ApiError, ApiState, Operation, decode, run};
use axum::{
    Json, Router,
    extract::{Extension, Path, Query, State, rejection::JsonRejection},
    routing::{get, post},
};
use nous_app::App;
use nous_domain::{CallContext, Error};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, clap::ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Reinforce,
    Weaken,
    Protect,
    Unprotect,
    Freeze,
    Forget,
    Restore,
    Trash,
    Purge,
    Revise,
    Decay,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EditInput {
    pub title: Option<String>,
    pub content: Option<String>,
    pub epistemic_status: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ChangeInput {
    pub operation_id: String,
    pub action: Action,
    pub expected_revision: i64,
    pub strength: Option<f64>,
    pub hours: Option<f64>,
    #[serde(default)]
    pub reason: String,
    #[serde(default)]
    pub force: bool,
    pub edit: Option<EditInput>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MaintainInput {
    pub operation_id: String,
    #[serde(default = "default_limit")]
    pub limit: usize,
    pub after: Option<String>,
    pub snapshot_seq: Option<i64>,
}
fn default_limit() -> usize {
    50
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct ListInput {
    pub limit: usize,
}
impl Default for ListInput {
    fn default() -> Self {
        Self {
            limit: default_limit(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ImportInput {
    pub operation_id: String,
    pub namespace_id: String,
    pub subject_id: String,
    pub inputs: Vec<nous_domain::ImportDocument>,
    #[serde(default)]
    pub form_memory: bool,
    /// Captured origin for records without their own origin fact.
    pub origin: Option<nous_domain::ObservationOrigin>,
}

#[derive(Clone)]
pub enum MaintenanceOperation {
    Change(String, ChangeInput),
    Recycle(String, ListInput),
    Audit(String, ListInput),
    Maintain(String, MaintainInput),
    Import(ImportInput),
}
impl MaintenanceOperation {
    pub async fn execute(&self, app: &App, ctx: &CallContext) -> Result<Value, Error> {
        match self {
            Self::Change(id, input) => {
                let mut change = json!(input);
                change
                    .as_object_mut()
                    .expect("serialized change object")
                    .remove("operation_id");
                let change: nous_domain::MemoryChange =
                    serde_json::from_value(change).map_err(|e| Error::invalid(e.to_string()))?;
                Ok(json!(
                    app.change_memory(ctx, &input.operation_id, &id.parse()?, change)
                        .await?
                ))
            }
            Self::Recycle(subject, input) => Ok(json!(
                app.recycle_bin(ctx, &subject.parse()?, input.limit).await?
            )),
            Self::Audit(id, input) => Ok(json!(
                app.memory_audit(ctx, &id.parse()?, input.limit).await?
            )),
            Self::Maintain(subject, input) => Ok(json!(
                app.maintain_subject(
                    ctx,
                    &input.operation_id,
                    &subject.parse()?,
                    input.limit,
                    input.after.as_deref().map(str::parse).transpose()?,
                    input.snapshot_seq
                )
                .await?
            )),
            Self::Import(input) => {
                let mut request = json!(input);
                request
                    .as_object_mut()
                    .expect("serialized import input")
                    .remove("operation_id");
                let request: nous_domain::ImportRequest =
                    serde_json::from_value(request).map_err(|e| Error::invalid(e.to_string()))?;
                Ok(json!(
                    app.submit_import(ctx, &input.operation_id, request).await?
                ))
            }
        }
    }
    pub fn request(&self) -> (&'static str, String, Option<Value>) {
        match self {
            Self::Change(id, input) => (
                "POST",
                format!("/api/memories/{id}/change"),
                Some(json!(input)),
            ),
            Self::Recycle(id, input) => (
                "GET",
                format!("/api/subjects/{id}/recycle-bin?limit={}", input.limit),
                None,
            ),
            Self::Audit(id, input) => (
                "GET",
                format!("/api/memories/{id}/audit?limit={}", input.limit),
                None,
            ),
            Self::Maintain(id, input) => (
                "POST",
                format!("/api/subjects/{id}/maintenance"),
                Some(json!(input)),
            ),
            Self::Import(input) => ("POST", "/api/imports/text".into(), Some(json!(input))),
        }
    }
}
pub fn router() -> Router<ApiState> {
    Router::new()
        .route("/api/memories/{memory}/change", post(change))
        .route("/api/memories/{memory}/audit", get(audit))
        .route("/api/subjects/{subject}/recycle-bin", get(recycle))
        .route("/api/subjects/{subject}/maintenance", post(maintain))
        .route("/api/imports/text", post(import))
}
async fn change(
    State(s): State<ApiState>,
    Extension(c): Extension<CallContext>,
    Path(id): Path<String>,
    body: Result<Json<ChangeInput>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    run(
        s,
        c,
        Operation::Maintenance(MaintenanceOperation::Change(id, decode(body)?)),
    )
    .await
}
async fn audit(
    State(s): State<ApiState>,
    Extension(c): Extension<CallContext>,
    Path(id): Path<String>,
    input: Result<Query<ListInput>, axum::extract::rejection::QueryRejection>,
) -> Result<Json<Value>, ApiError> {
    run(
        s,
        c,
        Operation::Maintenance(MaintenanceOperation::Audit(id, list_input(input)?)),
    )
    .await
}
async fn recycle(
    State(s): State<ApiState>,
    Extension(c): Extension<CallContext>,
    Path(id): Path<String>,
    input: Result<Query<ListInput>, axum::extract::rejection::QueryRejection>,
) -> Result<Json<Value>, ApiError> {
    run(
        s,
        c,
        Operation::Maintenance(MaintenanceOperation::Recycle(id, list_input(input)?)),
    )
    .await
}
fn list_input(
    input: Result<Query<ListInput>, axum::extract::rejection::QueryRejection>,
) -> Result<ListInput, ApiError> {
    input
        .map(|Query(input)| input)
        .map_err(|error| Error::invalid(error.body_text()).into())
}
async fn maintain(
    State(s): State<ApiState>,
    Extension(c): Extension<CallContext>,
    Path(id): Path<String>,
    body: Result<Json<MaintainInput>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    run(
        s,
        c,
        Operation::Maintenance(MaintenanceOperation::Maintain(id, decode(body)?)),
    )
    .await
}
async fn import(
    State(s): State<ApiState>,
    Extension(c): Extension<CallContext>,
    body: Result<Json<ImportInput>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    run(
        s,
        c,
        Operation::Maintenance(MaintenanceOperation::Import(decode(body)?)),
    )
    .await
}
