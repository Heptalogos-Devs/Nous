//! Authenticated HTTP transport and stable error mapping.
mod client;
pub mod cognition;
pub mod configuration;
pub mod maintenance;
pub mod models;
mod operation;
pub mod prompts;
pub mod structured;
mod trace;
use crate::mcp::McpService;
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Extension, Path, State, rejection::JsonRejection},
    http::{Request, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
pub use client::ApiClient;
use nous_app::App;
use nous_domain::{CallContext, Error, ErrorCode, TraceId};
use nous_foundation::instance::InstanceConfig;
use nous_foundation::instance::TokenStore;
pub use operation::*;
use serde_json::{Value, json};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
#[derive(Clone)]
pub struct ApiState {
    pub app: App,
    pub tokens: Arc<TokenStore>,
    pub shutdown: CancellationToken,
}

#[derive(Debug)]
pub struct ApiError(pub Error, pub Option<StatusCode>);
impl From<Error> for ApiError {
    fn from(error: Error) -> Self {
        Self(error, None)
    }
}
impl IntoResponse for ApiError {
    fn into_response(mut self) -> Response {
        self.0 = nous_foundation::observability::contextualize(self.0);
        let status = self.1.unwrap_or(match self.0.code {
            ErrorCode::InvalidArgument => StatusCode::BAD_REQUEST,
            ErrorCode::Unauthorized => StatusCode::UNAUTHORIZED,
            ErrorCode::Forbidden => StatusCode::FORBIDDEN,
            ErrorCode::NotFound => StatusCode::NOT_FOUND,
            ErrorCode::Conflict => StatusCode::CONFLICT,
            ErrorCode::CapabilityUnavailable => StatusCode::UNPROCESSABLE_ENTITY,
            ErrorCode::ResourceExhausted => StatusCode::TOO_MANY_REQUESTS,
            ErrorCode::DependencyUnavailable => StatusCode::SERVICE_UNAVAILABLE,
            ErrorCode::Internal => StatusCode::INTERNAL_SERVER_ERROR,
            ErrorCode::Cancelled => StatusCode::from_u16(499).expect("cancellation status"),
        });
        tracing::warn!(event="request.rejected", error_code=?self.0.code, error=%self.0, details=?self.0.context, cause=?self.0.cause, "request failed");
        let mut response = (status, Json(json!({"error":self.0}))).into_response();
        if status == StatusCode::UNAUTHORIZED {
            response.headers_mut().insert(
                header::WWW_AUTHENTICATE,
                header::HeaderValue::from_static("Bearer"),
            );
        }
        response
    }
}

pub fn router(state: ApiState, config: &InstanceConfig, cancellation: CancellationToken) -> Router {
    use rmcp::transport::streamable_http_server::{
        StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
    };
    let app = state.app.clone();
    let mcp = StreamableHttpService::new(
        move || Ok(McpService::local(app.clone())),
        LocalSessionManager::default().into(),
        StreamableHttpServerConfig::default()
            .with_cancellation_token(cancellation)
            .with_max_request_body_bytes(config.server.max_body_bytes),
    );
    Router::new()
        .route("/api/status", get(status))
        .route("/api/diagnostics", get(diagnostics))
        .route("/api/traces/query", post(trace_records))
        .route("/api/metrics", get(metrics))
        .route("/api/stop", post(stop))
        .route("/api/backups", post(create_backup))
        .route(
            "/api/namespaces",
            get(list_namespaces).post(create_namespace),
        )
        .route(
            "/api/namespaces/{namespace}/subjects",
            get(list_subjects).post(create_subject),
        )
        .route("/api/materials", post(put_material))
        .route("/api/materials/{material}", get(get_material))
        .route("/api/observations", post(record_observation))
        .route(
            "/api/subjects/{subject}/observations",
            get(list_observations),
        )
        .route("/api/jobs/{job}", get(get_job))
        .route("/api/jobs/{job}/cancel", post(cancel_job))
        .merge(crate::http::cognition::router())
        .merge(crate::http::configuration::router())
        .merge(crate::http::models::router())
        .merge(crate::http::prompts::router())
        .merge(crate::http::structured::router())
        .merge(crate::http::maintenance::router())
        .nest_service("/mcp", mcp)
        .fallback(|| async {
            ApiError(
                Error::new(ErrorCode::NotFound, "route not found"),
                Some(StatusCode::NOT_FOUND),
            )
        })
        .method_not_allowed_fallback(|| async {
            ApiError(
                Error::invalid("HTTP method is not supported for this route"),
                Some(StatusCode::METHOD_NOT_ALLOWED),
            )
        })
        .layer(DefaultBodyLimit::max(config.server.max_body_bytes))
        .layer(middleware::from_fn_with_state(
            state.tokens.clone(),
            authenticate,
        ))
        .layer(middleware::from_fn(trace::trace_request))
        .with_state(state)
}

async fn authenticate(
    State(tokens): State<Arc<TokenStore>>,
    mut request: Request<axum::body::Body>,
    next: Next,
) -> Result<Response, ApiError> {
    let token = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));
    let identity = token
        .and_then(|token| tokens.authenticate(token))
        .ok_or_else(|| Error::new(ErrorCode::Unauthorized, "a valid bearer token is required"))?;
    let trace_id = request
        .extensions()
        .get::<TraceId>()
        .cloned()
        .ok_or_else(|| Error::internal("request trace middleware is missing"))?;
    let ctx = identity.begin_call(trace_id);
    let span = tracing::info_span!("authenticated", caller=%ctx.caller_id);
    request.extensions_mut().insert(ctx);
    use tracing::Instrument;
    Ok(next.run(request).instrument(span).await)
}

pub(crate) async fn run(
    state: ApiState,
    ctx: CallContext,
    operation: Operation,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(operation.execute(&state.app, &ctx).await?))
}
pub(crate) fn decode<T>(body: Result<Json<T>, JsonRejection>) -> Result<T, ApiError> {
    body.map(|Json(body)| body).map_err(|rejection| {
        let status = rejection.status();
        let code = if status == StatusCode::PAYLOAD_TOO_LARGE {
            ErrorCode::ResourceExhausted
        } else {
            ErrorCode::InvalidArgument
        };
        ApiError(Error::new(code, rejection.body_text()), Some(status))
    })
}
async fn status(
    State(s): State<ApiState>,
    Extension(c): Extension<CallContext>,
) -> Result<Json<Value>, ApiError> {
    run(s, c, Operation::Status).await
}

async fn trace_records(
    State(s): State<ApiState>,
    Extension(c): Extension<CallContext>,
    body: Result<Json<nous_app::administration::observability::TraceRequest>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    run(s, c, Operation::Trace(decode(body)?)).await
}

async fn diagnostics(
    State(s): State<ApiState>,
    Extension(c): Extension<CallContext>,
) -> Result<Json<Value>, ApiError> {
    run(s, c, Operation::Diagnostics).await
}

async fn metrics(Extension(c): Extension<CallContext>) -> Result<impl IntoResponse, ApiError> {
    c.require_admin()?;
    Ok((
        [(
            header::CONTENT_TYPE,
            "text/plain; version=0.0.4; charset=utf-8",
        )],
        nous_foundation::observability::Observability::current()?.metrics(),
    ))
}
async fn stop(
    State(s): State<ApiState>,
    Extension(c): Extension<CallContext>,
) -> Result<Json<Value>, ApiError> {
    c.require_admin()?;
    s.shutdown.cancel();
    Ok(Json(json!({"state":"stopping"})))
}
async fn create_backup(
    State(s): State<ApiState>,
    Extension(c): Extension<CallContext>,
) -> Result<Json<Value>, ApiError> {
    run(s, c, Operation::CreateBackup).await
}
async fn list_namespaces(
    State(s): State<ApiState>,
    Extension(c): Extension<CallContext>,
) -> Result<Json<Value>, ApiError> {
    run(s, c, Operation::ListNamespaces).await
}
async fn create_namespace(
    State(s): State<ApiState>,
    Extension(c): Extension<CallContext>,
    body: Result<Json<CreateNamespace>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    run(s, c, Operation::CreateNamespace(decode(body)?)).await
}
async fn list_subjects(
    State(s): State<ApiState>,
    Extension(c): Extension<CallContext>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    run(s, c, Operation::ListSubjects(id)).await
}
async fn create_subject(
    State(s): State<ApiState>,
    Extension(c): Extension<CallContext>,
    Path(id): Path<String>,
    body: Result<Json<CreateSubject>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    run(s, c, Operation::CreateSubject(id, decode(body)?)).await
}
async fn put_material(
    State(s): State<ApiState>,
    Extension(c): Extension<CallContext>,
    body: Result<Json<PutMaterial>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    run(s, c, Operation::PutMaterial(decode(body)?)).await
}
async fn get_material(
    State(s): State<ApiState>,
    Extension(c): Extension<CallContext>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    run(s, c, Operation::GetMaterial(id)).await
}
async fn record_observation(
    State(s): State<ApiState>,
    Extension(c): Extension<CallContext>,
    body: Result<Json<RecordObservation>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    run(s, c, Operation::RecordObservation(decode(body)?)).await
}
async fn list_observations(
    State(s): State<ApiState>,
    Extension(c): Extension<CallContext>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    run(s, c, Operation::ListObservations(id)).await
}
async fn get_job(
    State(s): State<ApiState>,
    Extension(c): Extension<CallContext>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    run(s, c, Operation::GetJob(id)).await
}

async fn cancel_job(
    State(state): State<ApiState>,
    Extension(context): Extension<CallContext>,
    Path(id): Path<String>,
    body: Result<Json<CancelJob>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    run(state, context, Operation::CancelJob(id, decode(body)?)).await
}
