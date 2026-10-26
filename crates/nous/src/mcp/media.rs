use super::*;

#[tool_router(router=media_tool_router,vis="pub(super)")]
impl McpService {
    #[tool(
        description = "Inspect media and exact source regions; create persistent descriptions or structured interpretations; read derivation jobs, representations and revision history. Every result retains original sources and actual model inputs."
    )]
    async fn media(
        &self,
        Parameters(input): Parameters<crate::http::media::MediaOperation>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.execute(Operation::Media(Box::new(input)), ctx).await
    }
}
