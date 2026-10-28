use super::*;

#[tool_router(router = import_tool_router, vis = "pub(super)")]
impl McpService {
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
