use super::*;

#[tool_router(router = bundle_tool_router, vis = "pub(super)")]
impl McpService {
    #[tool(
        input_schema = schema::object_input::<crate::http::bundles::BundleOperation>(),
        description = "Export portable subject knowledge, install an uploaded native or A-Memorix bundle, list owned resources, and uninstall an installation. Upload/download archive bytes through the material API; installation preserves source history and uses one authority transaction."
    )]
    async fn bundles(
        &self,
        Parameters(input): Parameters<crate::http::bundles::BundleOperation>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.execute(Operation::Bundle(Box::new(input)), ctx).await
    }
}
