use super::*;

#[tool_router(router = integration_tool_router, vis = "pub(super)")]
impl McpService {
    #[tool(
        input_schema = schema::object_input::<crate::http::integrations::AmemorixRequest>(),
        description = "Use A-Memorix consumer operations for memory search, text/summary ingest, maintenance, person profiles and exact sources. Namespace and Subject select the Nous authority; tool arguments retain the consumer's chat, person, time and filtering semantics."
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
