use super::*;
use rmcp::model::ContentBlock;

#[derive(Default, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum QueryFormat {
    #[default]
    Brief,
    Json,
}

#[derive(Deserialize, JsonSchema)]
pub struct QueryInput {
    #[serde(flatten)]
    pub query: nous_domain::QueryRequest,
    #[serde(default)]
    pub format: QueryFormat,
    pub language: Option<String>,
}

#[tool_router(router=query_tool_router,vis="pub(super)")]
impl McpService {
    #[tool(
        input_schema = schema::object_input::<QueryInput>(),
        description = "Search cognition and media using text, exact references or joint text/media input. Read results by their stable references; format=json returns complete records and diagnostics."
    )]
    async fn query(
        &self,
        Parameters(input): Parameters<QueryInput>,
        mut ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        if let Some(language) = input.language {
            ctx.meta
                .insert("nous/locale".into(), serde_json::Value::String(language));
        }
        let presentation = self.request_presentation(&ctx).await.map_err(|error| {
            ErrorData::invalid_params(
                error.message.clone(),
                Some(crate::presentation::error_document(
                    &error,
                    &crate::presentation::current(),
                )),
            )
        })?;
        ctx.extensions.insert(presentation.clone());
        let mut result = self
            .execute(
                Operation::Cognition(CognitionOperation::Query(input.query)),
                ctx,
            )
            .await?;
        if matches!(input.format, QueryFormat::Brief)
            && result.is_error != Some(true)
            && let Some(value) = &result.structured_content
        {
            let value: nous_domain::QueryResult = serde_json::from_value(value.clone())
                .map_err(|error| ErrorData::internal_error(error.to_string(), None))?;
            result.content = vec![ContentBlock::text(crate::presentation::query_result(
                &value,
                &presentation,
            ))];
            result.structured_content = None;
        } else if matches!(input.format, QueryFormat::Json)
            && let Some(value) = &result.structured_content
        {
            result.content = vec![ContentBlock::text(
                serde_json::to_string_pretty(value)
                    .map_err(|error| ErrorData::internal_error(error.to_string(), None))?,
            )];
        }
        Ok(result)
    }
}
