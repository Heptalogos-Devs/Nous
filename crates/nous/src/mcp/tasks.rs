use super::*;

#[tool_router(router=task_wait_tool_router,vis="pub(super)")]
impl McpService {
    #[tool(
        description = "Wait for authorized jobs to finish or require attention, using an explicit timeout in milliseconds. Returns their current states; waiting does not cancel durable work."
    )]
    async fn job_wait(
        &self,
        Parameters(input): Parameters<crate::http::tasks::WaitJobs>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.execute(Operation::WaitJobs(input), ctx).await
    }
}
