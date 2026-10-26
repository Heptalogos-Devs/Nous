use super::*;
use nous_foundation::localization::{LOCALIZATION, LocalizationPolicy, Localizer, Presentation};
use rmcp::model::ContentBlock;

impl McpService {
    pub(super) async fn request_presentation(
        &self,
        request: &RequestContext<RoleServer>,
    ) -> Result<Presentation, Error> {
        let explicit = locale(&request.meta)?;
        let connection = request
            .peer
            .peer_info()
            .and_then(|info| info.meta.as_ref().cloned());
        let connection = connection.as_ref().map(locale).transpose()?.flatten();
        let requested = explicit
            .or(connection)
            .or_else(|| crate::presentation::cli_language().map(str::to_owned));
        let inherited = request
            .extensions
            .get::<Presentation>()
            .cloned()
            .or_else(|| {
                request
                    .extensions
                    .get::<axum::http::request::Parts>()
                    .and_then(|parts| parts.extensions.get::<Presentation>())
                    .cloned()
            });
        if requested.is_none()
            && let Some(value) = &inherited
        {
            return Ok(value.clone());
        }
        let policy = if let Some(value) = &inherited {
            LocalizationPolicy {
                default_locale: value.locale().into(),
            }
        } else {
            match &self.backend {
                Backend::Local(app) => {
                    let context = authenticated(request)?;
                    app.config_view(&context, &nous_domain::ConfigScope::for_caller(&context))
                        .await?
                        .active
                        .get(LOCALIZATION)?
                }
                Backend::Remote(client) => {
                    let value = narrowed_client(client, request)?
                        .execute_domain(&Operation::Configuration(
                            nous_app::administration::configuration::ConfigRequest::Show {
                                scope: None,
                            },
                        ))
                        .await?;
                    let value = value
                        .pointer("/active/values/localization/json")
                        .cloned()
                        .ok_or_else(|| {
                            Error::internal("remote localization policy is not resolved")
                        })?;
                    serde_json::from_value(value)?
                }
            }
        };
        let localizer = Localizer::new(policy)?;
        localizer.select(&requested.as_deref().into_iter().collect::<Vec<_>>())
    }
    pub(super) async fn execute(
        &self,
        operation: Operation,
        request: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let presentation = match self.request_presentation(&request).await {
            Ok(value) => value,
            Err(error) => return Ok(error_result(error, &crate::presentation::current())),
        };
        let result = match &self.backend {
            Backend::Local(app) => match authenticated(&request) {
                Ok(context) => operation.execute(app, &context).await,
                Err(error) => Err(error),
            },
            Backend::Remote(client) => match narrowed_client(client, &request) {
                Ok(client) => {
                    client
                        .with_language(presentation.locale())
                        .execute_domain(&operation)
                        .await
                }
                Err(error) => Err(error),
            },
        };
        Ok(match result {
            Ok(value) => {
                let text = crate::presentation::render(&value, &presentation);
                let mut result = CallToolResult::structured(value);
                result.content = vec![ContentBlock::text(text)];
                result
            }
            Err(error) => error_result(error, &presentation),
        })
    }
}
fn locale(meta: &rmcp::model::RequestMetaObject) -> Result<Option<String>, Error> {
    meta.get("nous/locale")
        .map(|value| {
            value
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| Error::invalid("nous/locale must be a language identifier string"))
        })
        .transpose()
}
fn authenticated(request: &RequestContext<RoleServer>) -> Result<CallContext, Error> {
    let mut context = request
        .extensions
        .get::<axum::http::request::Parts>()
        .and_then(|parts| parts.extensions.get::<CallContext>())
        .cloned()
        .ok_or_else(|| {
            Error::new(
                ErrorCode::Unauthorized,
                "authenticated HTTP context is required",
            )
        })?;
    for scope in source_scopes(request)? {
        context = context.with_source_read(scope)?;
    }
    Ok(context)
}
fn source_scopes(
    request: &RequestContext<RoleServer>,
) -> Result<Vec<nous_domain::SourceReadScope>, Error> {
    let mut scopes = Vec::new();
    if let Some(peer) = request.peer.peer_info()
        && let Some(value) = peer
            .meta
            .as_ref()
            .and_then(|meta| meta.get("nous/source_read"))
    {
        scopes.extend(crate::http::source_read::from_value(value)?);
    }
    if let Some(value) = request.meta.get("nous/source_read") {
        scopes.extend(crate::http::source_read::from_value(value)?);
    }
    Ok(scopes)
}
fn narrowed_client(
    client: &ApiClient,
    request: &RequestContext<RoleServer>,
) -> Result<ApiClient, Error> {
    let mut client = client.clone();
    for scope in source_scopes(request)? {
        client = client.with_source_read(scope)?;
    }
    Ok(client)
}
fn error_result(error: Error, presentation: &Presentation) -> CallToolResult {
    let mut result =
        CallToolResult::structured_error(crate::presentation::error_document(&error, presentation));
    result.content = vec![ContentBlock::text(crate::presentation::error_text(
        &error,
        presentation,
    ))];
    result
}
