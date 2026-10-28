use super::*;

#[tool_router(router = import_tool_router, vis = "pub(super)")]
impl McpService {
    #[tool(
        description = "Admit supplied import file documents under the native dedupe policy, freeze selected text units and submit real processing tasks. Accepts inline documents or authorized MaterialIds and optional opaque caller file identities. Returns per-input process/skip decisions; an all-skipped batch has null catalog_id and processing_job_id."
    )]
    async fn import_admit_files(
        &self,
        Parameters(input): Parameters<crate::http::import_files::AdmitImportFilesInput>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.execute(Operation::AdmitImportFiles(Box::new(input)), ctx)
            .await
    }
    #[tool(
        description = "Read an adopted immutable import unit output under the unit's actual source-read permissions. Includes exact Memory/cognition references and real semantic/temporal call identities."
    )]
    async fn import_output(
        &self,
        Parameters(input): Parameters<crate::http::import_processing::ReadImportOutputInput>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.execute(Operation::ReadImportOutput(input), ctx).await
    }
    #[tool(
        description = "Freeze immutable text units, inspect a retained catalog, or read explicit retry UnitIds. Freezing saves input and region facts; retry_inputs reuses exact original ranges and configuration without running processing tasks."
    )]
    async fn import_catalog(
        &self,
        Parameters(input): Parameters<crate::http::import_catalogs::ImportCatalogOperation>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.execute(Operation::ImportCatalog(Box::new(input)), ctx)
            .await
    }
    #[tool(
        description = "Process selected frozen import units with independent durable child tasks. Semantic and temporal models use registered roles; vector policy controls actual indexing jobs. Returns a parent job_id whose result references the real unit jobs."
    )]
    async fn import_process(
        &self,
        Parameters(input): Parameters<crate::http::import_processing::ProcessImportInput>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.execute(Operation::ProcessImportUnits(Box::new(input)), ctx)
            .await
    }
    #[tool(
        description = "Preview a text import's selected strategy, exact original UTF-8 byte spans, unit text and split warnings. Accepts inline text or an authorized MaterialId; does not create Materials, Jobs or model calls."
    )]
    async fn import_preview(
        &self,
        Parameters(input): Parameters<nous_domain::PreviewTextImportRequest>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.execute(Operation::PreviewTextImport(Box::new(input)), ctx)
            .await
    }
}
