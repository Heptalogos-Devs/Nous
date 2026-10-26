use super::*;

#[tool_router(router=context_tool_router,vis="pub(super)")]
impl McpService {
    #[tool(
        input_schema=schema::object_input::<crate::http::contexts::ContextOperation>(),
        description="Manage WorkContexts and sessions. Prepare cognition before a model call, admit the actually applied segments with the host's measured input budget, then report outcomes and meaningful use. Read and change resident items by exact reference."
    )]
    async fn contexts(&self,Parameters(input):Parameters<crate::http::contexts::ContextOperation>,ctx:RequestContext<RoleServer>)->Result<CallToolResult,ErrorData> {
        self.execute(Operation::Context(Box::new(input)),ctx).await
    }
}
