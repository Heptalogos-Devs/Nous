//! Shared typed dispatch used by HTTP, CLI and MCP.
use nous_app::App;
use nous_domain::{CallContext, Capability, Error, MaterialId, NamespaceId, SubjectId, TimePoint};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::str::FromStr;
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
    pub origin: Option<nous_domain::ObservationOrigin>,
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

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CancelJob {
    pub operation_id: String,
}

#[derive(Clone)]
pub enum Operation {
    Amemorix(Box<super::integrations::AmemorixRequest>),
    ReadSource(super::sources::RetrievalSourceRequest),
    Bundle(Box<super::bundles::BundleOperation>),
    WaitJobs(super::tasks::WaitJobs),
    Context(Box<super::contexts::ContextOperation>),
    Media(Box<super::media::MediaOperation>),
    Identity(nous_app::administration::identity::IdentityRequest),
    Prompt(nous_app::administration::prompts::PromptRequest),
    ModelContract(nous_app::administration::models::ModelContractRequest),
    ModelPreview(nous_app::administration::models::ModelPreviewRequest),
    Configuration(nous_app::administration::configuration::ConfigRequest),
    Structured(Box<crate::http::structured::StructuredOperation>),
    Maintenance(crate::http::maintenance::MaintenanceOperation),
    CreateBackup,
    Cognition(crate::http::cognition::CognitionOperation),
    Status,
    Diagnostics,
    Trace(nous_app::administration::observability::TraceRequest),
    CreateNamespace(CreateNamespace),
    ListNamespaces,
    CreateSubject(String, CreateSubject),
    ListSubjects(String),
    PutMaterial(PutMaterial),
    GetMaterial(String),
    RecordObservation(RecordObservation),
    ListObservations(String),
    GetJob(String),
    CancelJob(String, CancelJob),
}

impl Operation {
    pub async fn execute(&self, app: &App, ctx: &CallContext) -> Result<Value, Error> {
        use tracing::Instrument;
        let (method, path, _) = self.request();
        let span = tracing::info_span!("api.dispatch", trace_id=%ctx.trace_id, caller=%ctx.caller_id, %method, %path);
        nous_foundation::observability::in_trace(
            ctx.trace_id.clone(),
            self.dispatch(app, ctx).instrument(span),
        )
        .await
        .map_err(|error| error.with_trace(ctx.trace_id.clone()))
    }

    async fn dispatch(&self, app: &App, ctx: &CallContext) -> Result<Value, Error> {
        match self {
            Self::Amemorix(request) => app.amemorix(ctx, *request.clone()).await,
            Self::ReadSource(request) => app.read_retrieval_source(ctx, request.clone()).await,
            Self::Bundle(operation) => operation.execute(app, ctx).await,
            Self::WaitJobs(request) => Ok(json!(
                app.wait_jobs(
                    ctx,
                    &request.ids,
                    request.timeout_ms,
                    &tokio_util::sync::CancellationToken::new()
                )
                .await?
            )),
            Self::Context(operation) => operation.execute(app, ctx).await,
            Self::Media(operation) => operation.execute(app, ctx).await,
            Self::Identity(request) => app.identity_request(ctx, request.clone()).await,
            Self::Trace(request) => app.trace_records(ctx, request).await,
            Self::Diagnostics => {
                ctx.require_admin()?;
                nous_foundation::observability::Observability::current()?.diagnostics()
            }
            Self::Prompt(request) => app.prompt_request(ctx, request.clone()).await,
            Self::ModelContract(request) => app.model_contract(ctx, request.clone()).await,
            Self::ModelPreview(request) => app.model_preview(ctx, request.clone()).await,
            Self::Configuration(request) => app.configuration_request(ctx, request.clone()).await,
            Self::Structured(operation) => operation.execute(app, ctx).await,
            Self::Maintenance(operation) => operation.execute(app, ctx).await,
            Self::CreateBackup => app.create_backup(ctx).await,
            Self::Cognition(operation) => operation.execute(app, ctx).await,
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
                    input.origin.clone(),
                )
                .await?,
            )?),
            Self::ListObservations(subject) => Ok(serde_json::to_value(
                app.list_observations(ctx, &SubjectId::from_str(subject)?)
                    .await?,
            )?),
            Self::GetJob(id) => app.job(ctx, id).await,
            Self::CancelJob(id, input) => {
                Ok(json!(app.cancel_job(ctx, &input.operation_id, id).await?))
            }
        }
    }

    pub fn request(&self) -> (&'static str, String, Option<Value>) {
        match self {
            Self::Amemorix(request) => (
                "POST",
                "/api/integrations/amemorix".into(),
                Some(json!(request)),
            ),
            Self::ReadSource(request) => ("POST", "/api/sources/read".into(), Some(json!(request))),
            Self::Bundle(operation) => ("POST", "/api/bundles".into(), Some(json!(operation))),
            Self::WaitJobs(request) => ("POST", "/api/jobs/wait".into(), Some(json!(request))),
            Self::Context(operation) => ("POST", "/api/contexts".into(), Some(json!(operation))),
            Self::Media(operation) => ("POST", "/api/media".into(), Some(json!(operation))),
            Self::Identity(request) => ("POST", "/api/identity".into(), Some(json!(request))),
            Self::Prompt(request) => ("POST", "/api/prompts".into(), Some(json!(request))),
            Self::ModelContract(request) => {
                ("POST", "/api/models/contract".into(), Some(json!(request)))
            }
            Self::ModelPreview(request) => {
                ("POST", "/api/models/preview".into(), Some(json!(request)))
            }
            Self::Configuration(request) => ("POST", "/api/config".into(), Some(json!(request))),
            Self::Structured(operation) => operation.request(),
            Self::Maintenance(operation) => operation.request(),
            Self::CreateBackup => ("POST", "/api/backups".into(), None),
            Self::Cognition(operation) => operation.request(),
            Self::Status => ("GET", "/api/status".into(), None),
            Self::Diagnostics => ("GET", "/api/diagnostics".into(), None),
            Self::Trace(request) => ("POST", "/api/traces/query".into(), Some(json!(request))),
            Self::CreateNamespace(body) => ("POST", "/api/namespaces".into(), Some(json!(body))),
            Self::ListNamespaces => ("GET", "/api/namespaces".into(), None),
            Self::CreateSubject(id, body) => (
                "POST",
                format!("/api/namespaces/{id}/subjects"),
                Some(json!(body)),
            ),
            Self::ListSubjects(id) => ("GET", format!("/api/namespaces/{id}/subjects"), None),
            Self::PutMaterial(body) => ("POST", "/api/materials".into(), Some(json!(body))),
            Self::GetMaterial(id) => ("GET", format!("/api/materials/{id}"), None),
            Self::RecordObservation(body) => {
                ("POST", "/api/observations".into(), Some(json!(body)))
            }
            Self::ListObservations(id) => ("GET", format!("/api/subjects/{id}/observations"), None),
            Self::GetJob(id) => ("GET", format!("/api/jobs/{id}"), None),
            Self::CancelJob(id, input) => {
                ("POST", format!("/api/jobs/{id}/cancel"), Some(json!(input)))
            }
        }
    }
}
