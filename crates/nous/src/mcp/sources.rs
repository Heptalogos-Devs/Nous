use super::*;

#[tool_router(router = source_tool_router, vis = "pub(super)")]
impl McpService {
    #[tool(
        description = "Read the exact source lineage of any cognition revision or media reference for a Subject. An optional observation_id qualifies the actual media contact/import; cognition already carries its source events."
    )]
    async fn source_read(
        &self,
        Parameters(input): Parameters<crate::http::sources::RetrievalSourceRequest>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.execute(Operation::ReadSource(input), ctx).await
    }
}
