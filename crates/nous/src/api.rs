//! Protocol mapping around the same application operations for HTTP, CLI and MCP.
use std::{str::FromStr, sync::Arc};

use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Extension, Path, State, rejection::JsonRejection},
    http::{Request, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use nous_app::{
    App,
    instance::{InstanceConfig, TokenStore},
};
use nous_domain::{
    CallContext, Capability, Error, ErrorCode, MaterialId, NamespaceId, SubjectId, TimePoint,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use crate::mcp::McpService;

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
    fn into_response(self) -> Response {
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
        });
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

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateNamespace {
    pub operation_id: String,
    pub name: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateSubject {
    pub operation_id: String,
    pub name: String,
    #[serde(default = "memory_capability")]
    pub capabilities: Vec<String>,
}
fn memory_capability() -> Vec<String> {
    vec!["memory".into()]
}
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PutMaterial {
    pub operation_id: String,
    pub namespace_id: String,
    pub text: String,
    pub source: Option<String>,
    pub event_time: Option<WireTime>,
}
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RecordObservation {
    pub operation_id: String,
    pub subject_id: String,
    pub material_id: String,
    pub observed_at: Option<WireTime>,
}
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct WireTime {
    pub domain: String,
    pub coordinate: i64,
}
impl From<WireTime> for TimePoint {
    fn from(time: WireTime) -> Self {
        Self {
            domain: time.domain,
            coordinate: time.coordinate,
        }
    }
}

#[derive(Clone)]
pub enum Operation {
    Status,
    CreateNamespace(CreateNamespace),
    ListNamespaces,
    CreateSubject(String, CreateSubject),
    ListSubjects(String),
    PutMaterial(PutMaterial),
    GetMaterial(String),
    RecordObservation(RecordObservation),
    ListObservations(String),
    GetJob(String),
}

impl Operation {
    pub async fn execute(&self, app: &App, ctx: &CallContext) -> Result<Value, Error> {
        match self {
            Self::Status => {
                ctx.require_admin()?;
                app.status().await
            }
            Self::CreateNamespace(input) => Ok(serde_json::to_value(
                app.create_namespace(ctx, &input.operation_id, &input.name)
                    .await?,
            )?),
            Self::ListNamespaces => Ok(serde_json::to_value(app.list_namespaces(ctx).await?)?),
            Self::CreateSubject(namespace, input) => {
                let capabilities = input
                    .capabilities
                    .iter()
                    .map(|capability| match capability.as_str() {
                        "memory" => Ok(Capability::Memory),
                        _ => Err(Error::invalid(format!("unknown capability: {capability}"))),
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(serde_json::to_value(
                    app.create_subject(
                        ctx,
                        &input.operation_id,
                        &NamespaceId::from_str(namespace)?,
                        &input.name,
                        capabilities,
                    )
                    .await?,
                )?)
            }
            Self::ListSubjects(namespace) => Ok(serde_json::to_value(
                app.list_subjects(ctx, &namespace.parse()?).await?,
            )?),
            Self::PutMaterial(input) => Ok(serde_json::to_value(
                app.put_material(
                    ctx,
                    &input.operation_id,
                    &input.namespace_id.parse()?,
                    &input.text,
                    input.source.clone(),
                    input.event_time.clone().map(Into::into),
                )
                .await?,
            )?),
            Self::GetMaterial(id) => Ok(serde_json::to_value(
                app.get_material(ctx, &MaterialId::from_str(id)?).await?,
            )?),
            Self::RecordObservation(input) => Ok(serde_json::to_value(
                app.record_observation(
                    ctx,
                    &input.operation_id,
                    &input.subject_id.parse()?,
                    &input.material_id.parse()?,
                    input.observed_at.clone().map(Into::into),
                )
                .await?,
            )?),
            Self::ListObservations(subject) => Ok(serde_json::to_value(
                app.list_observations(ctx, &SubjectId::from_str(subject)?)
                    .await?,
            )?),
            Self::GetJob(id) => app.job(ctx, id).await,
        }
    }

    pub fn request(&self) -> (&'static str, String, Option<Value>) {
        match self {
            Self::Status => ("GET", "/v1/status".into(), None),
            Self::CreateNamespace(body) => ("POST", "/v1/namespaces".into(), Some(json!(body))),
            Self::ListNamespaces => ("GET", "/v1/namespaces".into(), None),
            Self::CreateSubject(id, body) => (
                "POST",
                format!("/v1/namespaces/{id}/subjects"),
                Some(json!(body)),
            ),
            Self::ListSubjects(id) => ("GET", format!("/v1/namespaces/{id}/subjects"), None),
            Self::PutMaterial(body) => ("POST", "/v1/materials".into(), Some(json!(body))),
            Self::GetMaterial(id) => ("GET", format!("/v1/materials/{id}"), None),
            Self::RecordObservation(body) => ("POST", "/v1/observations".into(), Some(json!(body))),
            Self::ListObservations(id) => ("GET", format!("/v1/subjects/{id}/observations"), None),
            Self::GetJob(id) => ("GET", format!("/v1/jobs/{id}"), None),
        }
    }
}

pub fn router(state: ApiState, config: &InstanceConfig, cancellation: CancellationToken) -> Router {
    // Adapted from the official rmcp counter_streamhttp and simple_auth_streamhttp examples.
    // Source and Apache-2.0 license: ../UPSTREAM.md and ../licenses/rmcp-LICENSE.
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
        .route("/v1/status", get(status))
        .route("/v1/stop", post(stop))
        .route(
            "/v1/namespaces",
            get(list_namespaces).post(create_namespace),
        )
        .route(
            "/v1/namespaces/{namespace}/subjects",
            get(list_subjects).post(create_subject),
        )
        .route("/v1/materials", post(put_material))
        .route("/v1/materials/{material}", get(get_material))
        .route("/v1/observations", post(record_observation))
        .route(
            "/v1/subjects/{subject}/observations",
            get(list_observations),
        )
        .route("/v1/jobs/{job}", get(get_job))
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
    let ctx = token
        .and_then(|token| tokens.authenticate(token))
        .ok_or_else(|| Error::new(ErrorCode::Unauthorized, "a valid bearer token is required"))?;
    let request_id = uuid::Uuid::new_v4().to_string();
    let span = tracing::info_span!("request", request_id=%request_id, caller=%ctx.caller_id, method=%request.method(), path=%request.uri().path());
    request.extensions_mut().insert(ctx);
    use tracing::Instrument;
    let mut response = next.run(request).instrument(span).await;
    response
        .headers_mut()
        .insert("x-request-id", request_id.parse().expect("UUID header"));
    Ok(response)
}

async fn run(
    state: ApiState,
    ctx: CallContext,
    operation: Operation,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(operation.execute(&state.app, &ctx).await?))
}
fn decode<T>(body: Result<Json<T>, JsonRejection>) -> Result<T, ApiError> {
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
async fn stop(
    State(s): State<ApiState>,
    Extension(c): Extension<CallContext>,
) -> Result<Json<Value>, ApiError> {
    c.require_admin()?;
    s.shutdown.cancel();
    Ok(Json(json!({"state":"stopping"})))
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

#[derive(Clone)]
pub struct ApiClient {
    pub http: reqwest::Client,
    pub base_url: String,
    token: Arc<String>,
}
impl ApiClient {
    pub fn new(http: reqwest::Client, base_url: String, token: String) -> anyhow::Result<Self> {
        let url: reqwest::Url = base_url.parse()?;
        anyhow::ensure!(
            matches!(url.scheme(), "http" | "https") && url.host().is_some(),
            "service URL must use HTTP or HTTPS"
        );
        anyhow::ensure!(
            url.username().is_empty() && url.password().is_none(),
            "service URL may not contain credentials"
        );
        Ok(Self {
            http,
            base_url: base_url.trim_end_matches('/').to_owned(),
            token: Arc::new(token),
        })
    }
    pub async fn execute(&self, operation: &Operation) -> anyhow::Result<Value> {
        Ok(self.execute_domain(operation).await?)
    }
    pub async fn execute_domain(&self, operation: &Operation) -> Result<Value, Error> {
        let (method, path, body) = operation.request();
        self.request(method, &path, body).await
    }
    pub async fn stop(&self) -> anyhow::Result<Value> {
        Ok(self.request("POST", "/v1/stop", None).await?)
    }
    async fn request(&self, method: &str, path: &str, body: Option<Value>) -> Result<Value, Error> {
        let mut request = self
            .http
            .request(
                method.parse().expect("constant HTTP method"),
                format!("{}{path}", self.base_url),
            )
            .bearer_auth(self.token.as_str());
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request
            .send()
            .await
            .map_err(|e| Error::new(ErrorCode::DependencyUnavailable, e.to_string()))?;
        let status = response.status();
        let text = response
            .text()
            .await
            .map_err(|e| Error::new(ErrorCode::DependencyUnavailable, e.to_string()))?;
        let value: Value = serde_json::from_str(&text).unwrap_or_else(|_| json!({"message":text}));
        if !status.is_success() {
            return Err(value
                .get("error")
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .unwrap_or_else(|| {
                    Error::new(
                        ErrorCode::DependencyUnavailable,
                        format!("service returned {status}: {value}"),
                    )
                }));
        }
        Ok(value)
    }
}
