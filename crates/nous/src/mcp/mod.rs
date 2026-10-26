use nous_app::App;
mod bundles;
mod contexts;
mod identity;
mod integrations;
mod media;
mod observability;
mod presentation;
mod query;
mod schema;
mod sources;
mod tasks;
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
use std::sync::Arc;

use crate::http::cognition::{CognitionOperation, DeleteMemory, EmbeddingSync, FormMemory};
use crate::http::maintenance::{
    ChangeInput, ImportInput, ListInput, MaintainInput, MaintenanceOperation,
};
use crate::http::{
    ApiClient, CancelJob, CreateNamespace, CreateSubject, Operation, PutMaterial, RecordObservation,
};

#[derive(Clone)]
enum Backend {
    Local(Arc<App>),
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
pub struct CancelJobRequest {
    pub job_id: String,
    pub operation_id: String,
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
#[serde(deny_unknown_fields)]
pub struct MemorySourceRequest {
    pub memory_id: String,
    pub revision: Option<i64>,
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
            backend: Backend::Local(Arc::new(app)),
            tool_router: Self::tool_router()
                + Self::maintenance_tool_router()
                + Self::observability_tool_router()
                + Self::identity_tool_router()
                + Self::media_tool_router()
                + Self::query_tool_router()
                + Self::context_tool_router()
                + Self::bundle_tool_router()
                + Self::source_tool_router()
                + Self::integration_tool_router()
                + Self::task_wait_tool_router(),
        }
    }
    pub fn remote(client: ApiClient) -> Self {
        Self {
            backend: Backend::Remote(client),
            tool_router: Self::tool_router()
                + Self::maintenance_tool_router()
                + Self::observability_tool_router()
                + Self::identity_tool_router()
                + Self::media_tool_router()
                + Self::query_tool_router()
                + Self::context_tool_router()
                + Self::bundle_tool_router()
                + Self::source_tool_router()
                + Self::integration_tool_router()
                + Self::task_wait_tool_router(),
        }
    }
}

#[tool_router]
impl McpService {
    #[tool(
        input_schema = schema::object_input::<nous_app::administration::prompts::PromptRequest>(),
        description = "List, read, preview, edit immutable scoped Prompt resources and history, or activate a default/file/exact managed revision in a role. Editing and optional activation share one CAS operation; no provider call is made."
    )]
    async fn prompts(
        &self,
        Parameters(input): Parameters<nous_app::administration::prompts::PromptRequest>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.execute(Operation::Prompt(input), ctx).await
    }

    #[tool(
        description = "Read a registered model role's typed input contract, requirements and Prompt binding without invoking a provider. Optional input builds the consumer's actual messages and dynamic output schema even without a configured route. Scope defaults to this credential's scope."
    )]
    async fn model_contract(
        &self,
        Parameters(input): Parameters<nous_app::administration::models::ModelContractRequest>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.execute(Operation::ModelContract(input), ctx).await
    }
    #[tool(
        description = "Preview the actual invocation messages, media access, variables, Prompt, output contract and target JSON Schema using typed role input. Optional edits.prompt changes only this preview; no provider call or model budget reservation occurs."
    )]
    async fn model_preview(
        &self,
        Parameters(input): Parameters<nous_app::administration::models::ModelPreviewRequest>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.execute(Operation::ModelPreview(input), ctx).await
    }
    #[tool(
        input_schema = schema::object_input::<nous_app::administration::configuration::ConfigRequest>(),
        description = "Read scoped active/desired configuration and its sources, inspect typed definitions/history, or apply one atomic CAS set/clear batch. Instance, Namespace and Subject permissions are enforced; restart changes remain pending until their owner applies them."
    )]
    async fn config(
        &self,
        Parameters(input): Parameters<nous_app::administration::configuration::ConfigRequest>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.execute(Operation::Configuration(input), ctx).await
    }
    #[tool(
        input_schema = schema::object_input::<crate::http::structured::StructuredOperation>(),
        description = "Create, revise, retire, restore, inspect typed Concept, Episode, Journal, Schema, Association and Memory semantics. Source returns exact revision evidence and original material; revisions use optimistic concurrency."
    )]
    async fn cognition(
        &self,
        Parameters(input): Parameters<crate::http::structured::StructuredOperation>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.execute(Operation::Structured(Box::new(input)), ctx)
            .await
    }
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
        description = "Cancel an accepted task using an idempotent operation ID; the task scope controls authorization"
    )]
    async fn job_cancel(
        &self,
        Parameters(input): Parameters<CancelJobRequest>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.execute(
            Operation::CancelJob(
                input.job_id,
                CancelJob {
                    operation_id: input.operation_id,
                },
            ),
            ctx,
        )
        .await
    }
    #[tool(
        description = "Queue Memory formation from a recorded contact or import. An optional exact MediaReference selects an original, region, committed description or interpretation; omitted selects the observation's original. Preserves all source roots and the actual source event kind."
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
        Parameters(input): Parameters<MemorySourceRequest>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.execute(
            Operation::Cognition(CognitionOperation::Source {
                id: input.memory_id,
                revision: input.revision,
            }),
            ctx,
        )
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
