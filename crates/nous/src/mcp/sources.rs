use super::*;

#[tool_router(router = source_tool_router, vis = "pub(super)")]
impl McpService {
    #[tool(input_schema=schema::object_input::<crate::http::sources::SourceManagementOperation>(),description="Manage exact named source groups, preview or atomically trash selected labels, restore an exact trash batch, or inspect authorized source events and logical trash history. Preview returns the current plan without a receipt. Writes require operation_id. Logical history links the actual restore receipt to its original trash commit; later revisions are protected.")]
    async fn source_management(
        &self,
        Parameters(input): Parameters<crate::http::sources::SourceManagementOperation>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.execute(Operation::SourceManagement(Box::new(input)), ctx)
            .await
    }
    #[tool(input_schema=schema::object_input::<nous_app::references::ReferenceOperation>(),description="Resolve a returned Proquint address to a canonical exact reference within the Subject's current or historical source scope. Namespace operators can explicitly reconcile missing addresses; resolution is read-only and never invokes a model.")]
    async fn references(
        &self,
        Parameters(input): Parameters<nous_app::references::ReferenceOperation>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.execute(Operation::Reference(input), ctx).await
    }
    #[tool(
        description = "Read source lineage using a returned readable address or a canonical typed cognition/media reference. An optional observation_id qualifies actual media contact/import; cognition carries its own source events."
    )]
    async fn source_read(
        &self,
        Parameters(input): Parameters<crate::http::sources::RetrievalSourceRequest>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.execute(Operation::ReadSource(input), ctx).await
    }
}
