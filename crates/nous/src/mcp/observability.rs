use super::*;

#[tool_router(router = observability_tool_router, vis = "pub(super)")]
impl McpService {
    #[tool(
        description = "Follow one persisted trace across committed operations, durable tasks and paid model attempts; administrator scope required. Reuse next_cursor to read another bounded page."
    )]
    async fn trace_records(
        &self,
        Parameters(input): Parameters<nous_app::administration::observability::TraceRequest>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.execute(Operation::Trace(input), ctx).await
    }
    #[tool(
        description = "Inspect active log destinations, process diagnostics and dropped log records; administrator scope required"
    )]
    async fn diagnostics(
        &self,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.execute(Operation::Diagnostics, ctx).await
    }
}
