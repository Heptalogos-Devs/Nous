use super::*;

#[tool_router(router=identity_tool_router,vis="pub(super)")]
impl McpService {
    #[tool(
        description = "Manage current scoped access grants: inspect identity, issue controlled delegates, rotate or revoke credentials, and read exact authorized credential references."
    )]
    async fn identity(
        &self,
        Parameters(input): Parameters<nous_app::administration::identity::IdentityRequest>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.execute(Operation::Identity(input), ctx).await
    }
}
