use super::*;

#[tool_router(router = integration_tool_router, vis = "pub(super)")]
impl McpService {
    #[tool(
        input_schema = schema::object_input::<crate::http::integrations::AmemorixRequest>(),
        description = "Use A-Memorix operations for memory search, import, source administration, maintenance, person profiles and exact lineage. Namespace and Subject select the Nous authority; source deletion and restoration use real native batch identities and transactions."
    )]
    async fn amemorix(
        &self,
        Parameters(input): Parameters<crate::http::integrations::AmemorixRequest>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.execute(Operation::Amemorix(Box::new(input)), ctx)
            .await
    }
}
