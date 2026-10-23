//! Adapted from rmcp official examples/servers/src/common/counter.rs and counter_stdio.rs.
//! Copyright Model Context Protocol contributors. Apache-2.0; see ../UPSTREAM.md.
use nous_app::App;
use nous_domain::{CallContext, Error, ErrorCode};
use rmcp::{
    ErrorData, RoleServer, ServerHandler,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{CallToolResult, Implementation, ServerCapabilities, ServerConfig},
    service::RequestContext,
    tool, tool_handler, tool_router,
};
use schemars::JsonSchema;
use serde::Deserialize;

use crate::api::{
    ApiClient, CreateNamespace, CreateSubject, Operation, PutMaterial, RecordObservation,
};
use crate::cognition_api::{
    CognitionOperation, DeleteMemory, EmbeddingSync, FormMemory, QueryInput,
};
use crate::maintenance_api::{
    ChangeInput, ImportInput, ListInput, MaintainInput, MaintenanceOperation,
};

#[derive(Clone)]
enum Backend {
    Local(App),
    Remote(ApiClient),
}

#[derive(Clone)]
pub struct McpService {
    backend: Backend,
    tool_router: ToolRouter<Self>,
}

#[derive(Deserialize, JsonSchema)]
pub struct NamespaceRequest {
    pub namespace_id: String,
}
#[derive(Deserialize, JsonSchema)]
pub struct SubjectRequest {
    pub subject_id: String,
}
#[derive(Deserialize, JsonSchema)]
pub struct MaterialRequest {
    pub material_id: String,
}
#[derive(Deserialize, JsonSchema)]
pub struct JobRequest {
    pub job_id: String,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ModelCallRequest {
    pub model_call_id: String,
}
#[derive(Deserialize, JsonSchema)]
pub struct MemoryRequest {
    pub memory_id: String,
}
#[derive(Deserialize, JsonSchema)]
pub struct DeleteMemoryRequest {
    pub memory_id: String,
    pub operation_id: String,
    pub expected_revision: i64,
}
#[derive(Deserialize, JsonSchema)]
pub struct ChangeMemoryRequest {
    pub memory_id: String,
    #[serde(flatten)]
    pub change: ChangeInput,
}
#[derive(Deserialize, JsonSchema)]
pub struct SubjectListRequest {
    pub subject_id: String,
    #[serde(default = "management_limit")]
    pub limit: usize,
}
#[derive(Deserialize, JsonSchema)]
pub struct MemoryAuditRequest {
    pub memory_id: String,
    #[serde(default = "management_limit")]
    pub limit: usize,
}
#[derive(Deserialize, JsonSchema)]
pub struct MaintainSubjectRequest {
    pub subject_id: String,
    #[serde(flatten)]
    pub input: MaintainInput,
}
fn management_limit() -> usize {
    50
}
#[derive(Deserialize, JsonSchema)]
pub struct CreateSubjectRequest {
    pub namespace_id: String,
    pub operation_id: String,
    pub name: String,
    #[serde(default)]
    pub capabilities: Vec<String>,
}

impl McpService {
    pub fn local(app: App) -> Self {
        Self {
            backend: Backend::Local(app),
            tool_router: Self::tool_router() + Self::maintenance_tool_router(),
        }
    }
    pub fn remote(client: ApiClient) -> Self {
        Self {
            backend: Backend::Remote(client),
            tool_router: Self::tool_router() + Self::maintenance_tool_router(),
        }
    }

    async fn execute(
        &self,
        operation: Operation,
        request: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let result = match &self.backend {
            Backend::Local(app) => {
                let context = request
                    .extensions
                    .get::<axum::http::request::Parts>()
                    .and_then(|parts| parts.extensions.get::<CallContext>())
                    .cloned();
                match context {
                    Some(context) => operation.execute(app, &context).await,
                    None => Err(Error::new(
                        ErrorCode::Unauthorized,
                        "authenticated HTTP context is required",
                    )),
                }
            }
            Backend::Remote(client) => client.execute_domain(&operation).await,
        };
        Ok(match result {
            Ok(value) => CallToolResult::structured(value),
            Err(error) => CallToolResult::structured_error(serde_json::json!({"error":error})),
        })
    }
}

#[tool_router]
impl McpService {
    #[tool(
        description = "Read Nous instance storage and capability status; administrator scope required"
    )]
    async fn status(&self, ctx: RequestContext<RoleServer>) -> Result<CallToolResult, ErrorData> {
        self.execute(Operation::Status, ctx).await
    }

    #[tool(
        description = "Create a complete authority-and-object snapshot with configuration and integrity manifest; administrator scope required; credentials excluded"
    )]
    async fn backup_create(
        &self,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.execute(Operation::CreateBackup, ctx).await
    }

    #[tool(description = "Create an isolated Namespace using an idempotent operation ID")]
    async fn namespace_create(
        &self,
        Parameters(input): Parameters<CreateNamespace>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.execute(Operation::CreateNamespace(input), ctx).await
    }

    #[tool(description = "List Namespaces visible to this credential")]
    async fn namespace_list(
        &self,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.execute(Operation::ListNamespaces, ctx).await
    }

    #[tool(description = "Create a Subject in a Namespace; supported capability is memory")]
    async fn subject_create(
        &self,
        Parameters(input): Parameters<CreateSubjectRequest>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let capabilities = if input.capabilities.is_empty() {
            vec!["memory".into()]
        } else {
            input.capabilities
        };
        self.execute(
            Operation::CreateSubject(
                input.namespace_id,
                CreateSubject {
                    operation_id: input.operation_id,
                    name: input.name,
                    capabilities,
                },
            ),
            ctx,
        )
        .await
    }

    #[tool(description = "List Subjects in a Namespace")]
    async fn subject_list(
        &self,
        Parameters(input): Parameters<NamespaceRequest>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.execute(Operation::ListSubjects(input.namespace_id), ctx)
            .await
    }

    #[tool(
        description = "Save UTF-8 material with immutable content and optional provenance; recording a Subject contact is a separate operation"
    )]
    async fn material_put(
        &self,
        Parameters(input): Parameters<PutMaterial>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.execute(Operation::PutMaterial(input), ctx).await
    }

    #[tool(description = "Read material text and its source metadata by stable material ID")]
    async fn material_get(
        &self,
        Parameters(input): Parameters<MaterialRequest>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.execute(Operation::GetMaterial(input.material_id), ctx)
            .await
    }

    #[tool(
        description = "Record a real Subject contact with saved material using an idempotent operation ID"
    )]
    async fn observation_record(
        &self,
        Parameters(input): Parameters<RecordObservation>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.execute(Operation::RecordObservation(input), ctx).await
    }

    #[tool(description = "List a Subject's recorded contacts with material")]
    async fn observation_list(
        &self,
        Parameters(input): Parameters<SubjectRequest>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.execute(Operation::ListObservations(input.subject_id), ctx)
            .await
    }

    #[tool(description = "Read a durable job and its current processing state")]
    async fn job_get(
        &self,
        Parameters(input): Parameters<JobRequest>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.execute(Operation::GetJob(input.job_id), ctx).await
    }

    #[tool(
        description = "Queue model formation from a real recorded Subject contact; returns a durable job"
    )]
    async fn memory_form(
        &self,
        Parameters(input): Parameters<FormMemory>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.execute(Operation::Cognition(CognitionOperation::Form(input)), ctx)
            .await
    }
    #[tool(
        description = "Queue missing text embeddings for a Subject's current memories and tags; returns durable jobs"
    )]
    async fn embedding_sync(
        &self,
        Parameters(input): Parameters<EmbeddingSync>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.execute(
            Operation::Cognition(CognitionOperation::EmbeddingSync(input)),
            ctx,
        )
        .await
    }
    #[tool(
        description = "Read an authorized durable embedding run and its current processing state"
    )]
    async fn embedding_run(
        &self,
        Parameters(input): Parameters<JobRequest>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.execute(
            Operation::Cognition(CognitionOperation::EmbeddingRun(input.job_id)),
            ctx,
        )
        .await
    }
    #[tool(
        description = "Read an authorized model call with its resource, usage and provider evidence"
    )]
    async fn model_call(
        &self,
        Parameters(input): Parameters<ModelCallRequest>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.execute(
            Operation::Cognition(CognitionOperation::ModelCall(input.model_call_id)),
            ctx,
        )
        .await
    }
    #[tool(description = "Read a current memory and its five time axes by stable memory ID")]
    async fn memory_get(
        &self,
        Parameters(input): Parameters<MemoryRequest>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.execute(
            Operation::Cognition(CognitionOperation::Get(input.memory_id)),
            ctx,
        )
        .await
    }
    #[tool(description = "Read retained revisions of a memory")]
    async fn memory_history(
        &self,
        Parameters(input): Parameters<MemoryRequest>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.execute(
            Operation::Cognition(CognitionOperation::History(input.memory_id)),
            ctx,
        )
        .await
    }
    #[tool(
        description = "Soft-delete a memory with an explicit expected current revision; keeps retained history"
    )]
    async fn memory_delete(
        &self,
        Parameters(input): Parameters<DeleteMemoryRequest>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.execute(
            Operation::Cognition(CognitionOperation::Delete(
                input.memory_id,
                DeleteMemory {
                    operation_id: input.operation_id,
                    expected_revision: input.expected_revision,
                },
            )),
            ctx,
        )
        .await
    }
    #[tool(description = "Trace a memory to original material, real contact and actual model call")]
    async fn memory_source(
        &self,
        Parameters(input): Parameters<MemoryRequest>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.execute(
            Operation::Cognition(CognitionOperation::Source(input.memory_id)),
            ctx,
        )
        .await
    }
    #[tool(
        description = "Search a Subject's memories using lexical, dense, VCP or hybrid mode with exact reference, source or time filters; returns index coverage and semantic provenance"
    )]
    async fn query(
        &self,
        Parameters(input): Parameters<QueryInput>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.execute(Operation::Cognition(CognitionOperation::Query(input)), ctx)
            .await
    }
    #[tool(
        description = "Inspect model formation configuration and readiness; administrator scope required"
    )]
    async fn model_status(
        &self,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.execute(Operation::Cognition(CognitionOperation::ModelStatus), ctx)
            .await
    }
}

#[tool_router(router = maintenance_tool_router)]
impl McpService {
    #[tool(
        description = "Apply a revision-checked memory action: reinforce, weaken, protect, unprotect, freeze, forget, restore, trash, purge, revise or decay"
    )]
    async fn memory_change(
        &self,
        Parameters(input): Parameters<ChangeMemoryRequest>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.execute(
            Operation::Maintenance(MaintenanceOperation::Change(input.memory_id, input.change)),
            ctx,
        )
        .await
    }
    #[tool(description = "List a Subject's soft-deleted memories from the recycle bin")]
    async fn memory_recycle_bin(
        &self,
        Parameters(input): Parameters<SubjectListRequest>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.execute(
            Operation::Maintenance(MaintenanceOperation::Recycle(
                input.subject_id,
                ListInput { limit: input.limit },
            )),
            ctx,
        )
        .await
    }
    #[tool(description = "Read the durable actor, reason and revision audit for a memory")]
    async fn memory_audit(
        &self,
        Parameters(input): Parameters<MemoryAuditRequest>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.execute(
            Operation::Maintenance(MaintenanceOperation::Audit(
                input.memory_id,
                ListInput { limit: input.limit },
            )),
            ctx,
        )
        .await
    }
    #[tool(
        description = "Run bounded lifecycle maintenance for a Subject using an idempotent operation ID"
    )]
    async fn memory_maintain(
        &self,
        Parameters(input): Parameters<MaintainSubjectRequest>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.execute(
            Operation::Maintenance(MaintenanceOperation::Maintain(
                input.subject_id,
                input.input,
            )),
            ctx,
        )
        .await
    }
    #[tool(
        description = "Import faithful memories from bounded text, JSON or JSONL source records, preserving structured paragraph/LPMM units; returns per-item results and optional asynchronous model formation jobs"
    )]
    async fn import_text(
        &self,
        Parameters(input): Parameters<ImportInput>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.execute(
            Operation::Maintenance(MaintenanceOperation::Import(input)),
            ctx,
        )
        .await
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for McpService {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::from_build_env())
            .with_instructions("Nous Wave provides isolated identity, immutable material, real contacts and durable work. Supply a stable operation_id for retries. Material storage and Subject contact are separate operations.")
    }
}
